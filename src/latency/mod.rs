use std::time::Duration;

mod estimator;
use estimator::LatencyEstimator;

pub(super) mod revealer;

macro_rules! reveal {
    ($func:literal, $expression:expr) => {{
        let source_location = $crate::latency::revealer::SourceLocation {
            func: $func,
            module: module_path!(),
            file: $crate::latency::revealer::FileLine(file!(), line!()),
        };
        static STATS: std::sync::LazyLock<
            std::sync::Mutex<$crate::latency::revealer::CallSiteStats>,
        > = std::sync::LazyLock::new(|| {
            std::sync::Mutex::new($crate::latency::revealer::CallSiteStats::new())
        });
        let mut stats = STATS.lock().unwrap();
        let start_time = std::time::Instant::now();
        let result = $expression;
        let latency: $crate::latency::Micros = start_time.elapsed().into();
        stats.maybe_reveal(source_location, latency);
        result
    }};
}
pub(super) use reveal;

/// A number of microseconds.
///
/// N.B. The largest number of microseconds that can be represented is just
/// over 71 minutes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, PartialOrd, Ord)]
pub struct Micros(u32);

impl Micros {
    pub const ZERO: Self = Self(0);

    pub const fn as_value(&self) -> u32 {
        self.0
    }

    pub const fn from_micros(value: u32) -> Self {
        Self(value)
    }

    pub const fn from_duration(value: Duration) -> Self {
        let micros = value.as_micros();
        assert!(micros <= u32::MAX as u128);
        Self(micros as u32)
    }

    pub const fn to_duration(&self) -> Duration {
        Duration::from_micros(self.0 as u64)
    }

    pub const fn saturating_add(self, rhs: Self) -> Self {
        Self(self.0.saturating_add(rhs.0))
    }

    pub const fn saturating_sub(self, rhs: Self) -> Self {
        Self(self.0.saturating_sub(rhs.0))
    }

    pub const fn shl(self, rhs: u8) -> Self {
        Self(self.0 << rhs)
    }

    pub const fn shr(self, rhs: u8) -> Self {
        Self(self.0 >> rhs)
    }
}

impl From<Duration> for Micros {
    fn from(value: Duration) -> Self {
        Self::from_duration(value)
    }
}

impl From<Micros> for Duration {
    fn from(value: Micros) -> Self {
        value.to_duration()
    }
}

impl std::ops::Div<u32> for Micros {
    type Output = Self;

    fn div(self, rhs: u32) -> Self::Output {
        Self(self.0 / rhs)
    }
}

impl std::ops::Mul<u32> for Micros {
    type Output = Self;

    fn mul(self, rhs: u32) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl std::ops::Sub for Micros {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}
