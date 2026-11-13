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
/// A frame is heard only when the filter's estimate, the learned path's echo of the master, holds
/// at least this share of the mic's energy: 10 dB under it. The peak needs a normalised
/// correlation of `LIKE`, and a perfect estimate 10 dB under the mic reaches only 0.32, so a
/// quieter one dilutes the sum and cannot name the lag.
const ESTIMATE_SHARE: f32 = 0.1;
/// An estimate under 10 steps RMS, -70 dBFS, is a master that reaches no mic: 10 dB under the
/// quietest room floor these tests use. A silent intro or a break, or a master that holds only
/// the mic's own voice, which the estimate leaves out, is silence, and the search waits it out
/// with the slot passing untouched, however long it lasts.
const QUIETEST_ESTIMATE: f32 = 100.0 * FRAME as f32;
/// Frames since `begin` that are not silence, heard or not, after which a search that has not
/// named the lag ends: 2 s. The slowest search in these tests named its lag 0.5 s after the
/// master reached the mic. A mic that hears the room far louder than the filter learned, as a
/// phone picked up does, is never heard, and would keep the slot untouched for good; so it
/// passes untouched for 2 s at most, and silence between never adds to that.
const MOST_AUDIBLE: u32 = 750;
/// Frames of silence in a row after which what was summed is forgotten, and the next frame heard
/// starts the sums afresh: 10 s. The reference moves on at the drift's learned rate meanwhile,
/// learned to 1% (decision 0022), so at 100 ppm a sum held over 10 s of silence sits half a frame
/// from the frames after it, and one held longer blurs the peak.
const FORGET_AFTER: u32 = 3750;
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
    /// The master reached the mic for `MOST_AUDIBLE` frames, enough of them heard to judge, and no
    /// lag stood out: a singer over the return, or a room the filter never learned. The filter
    /// keeps what it holds.
    Lost,
    /// The master reached the mic for `MOST_AUDIBLE` frames and too few were heard to judge a
    /// peak at all: the mic outweighed the estimate throughout, as a room far louder than the
    /// filter learned does.
    Outweighed,
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
    /// Frames since `begin` that were not silence. `forget` keeps it, so silence between them
    /// never keeps a search open.
    audible: u32,
    /// Frames heard since `begin`. `forget` keeps it too: a search that heard fewer than
    /// `LEAST_HEARD` in its `MOST_AUDIBLE` was outweighed. A room 15 dB louder than the filter
    /// learned left 3 of 750 frames heard; a note held 17 dB over the echo for the whole search,
    /// 72, and one 9 dB over, 382.
    heard_since_begin: u32,
    /// Frames of silence in a row.
    silent: u32,
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
            audible: 0,
            heard_since_begin: 0,
            silent: 0,
            mic_energy: 0.0,
            estimate_energy: 0.0,
            looking: false,
        }
    }

    /// Starts a search from nothing.
    pub fn begin(&mut self) {
        self.windows.fill(SILENT);
        self.estimate = [0.0; WINDOW];
        self.last_mic = 0.0;
        self.audible = 0;
        self.heard_since_begin = 0;
        self.forget();
        self.looking = true;
    }

    fn forget(&mut self) {
        self.sums.fill(SILENT);
        self.heard = 0;
        self.silent = 0;
        self.mic_energy = 0.0;
        self.estimate_energy = 0.0;
    }

    pub fn stop(&mut self) {
        self.looking = false;
    }

    pub fn looking(&self) -> bool {
        self.looking
    }

    /// One frame: `estimate` is the filter's echo, pre-emphasised and read early, and `mic` what
    /// the slot heard, both on the 16-bit scale. A frame of silence counts for nothing, and one
    /// whose estimate is too quiet against the mic counts only toward `MOST_AUDIBLE`.
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
        let emphasised: f32 = self.mic[FRAME..].iter().map(|s| s * s).sum();
        let estimated: f32 = estimate.iter().map(|s| s * s).sum();
        if energy < QUIETEST_MIC || estimated < QUIETEST_ESTIMATE {
            self.silent += 1;
            if self.silent >= FORGET_AFTER {
                self.forget();
            }
            return Search::Looking;
        }
        self.silent = 0;
        self.audible += 1;
        if estimated < ESTIMATE_SHARE * emphasised {
            return self.verdict(None);
        }
        self.heard += 1;
        self.heard_since_begin += 1;
        self.mic_energy += emphasised;
        self.estimate_energy += estimated;
        self.fft.forward(&self.mic, &mut self.spectrum);
        for (sum, window) in self.sums.iter_mut().zip(&self.windows) {
            for ((sum, w), m) in sum.iter_mut().zip(window).zip(&self.spectrum) {
                *sum += w.conj() * m;
            }
        }
        let lag = if self.heard < LEAST_HEARD { None } else { self.peak() };
        self.verdict(lag)
    }

    fn verdict(&mut self, lag: Option<f64>) -> Search {
        let verdict = match lag {
            Some(lag) => Search::Found(lag),
            None if self.audible < MOST_AUDIBLE => Search::Looking,
            None if self.heard_since_begin < LEAST_HEARD => Search::Outweighed,
            None => Search::Lost,
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
            .fold(0.0, f32::max);
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
