//! A held note's vibrato rate: the peak of its contour's spectrum between 4 and 8 Hz, read finely.
//! Singers' rates sit at 4.6 to 5.7 Hz (Prame 1994) and depth at about 30 to 120 cents
//! peak-to-peak; the band and its RMS are `expression`'s.

use std::f64::consts::TAU;

use crate::expression::VIBRATO_BAND;

/// How finely the rate is read.
const STEP_HZ: f64 = 0.02;

/// The rate, in Hz, of the strongest wobble in `contour`, sampled at `rate` Hz.
pub fn rate_hz(contour: &[f64], rate: f64) -> f64 {
    let n = contour.len() as f64;
    let mean = contour.iter().sum::<f64>() / n.max(1.0);
    let power = |hz: f64| {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, value) in contour.iter().enumerate() {
            let window = 0.5 - 0.5 * (TAU * (i as f64 + 0.5) / n).cos();
            let angle = TAU * hz * i as f64 / rate;
            re += (value - mean) * window * angle.cos();
            im -= (value - mean) * window * angle.sin();
        }
        re * re + im * im
    };
    let (low, high) = VIBRATO_BAND;
    let steps = ((high - low) / STEP_HZ) as usize;
    (0..=steps)
        .map(|step| low + step as f64 * STEP_HZ)
        .max_by(|a, b| power(*a).total_cmp(&power(*b)))
        .unwrap_or(low)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Within 0.1 Hz on the shortest note the rows read, half a second, under a slow drift
    #[test]
    fn a_known_rate_reads_as_itself() {
        for hz in [4.5, 5.5, 6.3, 7.5] {
            let contour: Vec<f64> = (0..60)
                .map(|i| {
                    let t = i as f64 / 100.0;
                    40.0 * (TAU * hz * t).sin() + 15.0 * t
                })
                .collect();
            let read = rate_hz(&contour, 100.0);
            assert!((read - hz).abs() < 0.1, "{hz} Hz read {read}");
        }
    }
}
