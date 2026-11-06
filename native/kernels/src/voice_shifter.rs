mod delay_line;
mod envelope;
mod formant_filter;
mod lattice;
#[cfg(test)]
mod tests;

use delay_line::DelayLine;
use envelope::{Envelope, ORDER};
use formant_filter::FormantFilter;

use crate::yin::YinDetector;

/// The most the kernel transposes: a harmony's octave, either way.
const MAX_SEMITONES: f32 = 12.0;
const MIN_FORMANT_FACTOR: f32 = 0.5;
const MAX_FORMANT_FACTOR: f32 = 2.0;
/// YIN over 2048 samples at 48 kHz, read every 10 ms: the corrector's detector and cadence.
const DETECTOR_WINDOW_SECONDS: f64 = 2048.0 / 48_000.0;
const DETECTOR_HOP_SECONDS: f64 = 0.010;
/// A reading this sure is a voice, the corrector's bar.
const VOICED_CONFIDENCE: f32 = 0.9;
/// E2 and just above the detector's top. A jump never exceeds E2's period, so the delay has a
/// bound whatever the input; a lower voice gets worse splices, not more delay.
const LOWEST_HZ: f64 = 82.0;
const HIGHEST_HZ: f64 = 1100.0;
/// Until the detector has heard a voice there is nothing to jump over; any plausible period does.
const STARTING_HZ: f64 = 200.0;
/// How often the envelope is read: 5 ms, about as fast as a vocal tract moves, or a quarter of
/// the window where a low voice's long window would make that the kernel's largest cost.
const ENVELOPE_HOP_SECONDS: f64 = 0.005;
const ENVELOPE_HOPS_PER_WINDOW: usize = 4;
/// Moving the delay line in or out of the voice's path, which changes the delay, as the
/// corrector's own fade does.
const ENGAGE_SECONDS: f64 = 0.020;

/// Shifts one mono voice's pitch and formants apart, with no lookahead. The pitch comes from a
/// delay line read at the transposition's rate; a resampled voice's envelope moves with it, so a
/// zero-delay filter then takes the moved envelope off and puts the asked one on. The pitch out
/// is the pitch in times the ratio whatever the detector reads, and a formant move adds no delay
/// at all. docs/decisions/0004-voice-shifter.md has why, and what it costs against the rest.
///
/// The output trails the input by at most `latency_bound_samples`: for a shift up, 5 samples plus
/// what the read point gains during a 2.7 ms crossfade, plus 1.125 periods of the voice; for a
/// shift down, 5 samples plus 1.125 periods. With no pitch shift there is no delay. At no shift
/// and no formant move the output is the input itself.
///
/// The audio thread calls `process` and `set` between calls. Nothing allocates, locks or panics
/// after `new`.
pub struct VoiceShifter {
    line: DelayLine,
    detector: YinDetector,
    detector_window: Vec<f32>,
    envelope: Envelope,
    envelope_samples: Vec<f32>,
    filter: FormantFilter,
    rate: f64,
    ratio: f64,
    formant_factor: f64,
    period: f64,
    shortest_period: f64,
    longest_period: f64,
    /// How much of the path goes through the delay line, from 0 to 1.
    line_mix: f32,
    line_mix_step: f32,
    detector_hop: usize,
    until_reading: usize,
    envelope_hop: usize,
    until_envelope: usize,
}

impl VoiceShifter {
    pub fn new(rate: u32) -> Self {
        let rate = f64::from(rate);
        let detector_samples = (rate * DETECTOR_WINDOW_SECONDS).round() as usize;
        let envelope = Envelope::new(rate);
        let longest_window = envelope.longest_window();
        let longest_period = rate / LOWEST_HZ;
        // Two jumps back and a correlation window, and the longest thing that reads the history
        let reach = detector_samples.max(longest_window) + (3.0 * longest_period) as usize;
        let envelope_hop = (rate * ENVELOPE_HOP_SECONDS).round() as usize;
        Self {
            line: DelayLine::new(rate, reach, longest_period),
            detector: YinDetector::new(rate as u32, detector_samples),
            detector_window: vec![0.0; detector_samples],
            envelope,
            envelope_samples: vec![0.0; longest_window],
            filter: FormantFilter::new(rate),
            rate,
            ratio: 1.0,
            formant_factor: 1.0,
            period: rate / STARTING_HZ,
            shortest_period: rate / HIGHEST_HZ,
            longest_period,
            line_mix: 0.0,
            line_mix_step: (1.0 / (rate * ENGAGE_SECONDS)) as f32,
            detector_hop: (rate * DETECTOR_HOP_SECONDS).round() as usize,
            until_reading: 0,
            envelope_hop,
            until_envelope: 0,
        }
    }

