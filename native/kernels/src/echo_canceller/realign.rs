//! Finds where a new stream's echo landed. A phone back from a pause starts a stream its jitter
//! buffer anchors where the first packet happened to arrive, so the echo can land a fraction of
//! a packet or more from where the last stream had it. The room has not changed, so neither has
//! the filter: only where it reads the master is in question. Read up to `REACH` early, the
//! filter's estimate is the echo the mic will hear some lag later, and the peak of their
//! cross-correlation is that lag, to a fraction of a frame.

use super::fft::{Fft, SILENT, Spectrum};
use super::{FRAME, PREEMPHASIS, WINDOW};

/// How far either way the echo is looked for: 21 ms, a block's rounding of where a stream is
/// anchored and as much again as three quarters of a 20 ms packet.
pub const REACH: usize = 8 * FRAME;
/// The lags searched, `2 * REACH` of them, a frame's worth per partition.
const PARTITIONS: usize = 2 * REACH / FRAME;
/// Heard frames summed before the peak is judged: 85 ms.
const LEAST_HEARD: u32 = 32;
/// Heard frames after which an echo still not found is not the filter's: a second.
const MOST_HEARD: u32 = 375;
/// The peak's correlation, normalised by both energies, that names the lag.
const LIKE: f32 = 0.5;
/// And at least twice any lag more than `RIVAL_GAP` frames from it. A voice correlates with
/// itself a few pitch periods away nearly as well as at its own lag over one syllable: without
/// this, an echo 30 frames early was taken for one 711 early. Once the pitch moves, the room's
/// lag leads.
const PEAK_OVER_RIVAL: f32 = 2.0;
const RIVAL_GAP: usize = 16;
/// Under one 16-bit step, RMS over the frame, a mic hears nothing to correlate.
const QUIETEST_MIC: f32 = FRAME as f32;

/// What one frame of the search told the caller.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Search {
    /// Not yet sure.
    Looking,
    /// The echo arrives this many frames after the estimate: as many as it was read early is
    /// where the old stream had it.
    Found(f64),
    /// A second heard and no lag stands out: the filter's room is not the one the mic hears.
    Lost,
}

/// The cross-correlation of the mic with the filter's estimate over `2 * REACH` lags, summed in
/// the frequency domain a partition at a time, as the filter's own gradient is. Built on the
/// control thread; `hear` allocates nothing.
pub struct Realign {
    fft: Fft,
    /// The estimate's windows, newest first: window `p` ends `p * FRAME` frames before this one.
    windows: Vec<Spectrum>,
    /// Each partition's summed cross-spectrum: lags `p * FRAME` to `(p + 1) * FRAME`.
    sums: Vec<Spectrum>,
    estimate: [f32; WINDOW],
    mic: [f32; WINDOW],
    spectrum: Spectrum,
    window: [f32; WINDOW],
    /// The correlation at every lag, once judged.
    correlation: [f32; 2 * REACH],
    last_mic: f32,
    heard: u32,
    mic_energy: f32,
    estimate_energy: f32,
    looking: bool,
}

impl Realign {
    pub fn new() -> Self {
        Self {
            fft: Fft::new(),
            windows: vec![SILENT; PARTITIONS],
            sums: vec![SILENT; PARTITIONS],
            estimate: [0.0; WINDOW],
            mic: [0.0; WINDOW],
            spectrum: SILENT,
            window: [0.0; WINDOW],
            correlation: [0.0; 2 * REACH],
            last_mic: 0.0,
            heard: 0,
            mic_energy: 0.0,
            estimate_energy: 0.0,
            looking: false,
        }
    }

    /// Starts a search from nothing.
    pub fn begin(&mut self) {
        self.windows.fill(SILENT);
        self.sums.fill(SILENT);
        self.estimate = [0.0; WINDOW];
        self.last_mic = 0.0;
        self.heard = 0;
        self.mic_energy = 0.0;
        self.estimate_energy = 0.0;
        self.looking = true;
    }

    pub fn stop(&mut self) {
        self.looking = false;
    }

    pub fn looking(&self) -> bool {
        self.looking
    }

    /// One frame: `estimate` is the filter's echo, pre-emphasised and read early, and `mic` what
    /// the slot heard, both on the 16-bit scale. A mic that hears nothing counts for nothing.
    pub fn hear(&mut self, estimate: &[f32; FRAME], mic: &[f32; FRAME]) -> Search {
        self.estimate.copy_within(FRAME.., 0);
        self.estimate[FRAME..].copy_from_slice(estimate);
        self.windows.rotate_right(1);
        self.fft.forward(&self.estimate, &mut self.windows[0]);
        let mut energy = 0.0;
        for (emphasised, &sample) in self.mic[FRAME..].iter_mut().zip(mic) {
            *emphasised = sample - PREEMPHASIS * self.last_mic;
            self.last_mic = sample;
            energy += sample * sample;
        }
        if energy < QUIETEST_MIC {
            return Search::Looking;
        }
        self.heard += 1;
        self.mic_energy += self.mic[FRAME..].iter().map(|s| s * s).sum::<f32>();
        self.estimate_energy += estimate.iter().map(|s| s * s).sum::<f32>();
        self.fft.forward(&self.mic, &mut self.spectrum);
        for (sum, window) in self.sums.iter_mut().zip(&self.windows) {
            for ((sum, w), m) in sum.iter_mut().zip(window).zip(&self.spectrum) {
                *sum += w.conj() * m;
            }
        }
        if self.heard < LEAST_HEARD {
            return Search::Looking;
        }
        let verdict = match self.peak() {
            Some(lag) => Search::Found(lag),
            None if self.heard >= MOST_HEARD => Search::Lost,
            None => Search::Looking,
        };
        self.looking = verdict == Search::Looking;
        verdict
    }

    /// The lag of the correlation's peak, between frames, if it stands out.
    fn peak(&mut self) -> Option<f64> {
        for (p, sum) in self.sums.iter().enumerate() {
            self.fft.inverse(sum, &mut self.window);
            self.correlation[p * FRAME..(p + 1) * FRAME].copy_from_slice(&self.window[..FRAME]);
        }
        let (at, &peak) = self
            .correlation
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap_or((0, &0.0));
        let rival = self
            .correlation
            .iter()
            .enumerate()
            .filter(|(lag, _)| lag.abs_diff(at) > RIVAL_GAP)
            .map(|(_, &value)| value)
            .fold(0.0f32, f32::max);
        let alike = peak >= LIKE * (self.mic_energy * self.estimate_energy).sqrt() / WINDOW as f32;
        if !alike || peak < PEAK_OVER_RIVAL * rival {
            return None;
        }
        // A parabola through the peak and its neighbours puts it between frames
        let near = |lag: Option<usize>| {
            f64::from(lag.and_then(|lag| self.correlation.get(lag)).copied().unwrap_or(peak))
        };
        let (before, after) = (near(at.checked_sub(1)), near(Some(at + 1)));
        let curve = before - 2.0 * f64::from(peak) + after;
        let fraction = if curve < 0.0 { 0.5 * (before - after) / curve } else { 0.0 };
        Some(at as f64 + fraction.clamp(-0.5, 0.5))
    }
}

impl Default for Realign {
    fn default() -> Self {
        Self::new()
    }
}
