//! What the backing tests share: a voice-shaped tone, a way to read its pitch, and its level.

use std::f64::consts::PI;

use kernels::YinDetector;

use crate::block::RATE;

pub const SECOND: usize = RATE as usize;

/// Twelve harmonics falling as 1/h at `hz`: a tone the stretcher and the detector both treat as
/// music, peaking near 0.5.
pub fn harmonic_tone(hz: f64, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let sample: f64 =
                (1..=12).map(|h| (2.0 * PI * hz * h as f64 * t).sin() / h as f64).sum();
            (0.25 * sample) as f32
        })
        .collect()
}

/// The median YIN reading over every 10 ms window of `signal`.
pub fn median_pitch(signal: &[f32]) -> f64 {
    let window = 2048;
    let mut yin = YinDetector::new(RATE, window);
    let mut readings: Vec<f64> = (window..=signal.len())
        .step_by(SECOND / 100)
        .map(|end| f64::from(yin.analyze(&signal[end - window..end]).hz))
        .filter(|hz| *hz > 0.0)
        .collect();
    assert!(!readings.is_empty(), "no pitch in the signal");
    let middle = readings.len() / 2;
    *readings.select_nth_unstable_by(middle, f64::total_cmp).1
}

pub fn cents_between(hz: f64, reference: f64) -> f64 {
    1200.0 * (hz / reference).log2()
}

/// The RMS of the 5 ms that start at each frame, for finding where a sound starts and stops.
pub fn rms_at(signal: &[f32], at: usize) -> f32 {
    let window = &signal[at..(at + SECOND / 200).min(signal.len())];
    (window.iter().map(|s| s * s).sum::<f32>() / window.len() as f32).sqrt()
}
