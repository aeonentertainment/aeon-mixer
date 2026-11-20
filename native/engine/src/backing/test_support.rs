//! What the backing tests share: a way to read a tone's pitch, and its level. The tone they play is
//! `kernels::fixtures::harmonic_voice`.

use std::f32::consts::TAU;

use kernels::YinDetector;

use crate::block::RATE;

pub const SECOND: usize = RATE as usize;

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

pub fn sine(hz: f32, rate: u32, frames: usize) -> Vec<f32> {
    (0..frames).map(|i| 0.5 * (TAU * hz * i as f32 / rate as f32).sin()).collect()
}

/// A 16-bit PCM WAV of `channels`, which must be one length.
pub fn wav(rate: u32, channels: &[Vec<f32>]) -> Vec<u8> {
    let (count, frames) = (channels.len() as u16, channels[0].len() as u32);
    let data = frames * u32::from(count) * 2;
    let mut bytes = Vec::new();
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(count.to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * u32::from(count) * 2).to_le_bytes());
    bytes.extend((count * 2).to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    for frame in 0..frames as usize {
        for channel in channels {
            bytes.extend(((channel[frame] * f32::from(i16::MAX)) as i16).to_le_bytes());
        }
    }
    bytes
}
