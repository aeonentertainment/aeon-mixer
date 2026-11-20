//! What the kernels' tests share, from Aeon Karaoke's `TestSupport.h`: ways to read the pitch of
//! a stretch of output. The voice they sing is `crate::fixtures::harmonic_voice`.

use crate::yin::YinDetector;

pub const RATE: u32 = 48_000;

pub fn cents_between(hz: f64, reference: f64) -> f64 {
    1200.0 * (hz / reference).log2()
}

/// A YIN reading of `signal` every 10 ms from `from` to `to` seconds, in cents from `reference` Hz.
/// The 1024-sample window keeps 98 percent of a 5.5 Hz vibrato's depth; unvoiced frames are left out.
pub fn cents_track(signal: &[f32], from: f64, to: f64, reference: f64) -> Vec<f64> {
    let rate = f64::from(RATE);
    let mut yin = YinDetector::new(RATE, 1024);
    ((from * rate) as usize + 1024..=(to * rate) as usize)
        .step_by(RATE as usize / 100)
        .map(|end| f64::from(yin.analyze(&signal[end - 1024..end]).hz))
        .filter(|hz| *hz > 0.0)
        .map(|hz| cents_between(hz, reference))
        .collect()
}

/// The median YIN reading over every 10 ms frame between `from` and `to` seconds of `signal`.
pub fn median_pitch(signal: &[f32], from: f64, to: f64) -> f64 {
    let rate = f64::from(RATE);
    let mut yin = YinDetector::new(RATE, 2048);
    let mut readings: Vec<f64> = ((from * rate) as usize + 2048..=(to * rate) as usize)
        .step_by(RATE as usize / 100)
        .map(|end| f64::from(yin.analyze(&signal[end - 2048..end]).hz))
        .filter(|hz| *hz > 0.0)
        .collect();
    if readings.is_empty() {
        return 0.0;
    }
    let middle = readings.len() / 2;
    *readings.select_nth_unstable_by(middle, f64::total_cmp).1
}
