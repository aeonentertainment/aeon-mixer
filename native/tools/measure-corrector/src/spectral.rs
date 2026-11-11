//! How far one sound's spectrum sits from another's on voiced frames, two ways.
//!
//! Log-spectral distance reads the whole short-time spectrum, harmonics and the valleys between
//! them, so it sees pitch as well as timbre: a few cents off moves every upper harmonic across a
//! bin. Mel cepstral distortion reads only the envelope, the first 12 mel cepstra from 24 bands, so it is close
//! to blind to pitch and sees what a shifter does to the vowel. Kubichek (1993) defined MCD for
//! speech coding, and voice conversion and singing synthesis report it; it correlates only weakly
//! with listeners there. These MCD numbers come from MFCCs, not SPTK's warped mel cepstra, so they
//! do not compare with papers' numbers, only with each other.

use std::f64::consts::{LN_10, PI};
use std::sync::Arc;

use engine::block::RATE;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};

use crate::take_delay::TakeDelay;

const FRAME: usize = 2048;
const HOP: usize = 480;
/// The band both read: below 60 Hz is hum and above 12 kHz is breath and codec cut-off.
const BAND_HZ: (f64, f64) = (60.0, 12_000.0);
/// Bins this far under the reference frame's loudest are floored there, so silence between
/// harmonics in a studio vocal does not decide the number.
const FLOOR_DB: f64 = 60.0;
const MEL_BANDS: usize = 24;
const CEPSTRA: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpectralDistance {
    pub frames: usize,
    pub lsd_db: f64,
    pub mcd_db: f64,
}

/// Reads frames of 2048 under a Hann window every 10 ms.
pub struct SpectralRuler {
    fft: Arc<dyn Fft<f64>>,
    window: Vec<f64>,
    /// Each mel band's bins and weights.
    bands: Vec<Vec<(usize, f64)>>,
    bins: (usize, usize),
}

