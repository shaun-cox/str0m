use super::{LatencyEstimator, Micros};
use std::fmt::Display;
use tracing::warn;

pub struct CallSiteStats {
    max: Option<Micros>,
    avg: LatencyEstimator,
}

impl CallSiteStats {
    pub const fn new() -> Self {
        Self {
            max: None,
            avg: LatencyEstimator::new(),
        }
    }

    pub fn maybe_reveal(
        &mut self,
        SourceLocation { func, module, file }: SourceLocation,
        latency: Micros,
    ) {
        let prior_max = self.max.unwrap_or(Micros::ZERO).as_value();
        let prior_max_exceeded = latency > Micros(prior_max);

        if latency > Micros(500) && self.avg.has_sample() {
            if prior_max_exceeded {
                warn!(
                    latency = latency.as_value(),
                    avg = self.avg.mean().as_value(),
                    dev = self.avg.deviation().as_value(),
                    prior_max,
                    func,
                    module,
                    %file,
                    "Execution (µs) exceeded prior max",
                );
            } else if latency > Micros(5000) {
                warn!(
                    latency = latency.as_value(),
                    avg = self.avg.mean().as_value(),
                    dev = self.avg.deviation().as_value(),
                    prior_max,
                    func,
                    module,
                    %file,
                    "Execution (µs) exceeded threshold",
                );
            } else if self.avg.mean() > Micros(50) && latency > self.avg.mean() * 20 {
                warn!(
                    latency = latency.as_value(),
                    avg = self.avg.mean().as_value(),
                    dev = self.avg.deviation().as_value(),
                    prior_max,
                    func,
                    module,
                    %file,
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

impl Default for CallSiteStats {
    fn default() -> Self {
        Self::new()
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
