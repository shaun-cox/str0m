use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;
use std::net::SocketAddr;
use std::ops::Add;

use crate::peer::OutputQueue;
use crate::sdp::{Candidate, IceCreds, SessionId};
use crate::stun::StunMessage;
use crate::util::{random_id, Ts};
use crate::Error;

#[derive(Debug)]
pub(crate) struct IceState {
    /// Id of session, used for logging
    session_id: SessionId,

    /// Whether this is the controlling agent.
    controlling: bool,

    /// If we are running ice-lite mode and only deal with local host candidates.
    ice_lite: bool,

    /// If we got indication there be no more local candidates.
    local_end_of_candidates: bool,

    /// If we got indication there be no more remote candidates.
    remote_end_of_candidates: bool,

    /// State of checking connection.
    conn_state: IceConnectionState,

    /// Local credentials for STUN. We use one set for all m-lines.
    local_creds: IceCreds,

    /// Remote credentials for STUN. Obtained from SDP.
    remote_creds: HashSet<IceCreds>,

    /// Addresses that have been "unlocked" via STUN. These IP:PORT combos
    /// are now verified for other kinds of data like DTLS, RTP, RTCP...
    verified: HashSet<SocketAddr>,

    /// Candidates, in the order they drop in.
    local_candidates: Vec<Candidate>,

    /// Candidates, in the order they drop in.
    remote_candidates: Vec<Candidate>,

