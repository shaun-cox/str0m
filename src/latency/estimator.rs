//! An estimator for tracking mean latency given a sequence of measurements.
//!
//! The core algorithm is taken from the [seminal work] in TCP by Van Jacobson
//! and also referenced by [RFC 6298]. More information about [exponential smoothing]
//! in general can be found on Wikipedia.
//!
//! [seminal work]: <https://ee.lbl.gov/papers/congavoid.pdf>
//! [RFC 6298]: <https://www.rfc-editor.org/rfc/rfc6298#section-2>
//! [exponential smoothing]: <https://en.wikipedia.org/wiki/Exponential_smoothing>

use super::Micros;

/// Tracks the mean latency and its deviation given an online sequence of measurements.
///
/// This implements the standard algorithm defined in [RFC 6298].
/// [RFC 6298]: <https://www.rfc-editor.org/rfc/rfc6298#section-2>
pub struct LatencyEstimator {
    inner: Option<Inner>,
}

struct Inner {
    mean: ScaledUnsignedEstimator<3, Micros>,
    deviation: ScaledUnsignedEstimator<2, Micros>,
}

impl LatencyEstimator {
    /// Creates a new estimator.
    pub const fn new() -> Self {
        Self { inner: None }
    }

    pub const fn has_sample(&self) -> bool {
        self.inner.is_some()
    }

    /// Update the estimate based on a new measurement.
    pub fn record(&mut self, value: Micros) {
        match &mut self.inner {
            None => {
                self.inner = Some(Inner {
                    mean: ScaledUnsignedEstimator::new(value),
                    deviation: ScaledUnsignedEstimator::new(value / 2),
                });
            }
            Some(inner) => {
                let abs_error = inner.mean.record(value);
                let _ = inner.deviation.record(abs_error);
            }
        }
    }

    /// Return the estimate of the mean of all prior recorded measurements.
    ///
    /// # Panics
    /// If `record` has not been called at least once.
    pub fn mean(&self) -> Micros {
        self.assume_recorded().mean.smoothed()
    }

    /// Return the estimate of the mean deviation of all prior recorded measurements.
    ///
    /// Note, mean deviation is more conservative estimate of variation than standard
    /// deviation and is easier to compute.
    ///
    /// # Panics
    /// If `record` has not been called at least once.
    pub fn deviation(&self) -> Micros {
        self.assume_recorded().deviation.smoothed()
    }

    fn assume_recorded(&self) -> &Inner {
        self.inner
            .as_ref()
            .expect("at least one measurement needs to be recorded first")
    }
}

impl Default for LatencyEstimator {
    fn default() -> Self {
        Self::new()
    }
}

/// A generic estimator employing [exponential smoothing] and using scaled,
/// unsigned integer arithmetic.
///
/// Generic parameter S is the number of bits to shift for the smoothing factor,
/// or alpha.
/// Typical values are 3, for an alpha of .125, or 2, for an alpha of .25.
///
/// [exponential smoothing]: <https://en.wikipedia.org/wiki/Exponential_smoothing>
struct ScaledUnsignedEstimator<const S: u8, T> {
    scaled: T,
}

impl<const S: u8> ScaledUnsignedEstimator<S, Micros> {
    /// Create a new estimator from an initial measurement.
    const fn new(value: Micros) -> Self {
        Self {
            scaled: value.shl(S),
        }
    }

    /// Return the smoothed estimate of all prior recorded measurements.
    const fn smoothed(&self) -> Micros {
        self.scaled.shr(S)
    }

    /// Update the estimate based on a new measurement.
    fn record(&mut self, value: Micros) -> Micros {
        let mean = self.smoothed();
        match value.cmp(&mean) {
            std::cmp::Ordering::Less => {
                let abs_error = mean - value;
                self.scaled = self.scaled.saturating_sub(abs_error);
                abs_error
            }
            std::cmp::Ordering::Equal => Micros(0),
            std::cmp::Ordering::Greater => {
                let abs_error = value - mean;
                self.scaled = self.scaled.saturating_add(abs_error);
                abs_error
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scaled_unsigned_estimator_works() {
        let first_input = Micros(2800);
        let inputs: [u32; 10] = [2500, 2000, 3500, 4000, 2800, 3300, 3500, 3026, 3000, 4000];
        let abs_errors: [u32; 10] = [300, 762, 833, 1229, 125, 391, 542, 0, 26, 978];
        let outputs: [u32; 10] = [2762, 2667, 2771, 2925, 2909, 2958, 3026, 3026, 3022, 3145];

        let mut estimator = ScaledUnsignedEstimator::<3, Micros>::new(first_input);
        assert_eq!(
            estimator.smoothed(),
            first_input,
            "new estimator produced wrong first estimate"
        );
        for (input, (abs_error, smoothed)) in
            inputs.iter().zip(abs_errors.iter().zip(outputs.iter()))
        {
            assert_eq!(
                estimator.record(Micros(*input)),
                Micros(*abs_error),
                "incorrect absolute error"
            );
            assert_eq!(
                estimator.smoothed(),
                Micros(*smoothed),
                "incorrect smoothed estimate"
            );
        }
    }

    #[test]
    fn latency_estimator_works() {
        let inputs: [u32; 15] = [
            6309, 6225, 6469, 5908, 6017, 6169, 6283, 6050, 5814, 6340, 6210, 6228, 6247, 10056,
            4375,
        ];
        let means: [u32; 15] = [
            6309, 6298, 6319, 6268, 6237, 6228, 6235, 6212, 6162, 6184, 6188, 6193, 6199, 6682,
            6393,
        ];
        let deviations: [u32; 15] = [
            3154, 2386, 1832, 1477, 1171, 895, 685, 560, 519, 434, 332, 259, 208, 1120, 1417,
        ];
        let mut estimator = LatencyEstimator::new();
        for (input, (mean, deviation)) in inputs.iter().zip(means.iter().zip(deviations.iter())) {
            estimator.record(Micros(*input));
            assert_eq!(estimator.mean(), Micros(*mean), "incorrect mean");
            assert_eq!(
                estimator.deviation(),
                Micros(*deviation),
                "incorrect deviations"
            );
        }
    }
}
