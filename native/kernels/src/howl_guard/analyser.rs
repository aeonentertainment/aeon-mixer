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
/// And this far above the band's mean (PAPR).
const OVER_MEAN_DB: f32 = 10.0;
/// A peak this close to a tone's second, third or fourth harmonic, or to half or a third of its
/// frequency, makes it part of a voice's harmonic series (PHPR, both ways). A sung partial has
/// others at multiples of its frequency, the second 6 dB down for a buzz falling as 1/h, and a
/// voice's second or third partial has its fundamental under it. A howl has neither until it
/// clips. Looking both ways means a room, or an echo canceller taking out an echo of the singer,
/// can sink one partial of a voice without making the rest look alone.
const NO_HARMONIC_DB: f32 = 12.0;
/// How far, in bins, a peak may sit from half or a third of a tone's frequency and be its
/// fundamental. A voice's partials sit at exact multiples, within the error of each one's place
/// between bins; a low singer's partials are dense enough that a whole bin would find one under a
/// howl as often as not.
const FUNDAMENTAL_BINS: f32 = 0.15;
/// Below this a peak is the room's hiss, whatever its shape.
const QUIETEST_DB: f32 = -60.0;

/// A narrow tone in one analysis.
#[derive(Clone, Copy, Debug)]
pub struct Peak {
    pub bin: usize,
    /// Where the tone sits between bins, by a parabola through the peak's log power and its
    /// neighbours': a tone's frequency to well within a bin.
    pub at: f32,
    /// The level a sine at this bin would have, in dBFS.
    pub db: f32,
    /// Whether it stood alone in this analysis: no harmonic above it and no fundamental under it.
    pub alone: bool,
}

/// Looks at the latest `LENGTH` frames through a Hann window and names the narrow tones in it, the
/// spectral half of a notch-based feedback suppressor's howl detector.
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

    /// `recent` is the latest `LENGTH` frames, oldest first. Writes the strongest narrow peaks into
    /// `peaks`, loudest first, as many as fit, and returns how many it wrote. A singer's louder
    /// partials do not hide a quieter tone from the guard as long as it is among them.
    pub fn analyse(&mut self, recent: impl Iterator<Item = f32>, peaks: &mut [Peak]) -> usize {
        for ((time, sample), window) in self.time.iter_mut().zip(recent).zip(&self.window) {
            *time = sample * window;
        }
        let _ =
            self.fft.process_with_scratch(&mut self.time, &mut self.spectrum, &mut self.scratch);
        for (power, bin) in self.power.iter_mut().zip(&self.spectrum) {
            *power = bin.norm_sqr();
        }
        let band = &self.power[self.lowest..=self.highest];
        let floor = band.iter().sum::<f32>() / band.len() as f32 * 10f32.powf(OVER_MEAN_DB / 10.0);
        let mut found = 0;
        for k in self.lowest..=self.highest {
            if !self.narrow(k, floor) {
                continue;
            }
            let Some(peak) = self.peak(k) else { continue };
            let at = peaks[..found].iter().position(|p| p.db < peak.db).unwrap_or(found);
            if at == peaks.len() {
                continue;
            }
            // Loudest first: the quietest falls off the end when they are full
            found = (found + 1).min(peaks.len());
            peaks[at..found].rotate_right(1);
            peaks[at] = peak;
        }
        found
    }

    /// Whether bin `k` is a local peak over `floor` and over the bins three and four away.
    fn narrow(&self, k: usize, floor: f32) -> bool {
        let p = &self.power;
        let peak = p[k];
        let local = peak > p[k - 1] && peak >= p[k + 1] && peak >= p[k - 2] && peak >= p[k + 2];
        local
            && peak > floor
            && [k - 4, k - 3, k + 3, k + 4].iter().all(|&n| p[n] <= under(peak, NARROW_DB))
    }

    /// The peak at bin `k`, unless it is quieter than the room's hiss.
    fn peak(&self, bin: usize) -> Option<Peak> {
        let p = &self.power;
        // A sine of amplitude A peaks at A * LENGTH / 4 through a Hann window
        let db = 10.0 * p[bin].max(1e-30).log10() + 20.0 * (4.0 / LENGTH as f32).log10();
        let at = self.at(bin);
        let quietest = under(p[bin], NO_HARMONIC_DB);
        let no_harmonic = (2..=4).all(|m| {
            m * bin + 1 >= p.len() || p[m * bin - 1..=m * bin + 1].iter().all(|&h| h <= quietest)
        });
        let no_fundamental = (2..=3).all(|d| {
            let fundamental = at / d as f32;
            let around = fundamental.round() as usize;
            around < 3
                || (around - 1..=around + 1).all(|j| {
                    let peak = p[j] > quietest && p[j] >= p[j - 1] && p[j] >= p[j + 1];
                    !(peak && (self.at(j) - fundamental).abs() <= FUNDAMENTAL_BINS)
                })
        });
        (db >= QUIETEST_DB).then_some(Peak { bin, at, db, alone: no_harmonic && no_fundamental })
    }

    /// Where the tone peaking at bin `k` sits between bins.
    fn at(&self, k: usize) -> f32 {
        let level = |k: usize| self.power[k].max(1e-30).ln();
        let (before, peak, after) = (level(k - 1), level(k), level(k + 1));
        let curvature = before - 2.0 * peak + after;
        let offset = if curvature < 0.0 {
            (0.5 * (before - after) / curvature).clamp(-0.5, 0.5)
        } else {
            0.0
        };
        k as f32 + offset
    }
}

/// `power` brought down by `db`.
fn under(power: f32, db: f32) -> f32 {
    power * 10f32.powf(-db / 10.0)
}