impl SpectralRuler {
    pub fn new() -> Self {
        let bin_hz = f64::from(RATE) / FRAME as f64;
        let bins = ((BAND_HZ.0 / bin_hz).ceil() as usize, (BAND_HZ.1 / bin_hz).floor() as usize);
        let mel = |hz: f64| 2595.0 * (1.0 + hz / 700.0).log10();
        let hz = |mel: f64| 700.0 * (10f64.powf(mel / 2595.0) - 1.0);
        let (low, high) = (mel(BAND_HZ.0), mel(BAND_HZ.1));
        let edges: Vec<f64> = (0..MEL_BANDS + 2)
            .map(|i| hz(low + (high - low) * i as f64 / (MEL_BANDS + 1) as f64))
            .collect();
        let bands = edges
            .windows(3)
            .map(|edge| {
                (bins.0..=bins.1)
                    .filter_map(|bin| {
                        let f = bin as f64 * bin_hz;
                        let weight = if f < edge[1] {
                            (f - edge[0]) / (edge[1] - edge[0])
                        } else {
                            (edge[2] - f) / (edge[2] - edge[1])
                        };
                        (weight > 0.0).then_some((bin, weight))
                    })
                    .collect()
            })
            .collect();
        Self {
            fft: FftPlanner::new().plan_fft_forward(FRAME),
            window: (0..FRAME)
                .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / FRAME as f64).cos())
                .collect(),
            bands,
            bins,
        }
    }

    /// `output` against `reference` on every frame whose centre `voiced` says is sung. The output
    /// frame is read `delay` later than the reference's.
    pub fn distance(
        &self,
        reference: &[f32],
        output: &[f32],
        delay: &TakeDelay,
        voiced: impl Fn(f64) -> bool,
    ) -> SpectralDistance {
        let (mut frames, mut lsd, mut mcd) = (0usize, 0.0, 0.0);
        let half = FRAME / 2;
        for centre in (half..reference.len().saturating_sub(half)).step_by(HOP) {
            if !voiced(centre as f64 * 1000.0 / f64::from(RATE)) {
                continue;
            }
            let shifted = centre + delay.frames_at(centre);
            let Some(theirs) = output.get(shifted - half..shifted + half) else { continue };
            let ours = self.power(&reference[centre - half..centre + half]);
            let theirs = self.power(theirs);
            let floor = ours.iter().copied().fold(0.0, f64::max) * 10f64.powf(-FLOOR_DB / 10.0);
            if floor <= 0.0 {
                continue;
            }
            let (ours, theirs) = (floored(&ours, floor), floored(&theirs, floor));
            lsd += self.log_spectral(&ours, &theirs);
            mcd += self.cepstral(&ours, &theirs);
            frames += 1;
        }
        let per = frames.max(1) as f64;
        SpectralDistance { frames, lsd_db: lsd / per, mcd_db: mcd / per }
    }

    fn power(&self, frame: &[f32]) -> Vec<f64> {
        let mut buffer: Vec<Complex<f64>> = frame
            .iter()
            .zip(&self.window)
            .map(|(sample, w)| Complex::new(f64::from(*sample) * w, 0.0))
            .collect();
        self.fft.process(&mut buffer);
        buffer[..=self.bins.1].iter().map(Complex::norm_sqr).collect()
    }

    fn log_spectral(&self, ours: &[f64], theirs: &[f64]) -> f64 {
        let (low, high) = self.bins;
        let sum: f64 =
            (low..=high).map(|bin| (10.0 * (ours[bin] / theirs[bin]).log10()).powi(2)).sum();
        (sum / (high - low + 1) as f64).sqrt()
    }

    fn cepstra(&self, power: &[f64]) -> [f64; CEPSTRA] {
        let logs: Vec<f64> = self
            .bands
            .iter()
            .map(|band| band.iter().map(|(bin, weight)| power[*bin] * weight).sum::<f64>().ln())
            .collect();
        let scale = (2.0 / MEL_BANDS as f64).sqrt();
        std::array::from_fn(|d| {
            let d = d + 1;
            scale
                * logs
                    .iter()
                    .enumerate()
                    .map(|(m, log)| {
                        log * (PI * d as f64 * (m as f64 + 0.5) / MEL_BANDS as f64).cos()
                    })
                    .sum::<f64>()
        })
    }

    fn cepstral(&self, ours: &[f64], theirs: &[f64]) -> f64 {
        let (ours, theirs) = (self.cepstra(ours), self.cepstra(theirs));
        let sum: f64 = ours.iter().zip(&theirs).map(|(a, b)| (a - b).powi(2)).sum();
        10.0 / LN_10 * (2.0 * sum).sqrt()
    }
}

fn floored(power: &[f64], floor: f64) -> Vec<f64> {
    power.iter().map(|p| p.max(floor)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::harmonic_voice;

    fn between(a: &[f32], b: &[f32]) -> SpectralDistance {
        SpectralRuler::new().distance(a, b, &TakeDelay::NONE, |_| true)
    }

    // What each charges for pitch alone, on a harmonic vowel at 220 Hz against itself 10 cents
    // sharp: LSD about 3 dB, MCD about 1, each within a quarter. A same signal reads 0. Real voices
    // read less, since breath fills the valleys; decision 0015 has those numbers
    #[test]
    fn lsd_charges_pitch_and_mcd_charges_a_third_of_it() {
        let voice = harmonic_voice(48_000, |_| 220.0);
        assert_eq!(between(&voice, &voice).lsd_db, 0.0);
        let sharp = harmonic_voice(48_000, |_| 220.0 * 2f64.powf(10.0 / 1200.0));
        let read = between(&voice, &sharp);
        assert!((read.lsd_db - 3.0).abs() < 0.75, "{read:?}");
        assert!((read.mcd_db - 1.1).abs() < 0.3, "{read:?}");
    }

    // A frame read later by the take's delay lines a delayed copy up with its source
    #[test]
    fn the_output_is_read_as_late_as_the_take_says() {
        let voice = harmonic_voice(48_000, |ms| 200.0 + 30.0 * (ms / 80.0).sin());
        let mut late = vec![0.0; 240];
        late.extend_from_slice(&voice);
        let delays = vec![240; 400];
        let lined_up =
            SpectralRuler::new().distance(&voice, &late, &TakeDelay::new(&delays), |_| true);
        assert!(lined_up.lsd_db < 1e-9, "{lined_up:?}");
        assert!(between(&voice, &late).lsd_db > 1.0);
    }
}
