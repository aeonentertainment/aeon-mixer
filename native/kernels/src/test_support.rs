//! What the kernels' tests share, from Aeon Karaoke's `TestSupport.h`: a voice-shaped tone and
//! a way to read the pitch of a stretch of output.

use std::f64::consts::PI;

use crate::yin::YinDetector;

pub const RATE: u32 = 48_000;

/// Twelve harmonics falling as 1/h at `hz`: the buzz of a sung vowel, which is what a detector
/// and a shifter work on.
pub fn harmonic_tone(hz: f64, seconds: f64) -> Vec<f32> {
    let rate = f64::from(RATE);
    (0..(seconds * rate) as usize)
        .map(|i| {
            let t = i as f64 / rate;
            let sample: f64 =
                (1..=12).map(|h| (2.0 * PI * hz * h as f64 * t).sin() / h as f64).sum();
            (0.25 * sample) as f32
        })
        .collect()
}

/// A voice whose pitch follows `hz_at` a sample at a time, with no break in phase, and whose
/// harmonics are `levels`, the first being the fundamental.
pub fn sung(frames: usize, levels: &[f64], hz_at: impl Fn(usize) -> f64) -> Vec<f32> {
    let mut phase = 0.0f64;
    (0..frames)
        .map(|i| {
            phase = (phase + hz_at(i) / f64::from(RATE)) % 1.0;
            let sample: f64 = levels
                .iter()
                .enumerate()
                .map(|(h, level)| (2.0 * PI * (h + 1) as f64 * phase).sin() * level)
                .sum();
            (0.25 * sample) as f32
        })
        .collect()
}

/// The levels of `harmonic_tone`: twelve harmonics falling as 1/h.
pub fn buzz() -> Vec<f64> {
    (1..=12).map(|h| 1.0 / f64::from(h)).collect()
}

pub fn cents_between(hz: f64, reference: f64) -> f64 {
    1200.0 * (hz / reference).log2()
}

pub fn midi_hz(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
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
