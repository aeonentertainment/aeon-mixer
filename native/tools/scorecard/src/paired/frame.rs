//! A take cut into the frames the paired rows read: 4096 samples (85 ms) every 2048,
//! Hann-windowed, each as its power in total, either side of 5 kHz and in each third octave
//! from 100 Hz to 12.5 kHz (docs/design.md 8.9).

use std::f64::consts::TAU;

use engine::block::RATE;
use realfft::RealFftPlanner;

const LENGTH: usize = 4096;
const HOP: usize = 2048;
/// Where an s sits: unvoiced frames hold more power over it than under.
pub const SIBILANT_HZ: f64 = 5000.0;
/// The third octaves from 100 Hz, `1000 * 10^(k/10)` for `k` from -10 to 11.
pub const BANDS: usize = 22;

/// Each power is a mean square, so a sine of amplitude `a` reads `a² / 2` whatever the window.
pub struct Frame {
    pub power: f64,
    pub under: f64,
    pub over: f64,
    pub bands: [f64; BANDS],
}

impl Frame {
    /// Every whole frame of `samples`.
    pub fn all(samples: &[f32]) -> Vec<Self> {
        let fft = RealFftPlanner::<f64>::new().plan_fft_forward(LENGTH);
        let window: Vec<f64> =
            (0..LENGTH).map(|n| 0.5 - 0.5 * (TAU * n as f64 / LENGTH as f64).cos()).collect();
        let energy: f64 = window.iter().map(|w| w * w).sum();
        let bin_hz = f64::from(RATE) / LENGTH as f64;
        let band_of: Vec<Option<usize>> =
            (0..=LENGTH / 2).map(|k| band(k as f64 * bin_hz)).collect();
        let (mut input, mut spectrum) = (fft.make_input_vec(), fft.make_output_vec());
        let starts = (0..samples.len().saturating_sub(LENGTH - 1)).step_by(HOP);
        starts
            .map(|start| {
                for ((x, sample), w) in input.iter_mut().zip(&samples[start..]).zip(&window) {
                    *x = f64::from(*sample) * w;
                }
                fft.process(&mut input, &mut spectrum).expect("buffers from the plan");
                let mut frame = Self { power: 0.0, under: 0.0, over: 0.0, bands: [0.0; BANDS] };
                for (k, bin) in spectrum.iter().enumerate() {
                    // Parseval over a one-sided spectrum: every bin but the two ends stands for two
                    let twice = if k == 0 || k == LENGTH / 2 { 1.0 } else { 2.0 };
                    let power = twice * bin.norm_sqr() / (LENGTH as f64 * energy);
                    frame.power += power;
                    if (k as f64 * bin_hz) < SIBILANT_HZ {
                        frame.under += power;
                    } else {
                        frame.over += power;
                    }
                    if let Some(band) = band_of[k] {
                        frame.bands[band] += power;
                    }
                }
                frame
            })
            .collect()
    }
}

/// The third octave `hz` falls in, from its lower edge up to its upper.
fn band(hz: f64) -> Option<usize> {
    let k = (10.0 * (hz / 1000.0).log10() + 0.5).floor() + 10.0;
    (0.0..BANDS as f64).contains(&k).then_some(k as usize)
}

/// A power in dB, with digital silence at -120 rather than minus infinity.
pub fn db(power: f64) -> f64 {
    10.0 * power.max(1e-12).log10()
}