    /// The transposition in semitones, up to an octave either way, and the factor the formants
    /// move by from where the voice has them, from 0.5 to 2. At 1 they stay put through a
    /// transposition. A change of transposition applies from the next sample; a host wanting a
    /// glide calls this per block.
    pub fn set(&mut self, semitones: f32, formant_factor: f32) {
        // A NaN would stay in the read point for good. Params come from the host: ignore it.
        if !(semitones.is_finite() && formant_factor.is_finite()) {
            return;
        }
        let was_shifting = self.is_shifting();
        self.ratio = f64::from(2f32.powf(semitones.clamp(-MAX_SEMITONES, MAX_SEMITONES) / 12.0));
        self.formant_factor =
            f64::from(formant_factor.clamp(MIN_FORMANT_FACTOR, MAX_FORMANT_FACTOR));
        if self.is_shifting() && !was_shifting && self.line_mix == 0.0 {
            self.line.start(self.ratio, self.period);
        }
    }

    /// Panics never: a run takes the shorter of the two lengths. Hand it equal ones.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        if self.is_idle() {
            for (sample, out) in input.iter().zip(output) {
                self.line.write(*sample);
                *out = *sample;
            }
            // The first sample after this reads the voice, whose history is already in the line
            self.until_reading = 0;
            self.until_envelope = 0;
            return;
        }
        for (sample, out) in input.iter().zip(output) {
            self.line.write(*sample);
            self.follow_the_voice();

            let through_line = if self.line_mix > 0.0 || self.is_shifting() {
                self.line.read(self.ratio, self.period)
            } else {
                *sample
            };
            let target = if self.is_shifting() { 1.0 } else { 0.0 };
            self.line_mix = if target > self.line_mix {
                (self.line_mix + self.line_mix_step).min(1.0)
            } else {
                (self.line_mix - self.line_mix_step).max(0.0)
            };
            let voice = sample + self.line_mix * (through_line - sample);
            let shaped =
                if self.filter.is_transparent() { voice } else { self.filter.process(voice) };
            // A bad sample stays in the delay line until it is played out; it stops here
            *out = if shaped.is_finite() { shaped } else { 0.0 };
        }
    }

    /// Forgets the voice, for a new singer on the same slot. Keeps the settings.
    pub fn reset(&mut self) {
        self.line.clear();
        self.envelope.clear();
        self.filter.clear();
        self.line_mix = 0.0;
        self.period = self.rate / STARTING_HZ;
        self.until_reading = 0;
        self.until_envelope = 0;
        if self.is_shifting() {
            self.line.start(self.ratio, self.period);
        }
    }

    /// How far the output trails the input now, in samples; 0 with no pitch shift. What a slot
    /// reports in `stats()`.
    pub fn latency_samples(&self) -> usize {
        if self.line_mix > 0.0 || self.is_shifting() {
            self.line.delay().max(0.0).round() as usize
        } else {
            0
        }
    }

    /// The most `latency_samples` can read at this transposition for a voice at `hz`, once the
    /// detector has heard it. A voice under 82 Hz is bound as one at 82 Hz.
    pub fn latency_bound_samples(rate: u32, semitones: f32, hz: f64) -> usize {
        if semitones == 0.0 {
            return 0;
        }
        let rate = f64::from(rate);
        let ratio = f64::from(2f32.powf(semitones.clamp(-MAX_SEMITONES, MAX_SEMITONES) / 12.0));
        let period = (rate / hz).clamp(rate / HIGHEST_HZ, rate / LOWEST_HZ);
        delay_line::bound_samples(ratio, period, rate).ceil() as usize
    }

    fn is_shifting(&self) -> bool {
        self.ratio != 1.0
    }

    fn moves_formants(&self) -> bool {
        self.ratio != self.formant_factor
    }

    fn is_idle(&self) -> bool {
        !self.is_shifting()
            && !self.moves_formants()
            && self.line_mix == 0.0
            && self.filter.is_transparent()
    }

    fn follow_the_voice(&mut self) {
        if self.until_reading == 0 {
            self.until_reading = self.detector_hop;
            self.read_the_period();
        }
        self.until_reading -= 1;
        if self.until_envelope == 0 {
            self.until_envelope = self.read_the_envelope();
        }
        self.until_envelope -= 1;
    }

    fn read_the_period(&mut self) {
        self.line.copy_newest(&mut self.detector_window);
        let pitch = self.detector.analyze(&self.detector_window);
        if pitch.confidence >= VOICED_CONFIDENCE && pitch.hz > 0.0 {
            let period = self.rate / f64::from(pitch.hz);
            self.period = period.clamp(self.shortest_period, self.longest_period);
        }
    }

    /// Sets the filter's next target and says how many samples until the next one.
    fn read_the_envelope(&mut self) -> usize {
        const FLAT: [f32; ORDER] = [0.0; ORDER];
        let count = self.envelope.window_samples(self.period);
        let hop = self.envelope_hop.max(count / ENVELOPE_HOPS_PER_WINDOW);
        if !self.moves_formants() {
            self.filter.glide_to(&FLAT, &FLAT, hop);
            return hop;
        }
        let window = &mut self.envelope_samples[..count];
        self.line.copy_newest(window);
        let largest = self.ratio.max(self.formant_factor);
        if self.envelope.measure(window, self.period, largest) {
            let whiten = self.envelope.model_at(self.ratio);
            let colour = self.envelope.model_at(self.formant_factor);
            self.filter.glide_to(&whiten, &colour, hop);
        } else {
            self.filter.glide_to(&FLAT, &FLAT, hop);
        }
        hop
    }
}