    /// Pairs formed by combining all local/remote as they drop in.
    candidate_pairs: Vec<CandidatePair>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceConnectionState {
    /// Waiting for candidates.
    New,

    /// Checking pairs of local-remote candidates.
    Checking,

    /// A usable pair of local-remote candidates found, but still checking.
    Connected,

    /// A usable pair of local-remote candidates found. Checking is finished.
    Completed,

    /// No connection found from the candidate pairs.
    Failed,

    /// Shut down.
    Closed,
}

impl IceConnectionState {
    fn should_check(&self) -> bool {
        use IceConnectionState::*;
        matches!(self, New | Checking | Connected)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CandidatePair {
    /// Index into local_candidates.
    local_idx: usize,
    /// Index into remote_candidates.
    remote_idx: usize,
    /// Calculated prio for this pair. This is the basis
    /// for sorting the pairs.
    prio: u64,
    /// Current state of checking the entry.
    state: CheckState,
    /// The time we attempted to send a STUN request using this pair.
    attempted: Option<Ts>,
    /// Transaction id to tally up reply wth request.
    trans_id: Option<[u8; 12]>,
}

impl PartialOrd for CandidatePair {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CandidatePair {
    fn cmp(&self, other: &Self) -> Ordering {
        self.prio.cmp(&other.prio)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CheckState {
    Waiting,
    InProgress,
    Succeeded,
    Failed,
}

impl IceState {
    pub fn new(session_id: SessionId, ice_lite: bool) -> Self {
        IceState {
            session_id,
            controlling: false,
            ice_lite,
            local_end_of_candidates: false,
            remote_end_of_candidates: false,
            conn_state: IceConnectionState::New,
            local_creds: IceCreds {
                username: random_id::<8>().to_string(),
                password: random_id::<24>().to_string(),
            },
            remote_creds: HashSet::new(),
            verified: HashSet::new(),
            local_candidates: vec![],
            remote_candidates: vec![],
            candidate_pairs: Vec::new(),
        }
    }

    pub fn can_set_controlling(&mut self) -> bool {
        self.candidate_pairs.is_empty()
    }

    pub fn set_controlling(&mut self, c: bool) {
        assert!(self.candidate_pairs.is_empty());
        debug!(
            "{:?} Ice agent is {}",
            self.session_id,
            if c { "controlling" } else { "controlled" }
        );
        self.controlling = c;
    }

    pub fn add_local_candidate(&mut self, c: Candidate) {
        if self.local_end_of_candidates {
            debug!(
                "{:?} No more local candidates accepted: end-of-candidates",
                self.session_id
            );
        }

        if self.ice_lite && !c.is_host() {
            debug!(
                "{:?} Ignoring non-host ICE candidate due to ice-lite: {:?}",
                self.session_id, c
            );
            return;
        }

        debug!("{:?} Adding local candidate: {}", self.session_id, c);

        let add = AddCandidate {
            candidate: c,
            add_to: &mut self.local_candidates,
            pair_with: &self.remote_candidates,
            pair_to: &mut self.candidate_pairs,
            prio_left: self.controlling,
        };

        IceState::do_add_candidate(add)
    }

    pub fn add_remote_candidate(&mut self, c: Candidate) {
        if self.local_end_of_candidates {
            debug!(
                "{:?} No more remote candidates accepted: end-of-candidates",
                self.session_id
            );
        }

        debug!("{:?} Adding remote candidate: {:?}", self.session_id, c);

        let add = AddCandidate {
            candidate: c,
            add_to: &mut self.remote_candidates,
            pair_with: &self.local_candidates,
            pair_to: &mut self.candidate_pairs,
            prio_left: !self.controlling,
        };

        IceState::do_add_candidate(add)
    }

    fn do_add_candidate(add: AddCandidate<'_>) {
        if add.add_to.contains(&add.candidate) {
            // TODO this should keep the one with lower priority.
            trace!("Not adding redundant candidate: {:?}", add.candidate);
            return;
        }

        if add.pair_to.len() >= 100 {
            debug!("Ignoring further ice candidates since we got >= 100 pairs");
            return;
        }

        add.add_to.push(add.candidate);
        let left = add.add_to.last().unwrap();
        let left_idx = add.add_to.len() - 01;
        let left_prio = left.prio() as u64;

        for (right_idx, right) in add.pair_with.iter().enumerate() {
            let right_prio = right.prio() as u64;

            // Once the pairs are formed, a candidate pair priority is computed.
            // Let G be the priority for the candidate provided by the controlling
            // agent.  Let D be the priority for the candidate provided by the
            // controlled agent.  The priority for a pair is computed as:
            // pair priority = 2^32*MIN(G,D) + 2*MAX(G,D) + (G>D?1:0)

            let (g, d) = if add.prio_left {
                (left_prio, right_prio)
            } else {
                (right_prio, left_prio)
            };

            let prio = 2 ^ 32 * g.min(d) + 2 * g.max(d) + if g > d { 1 } else { 0 };

            let pair = CandidatePair {
                local_idx: if add.prio_left { left_idx } else { right_idx },
                remote_idx: if add.prio_left { right_idx } else { left_idx },
                prio,
                state: CheckState::Waiting,
                attempted: None,
                trans_id: None,
            };

            add.pair_to.push(pair);

            // Note: It would seem more efficient to use a BTreeSet to keep the
            // order sorted as soon as we insert an entry. The problem is that
            // we have state in the CandidatePair that is hard to manipulate when
            // in a hashed set.
            add.pair_to.sort();
        }
    }

    pub fn add_remote_creds(&mut self, creds: IceCreds) {
        let line = format!("{:?} Added remote creds: {:?}", self.session_id, creds);
        if self.remote_creds.insert(creds) {
            trace!(line);
        }
    }

    pub fn accepts_stun(&self, target: SocketAddr, stun: &StunMessage<'_>) -> Result<bool, Error> {
        let (local, remote) = stun.split_username();

        let (local_username, remote_username) = if self.controlling {
            (remote, local)
        } else {
            (local, remote)
        };

        let creds_in_remote_sdp = self
            .remote_creds
            .iter()
            .any(|c| c.username == remote_username);

        if !creds_in_remote_sdp {
            // this is not a fault, the packet might not be for this peer.
            return Ok(false);
        }

        if local_username != self.local_creds.username {
            // this is a bit suspicious... maybe a name clash on the remote username?
            return Err(Error::StunError(format!(
                "STUN local != peer.local ({}): {} != {}",
                target, local_username, self.local_creds.username
            )));
        }

        let mut check_ok = false;

        if self.controlling {
            for creds in self.remote_creds.iter() {
                check_ok |= stun.check_integrity(&creds.password);
            }
        } else {
            check_ok = stun.check_integrity(&self.local_creds.password);
        }

        if !check_ok {
            return Err(Error::StunError(format!(
                "STUN check_integrity failed ({})",
                target,
            )));
        }

        Ok(true)
    }

    pub fn handle_stun<'a>(
        &mut self,
        source: SocketAddr,
        target: SocketAddr,
        output: &mut OutputQueue,
        stun: StunMessage<'a>,
    ) -> Result<(), Error> {
        // fail if this is not for us.
        self.accepts_stun(target, &stun)?;

        // on the back of a successful (authenticated) stun bind, we update
        // the validated addresses to receive dtls, rtcp, rtp etc.
        if self.verified.insert(target) {
            trace!("{:?} STUN new verified peer ({})", self.session_id, target);
        }

        use IceConnectionState::*;
        self.set_conn_state(if self.has_more_candidates_to_check() {
            Connected
        } else {
            Completed
        });

        if stun.is_binding_response() {
            let pair = self
                .candidate_pairs
                .iter_mut()
                .find(|c| c.trans_id.as_ref().map(|t| t.as_slice()) == Some(stun.trans_id()));

            if let Some(pair) = pair {
                pair.state = CheckState::Succeeded;
            } else {
                return Err(Error::StunError(
                    "Failed to find STUN request via transaction id".into(),
                ));
            }

            return Ok(());
        }

        // TODO: do we ever get binding failures?
        assert!(stun.is_binding_request());

        trace!("{:?} STUN reply to ({})", self.session_id, source);

        let reply = stun.reply()?;

        let mut writer = output.get_buffer_writer();
        let len = reply.to_bytes(&self.local_creds.password, &mut writer)?;
        let buffer = writer.set_len(len);

        output.enqueue(target, source, buffer);

        Ok(())
    }

    pub fn is_stun_verified(&self, addr: SocketAddr) -> bool {
        self.verified.contains(&addr)
    }

    pub fn has_any_verified(&self) -> bool {
        !self.verified.is_empty()
    }

    pub fn local_creds(&self) -> &IceCreds {
        &self.local_creds
    }

    pub fn local_candidates(&self) -> &[Candidate] {
        &self.local_candidates
    }

    pub fn set_remote_end_of_candidates(&mut self) {
        if self.remote_end_of_candidates {
            return;
        }
        info!("{:?} Remote end-of-candidates", self.session_id);
        self.remote_end_of_candidates = true;
    }

    pub fn set_local_end_of_candidates(&mut self) {
        if self.local_end_of_candidates {
            return;
        }
        info!("{:?} Local end-of-candidates", self.session_id);
        self.local_end_of_candidates = true;
    }

    pub fn local_end_of_candidates(&self) -> bool {
        self.local_end_of_candidates
    }

    fn set_conn_state(&mut self, c: IceConnectionState) {
        if c != self.conn_state {
            info!(
                "{:?} Ice connection state change: {} -> {}",
                self.session_id, self.conn_state, c
            );
            self.conn_state = c;
        }
        // TODO emit event that this is happening.
    }

    pub fn drive_stun_controlling(
        &mut self,
        time: Ts,
        queue: &mut OutputQueue,
    ) -> Result<(), Error> {
        if !self.controlling {
            return Ok(());
        }

        use IceConnectionState::*;

        if self.conn_state.should_check() {
            const MAX_CONCURRENT: usize = 10;

            if self.conn_state == New {
                self.set_conn_state(Checking);
            }

            while self.count_candidates_in_progress() < MAX_CONCURRENT {
                // The candidates are ordered in prio order, so the first in Waiting is
                // the top prio pair
                let next = self
                    .candidate_pairs
                    .iter_mut()
                    .find(|c| c.state == CheckState::Waiting);

                if let Some(next) = next {
                    let local_creds = &self.local_creds;
                    let remote_creds = self
                        .remote_creds
                        .iter()
                        .next()
                        .expect("Must have remote ice credentials");

                    let local = &self.local_candidates[next.local_idx];
                    let remote = &self.remote_candidates[next.remote_idx];

                    let req = BindingReq {
                        id: &self.session_id,
                        next,
                        local,
                        remote,
                        time,
                        local_creds,
                        remote_creds,
                        queue,
                    };

                    IceState::send_binding_request(req)?;
                } else {
                    // No more candidates to check.
                    if self.conn_state == Connected {
                        self.set_conn_state(Completed);
                    }
                    break;
                }
            }
        }

        Ok(())
    }

    fn has_more_candidates_to_check(&self) -> bool {
        self.candidate_pairs
            .iter()
            .any(|c| c.state == CheckState::Waiting)
    }

    fn count_candidates_in_progress(&self) -> usize {
        self.candidate_pairs
            .iter()
            .filter(|c| c.state == CheckState::InProgress)
            .count()
    }

    fn send_binding_request(mut req: BindingReq<'_>) -> Result<(), Error> {
        assert!(req.next.state == CheckState::Waiting);
        assert!(req.next.attempted.is_none());

        req.next.state = CheckState::InProgress;
        req.next.attempted = Some(req.time);

        let remote_local = format!("{}:{}", req.remote_creds.username, req.local_creds.username);
        let trans_id = random_id::<12>().into_array();

        let msg = StunMessage::binding_request(&remote_local, &trans_id);

        let mut writer = req.queue.get_buffer_writer();
        let len = msg.to_bytes(&req.remote_creds.password, &mut writer)?;
        let buffer = writer.set_len(len);

        req.next.trans_id = Some(trans_id);

        let source = req.local.addr();
        let target = req.remote.addr();

        trace!("{:?} STUN binding request to: {}", req.id, target);

        req.queue.enqueue(source, target, buffer);

        Ok(())
    }
}

struct AddCandidate<'a> {
    candidate: Candidate,
    add_to: &'a mut Vec<Candidate>,
    pair_with: &'a Vec<Candidate>,
    pair_to: &'a mut Vec<CandidatePair>,
    prio_left: bool,
}

struct BindingReq<'a> {
    id: &'a SessionId,
    next: &'a mut CandidatePair,
    local: &'a Candidate,
    remote: &'a Candidate,
    time: Ts,
    local_creds: &'a IceCreds,
    remote_creds: &'a IceCreds,
    queue: &'a mut OutputQueue,
}

impl fmt::Display for IceConnectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use IceConnectionState::*;
        write!(
            f,
            "{}",
            match self {
                New => "new",
                Checking => "checking",
                Connected => "connected",
                Completed => "completed",
                Failed => "failed",
                Closed => "closed",
            }
        )
    }
}
