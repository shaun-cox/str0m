use super::{LatencyEstimator, Micros};
use std::{fmt::Display, sync::MutexGuard, time::Instant};
use tracing::warn;

pub struct CallSiteStats {
    location: SourceLocation,
    max: Option<Micros>,
    avg: LatencyEstimator,
}

impl CallSiteStats {
    pub const fn new(location: SourceLocation) -> Self {
        Self {
            location,
            max: None,
            avg: LatencyEstimator::new(),
        }
    }

    pub fn maybe_reveal(&mut self, latency: Micros, iterations: usize) {
        let prior_max = self.max.unwrap_or(Micros::ZERO).as_value();
        let prior_max_exceeded = latency > Micros(prior_max);

        if latency > Micros(500) && self.avg.has_sample() {
            if prior_max_exceeded {
                warn!(
                    latency = latency.as_value(),
                    iterations,
                    avg = self.avg.mean().as_value(),
                    dev = self.avg.deviation().as_value(),
                    prior_max,
                    func = self.location.func,
                    module = self.location.module,
                    file = %self.location.file,
                    "Execution (µs) exceeded prior max",
                );
            } else if latency > Micros(5000) {
                warn!(
                    latency = latency.as_value(),
                    iterations,
                    avg = self.avg.mean().as_value(),
                    dev = self.avg.deviation().as_value(),
                    prior_max,
                    func = self.location.func,
                    module = self.location.module,
                    file = %self.location.file,
                    "Execution (µs) exceeded threshold",
                );
            } else if self.avg.mean() > Micros(50) && latency > self.avg.mean() * 20 {
                warn!(
                    latency = latency.as_value(),
                    iterations,
                    avg = self.avg.mean().as_value(),
                    dev = self.avg.deviation().as_value(),
                    prior_max,
                    func = self.location.func,
                    module = self.location.module,
                    file = %self.location.file,
                    "Execution (µs) exceeded 20x average",
                );
            }
        }
        if prior_max_exceeded {
            self.max = Some(latency);
        }
        self.avg.record(latency);
    }
}

pub struct CallSiteUsage<'a> {
    stats: MutexGuard<'a, CallSiteStats>,
    start_time: Instant,
    iteration_count: usize,
}

impl<'a> CallSiteUsage<'a> {
    pub fn new(stats: MutexGuard<'a, CallSiteStats>) -> Self {
        Self {
            stats,
            start_time: Instant::now(),
            iteration_count: 0,
        }
    }

    pub fn record_iteration(&mut self) {
        self.iteration_count += 1;
    }
}

impl<'a> Drop for CallSiteUsage<'a> {
    fn drop(&mut self) {
        let latency = self.start_time.elapsed().into();
        self.stats.maybe_reveal(latency, self.iteration_count);
    }
}

pub struct FileLine(pub &'static str, pub u32);

pub struct SourceLocation {
    pub func: &'static str,
    pub module: &'static str,
    pub file: FileLine,
}

impl Display for FileLine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.0, self.1)
    }
}
impl Display for SourceLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at {}/{}", self.func, self.module, self.file)
    }
}
