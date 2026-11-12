use super::FRAME;
use super::lowpass::Lowpass;

/// One finder sample stands for this many frames: 3 kHz at 48 kHz, where most of a voice's and
/// a backing track's energy is, and enough to place the echo within a third of a millisecond.
pub const DECIMATION: usize = 16;
const PER_FRAME: usize = FRAME / DECIMATION;
/// Lags searched, in finder samples, from one frame, the least any echo can trail its
/// reference, up to 515 ms. A phone's whole path, capture and Wi-Fi included, sits inside it.
pub const TAPS: usize = 1536;
const STEP: f32 = 0.5;
/// A tap must carry this many times the mean tap energy to name the echo. The largest of 1536
/// taps of noise is about 15 times their mean; a clean echo path's peak is hundreds.
const PEAK_OVER_MEAN: f32 = 30.0;
/// The filter must predict at least this share of the mic's energy for its peak to count: a
/// peak among taps that explain nothing is noise, however tall.
const EXPLAINED: f32 = 0.3;
/// And what it explains must be at least an echo 40 dB under the master, or a quiet mic's
/// rounding would pass the test above.
const QUIETEST_ECHO: f32 = 1e-4;
/// The prediction's smoothing per frame: about ten frames, short enough that a singer's
/// breath between phrases lets the echo show.
const SMOOTHING: f32 = 0.1;
/// And twice the energy of any tap more than `RIVAL_GAP` away. Two tones that happen to share a
/// pitch, as a singer and the melody they sing over do, predict each other at every period
/// of it, so their taps are a comb of near-equal peaks; an echo path has one that leads.
const PEAK_OVER_RIVAL: f32 = 2.0;
const RIVAL_GAP: usize = 8;
/// Frames the same lag must lead for before the finder names it: 160 ms, longer than two
/// pitches stay together.
const AGREE: u32 = 60;
/// Two candidates this close, in finder samples, are the same lag.
const SAME_LAG: usize = 2;

/// Where the room's echo of the master arrives in a mic, found as AEC3 finds it: a matched
/// filter on low-passed, decimated copies of both signals, adapted by normalised LMS, whose
/// largest tap is the lag. It needs no probe and assumes nothing about the path.
pub struct DelayFinder {
    mic_lowpass: Lowpass,
    reference_lowpass: Lowpass,
    /// The decimated reference, written twice, at `i` and `i + TAPS`, so the `TAPS` newest
    /// samples are always one contiguous slice, oldest first.
    reference: Vec<f32>,
    written: usize,
    /// Oldest first, as the slice is: the last tap is the shortest lag.
    taps: Vec<f32>,
    /// The energy of the samples under the filter, kept as a running sum.
    energy: f32,
    /// The mic's energy and the prediction's error, smoothed.
    mic_energy: f32,
    error_energy: f32,
    frame_mic: f32,
    frame_error: f32,
    candidate: Option<usize>,
    agreed: u32,
    found: Option<usize>,
}

impl DelayFinder {
    pub fn new(rate: u32) -> Self {
        let cutoff = rate as f32 / DECIMATION as f32 * 0.4;
        Self {
            mic_lowpass: Lowpass::new(rate, cutoff),
            reference_lowpass: Lowpass::new(rate, cutoff),
            reference: vec![0.0; 2 * TAPS],
            written: 0,
            taps: vec![0.0; TAPS],
            energy: 0.0,
            mic_energy: 0.0,
            error_energy: 0.0,
            frame_mic: 0.0,
            frame_error: 0.0,
            candidate: None,
            agreed: 0,
            found: None,
        }
    }

    pub fn reset(&mut self) {
        self.mic_lowpass.reset();
        self.reference_lowpass.reset();
        self.reference.fill(0.0);
        self.taps.fill(0.0);
        self.energy = 0.0;
        self.mic_energy = 0.0;
        self.error_energy = 0.0;
        self.candidate = None;
        self.agreed = 0;
        self.found = None;
    }

    /// One frame: `reference` is the master's newest frame, rendered just before `mic` arrived.
    /// Returns the lag in frames from a master frame to its echo in the mic, once found.
    pub fn process(&mut self, reference: &[f32; FRAME], mic: &[f32; FRAME]) -> Option<usize> {
        for (r, m) in
            reference.as_chunks::<DECIMATION>().0.iter().zip(mic.as_chunks::<DECIMATION>().0.iter())
        {
            let (mut decimated_mic, mut decimated_reference) = (0.0, 0.0);
            // The last of each group of `DECIMATION` is the one kept
            for (&r, &m) in r.iter().zip(m) {
                decimated_mic = self.mic_lowpass.process(m);
                decimated_reference = self.reference_lowpass.process(r);
            }
            let leaving = self.window()[0];
            let at = self.written % TAPS;
            self.reference[at] = decimated_reference;
            self.reference[at + TAPS] = decimated_reference;
            self.written += 1;
            let entering = decimated_reference;
            self.energy = (self.energy + entering * entering - leaving * leaving).max(0.0);
            self.adapt(decimated_mic);
        }
        self.mic_energy += SMOOTHING * (self.frame_mic - self.mic_energy);
        self.error_energy += SMOOTHING * (self.frame_error - self.error_energy);
        self.frame_mic = 0.0;
        self.frame_error = 0.0;
        self.choose();
        self.found.map(|tap| (PER_FRAME + TAPS - 1 - tap) * DECIMATION)
    }

    /// The `TAPS` newest reference samples, oldest first.
    fn window(&self) -> &[f32] {
        let start = self.written % TAPS;
        &self.reference[start..start + TAPS]
    }

    /// One normalised LMS step. The mic sample is a frame after the newest reference sample,
    /// since the master's frame was rendered before the mic's arrived, so the last tap is a
    /// lag of one frame.
    fn adapt(&mut self, mic: f32) {
        if self.energy < 1e-6 * TAPS as f32 {
            return;
        }
        let start = self.written % TAPS;
        let window = &self.reference[start..start + TAPS];
        let predicted: f32 = self.taps.iter().zip(window).map(|(h, x)| h * x).sum();
        let error = mic - predicted;
        self.frame_mic += mic * mic;
        self.frame_error += error * error;
        let step = STEP * error / (self.energy + 1e-9);
        for (h, x) in self.taps.iter_mut().zip(window) {
            *h += step * x;
        }
    }

    fn choose(&mut self) {
        let (lag, peak) = self
            .taps
            .iter()
            .map(|h| h * h)
            .enumerate()
            .fold((0, 0.0f32), |best, (k, e)| if e > best.1 { (k, e) } else { best });
        let mean = self.taps.iter().map(|h| h * h).sum::<f32>() / TAPS as f32;
        let explained = self.mic_energy - self.error_energy;
        let played = self.energy / TAPS as f32 * PER_FRAME as f32;
        let explains =
            explained >= EXPLAINED * self.mic_energy && explained >= QUIETEST_ECHO * played;
        let rival = self
            .taps
            .iter()
            .enumerate()
            .filter(|(k, _)| k.abs_diff(lag) > RIVAL_GAP)
            .map(|(_, h)| h * h)
            .fold(0.0f32, f32::max);
        let leads = peak > PEAK_OVER_MEAN * mean && peak > PEAK_OVER_RIVAL * rival;
        if !explains || !leads {
            self.agreed = 0;
            return;
        }
        match self.candidate {
            Some(previous) if previous.abs_diff(lag) <= SAME_LAG => self.agreed += 1,
            _ => {
                self.candidate = Some(lag);
                self.agreed = 1;
            }
        }
        if self.agreed >= AGREE {
            self.found = Some(lag);
        }
    }
}
