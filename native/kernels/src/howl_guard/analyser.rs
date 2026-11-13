use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};

/// 1024 frames, 21 ms: 47 Hz bins, fine enough to tell a howl from a sung partial's neighbours.
pub const LENGTH: usize = 1024;
const BINS: usize = LENGTH / 2 + 1;
/// A howl lives between the low voice and the top of the speakers' useful range.
const LOWEST_HZ: f32 = 150.0;
const HIGHEST_HZ: f32 = 8000.0;
/// The features of van Waterschoot and Moonen's survey of howl detection. A peak must stand this
/// far above the bins three and four away (PNPR): one tone, not a band.
const NARROW_DB: f32 = 10.0;
/// This far above its own second harmonic (PHPR): a sung partial has another partial at double
/// its frequency, 6 dB down for a buzz falling as 1/h, and a howl has none until it clips, and
/// then only odd ones.
const NO_HARMONIC_DB: f32 = 12.0;
/// And this far above the band's mean (PAPR).
const OVER_MEAN_DB: f32 = 10.0;
/// Below this a peak is the room's hiss, whatever its shape.
const QUIETEST_DB: f32 = -60.0;

/// The strongest tone in one analysis, when it is a howl's shape.
#[derive(Clone, Copy, Debug)]
pub struct Peak {
    pub bin: usize,
    /// Where the tone sits between bins, by a parabola through the peak's log power and its
    /// neighbours': a tone's frequency to well within a bin.
    pub at: f32,
    /// The level a sine at this bin would have, in dBFS.
    pub db: f32,
}

/// Looks at the latest `LENGTH` frames through a Hann window and names the tone that stands
/// alone, the spectral half of a notch-based feedback suppressor's howl detector.
pub struct Analyser {
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    time: Vec<f32>,
    spectrum: Vec<Complex32>,
    power: Vec<f32>,
    scratch: Vec<Complex32>,
    lowest: usize,
    highest: usize,
}

impl Analyser {
    pub fn new(rate: u32) -> Self {
        let fft = RealFftPlanner::new().plan_fft_forward(LENGTH);
        let scratch = fft.make_scratch_vec();
        let bin_hz = rate as f32 / LENGTH as f32;
        Self {
            window: (0..LENGTH)
                .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / LENGTH as f32).cos())
                .collect(),
            time: vec![0.0; LENGTH],
            spectrum: fft.make_output_vec(),
            power: vec![0.0; BINS],
            scratch,
            fft,
            lowest: (LOWEST_HZ / bin_hz).ceil() as usize,
            highest: ((HIGHEST_HZ / bin_hz) as usize).min(BINS - 5),
        }
    }

    /// `recent` is the latest `LENGTH` frames, oldest first. The strongest peak with a howl's
    /// shape, which a singer's louder partials beside it do not hide.
    pub fn analyse(&mut self, recent: impl Iterator<Item = f32>) -> Option<Peak> {
        for ((time, sample), window) in self.time.iter_mut().zip(recent).zip(&self.window) {
            *time = sample * window;
        }
        let _ =
            self.fft.process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.scratch);
        for (power, bin) in self.power.iter_mut().zip(&self.spectrum) {
            *power = bin.norm_sqr();
        }
        let band = &self.power[self.lowest..=self.highest];
        let mean = band.iter().sum::<f32>() / band.len() as f32;
        let p = &self.power;
        let under = |peak: f32, db: f32| peak * 10f32.powf(-db / 10.0);
        let howl_shaped = |k: usize| {
            let peak = p[k];
            let local = peak > p[k - 1] && peak >= p[k + 1] && peak >= p[k - 2] && peak >= p[k + 2];
            local
                && peak > mean * 10f32.powf(OVER_MEAN_DB / 10.0)
                && [k - 4, k - 3, k + 3, k + 4].iter().all(|&n| p[n] <= under(peak, NARROW_DB))
                && (2 * k + 1 >= p.len()
                    || p[2 * k - 1..=2 * k + 1].iter().all(|&h| h <= under(peak, NO_HARMONIC_DB)))
        };
        let bin = (self.lowest..=self.highest)
            .filter(|&k| howl_shaped(k))
            .max_by(|a, b| p[*a].total_cmp(&p[*b]))?;
        // A sine of amplitude A peaks at A * LENGTH / 4 through a Hann window
        let db = 10.0 * p[bin].max(1e-30).log10() + 20.0 * (4.0 / LENGTH as f32).log10();
        let level = |k: usize| p[k].max(1e-30).ln();
        let (before, peak, after) = (level(bin - 1), level(bin), level(bin + 1));
        let curvature = before - 2.0 * peak + after;
        let offset = if curvature < 0.0 {
            (0.5 * (before - after) / curvature).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        (db >= QUIETEST_DB).then_some(Peak { bin, at: bin as f32 + offset, db })
    }
}
