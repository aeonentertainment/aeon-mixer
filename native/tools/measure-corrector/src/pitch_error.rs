//! How far one contour sits from another, moment by moment: the answer key's pitch rows. Molina
//! Villota et al. (DAFx 2023) judge a corrector the same way, by the error between its output's
//! f0 and the curve it should have made, region by region.

use crate::contour::Point;
use crate::music::cents;

/// One moment both contours call voiced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Moment {
    pub ms: f64,
    /// The output's pitch from the reference's, signed.
    pub cents: f64,
}

/// At every voiced reading of `output`, the cents from `reference` read in a line between its two
/// nearest readings, both voiced. Readings are on the same clock.
pub fn moments(reference: &[Point], output: &[Point]) -> Vec<Moment> {
    output
        .iter()
        .filter(|point| point.voiced())
        .filter_map(|point| {
            let after = reference.partition_point(|r| r.ms < point.ms);
            let (before, after) = (reference.get(after.checked_sub(1)?)?, reference.get(after)?);
            if !(before.voiced() && after.voiced()) {
                return None;
            }
            let through = (point.ms - before.ms) / (after.ms - before.ms).max(1e-9);
            let expected = f64::from(before.hz).ln()
                + (f64::from(after.hz).ln() - f64::from(before.hz).ln()) * through;
            Some(Moment { ms: point.ms, cents: cents(f64::from(point.hz), expected.exp()) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_moment_reads_between_the_references_readings() {
        let point = |ms, hz| Point { ms, hz, confidence: 1.0 };
        let reference = [point(0.0, 220.0), point(10.0, 440.0), point(20.0, 0.0)];
        let found = moments(&reference, &[point(5.0, 311.127), point(15.0, 440.0)]);
        // Halfway between 220 and 440 Hz on a log scale is 311.127; past the last voiced is nothing
        assert_eq!(found.len(), 1);
        assert!(found[0].cents.abs() < 0.01, "{found:?}");
    }
}
