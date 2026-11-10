use std::ptr::NonNull;

use crate::ffi;
use crate::pitch::{Pitch, PitchClassMask};

/// Pulls one mono voice from 82 Hz up toward the nearest allowed pitch class. Every 5 ms it reads
/// the input, from the shortest window that can vouch for the voice; the distance from where the
/// voice is now to the nearest class in the target, scaled by the strength and eased in over the
/// retune time, becomes a transposition a time-domain shifter applies. The output trails the input
/// by 1.2 ms plus at most 1.125 periods of the voice. At strength 0 the output is the input itself
/// with no delay, and turning the strength up from or down to 0 crossfades over 20 ms; it reads
/// either way. docs/decisions/0010-pitch-reading.md says why.
///
/// The audio thread calls `process`, and `set_target` between calls. Nothing allocates after
/// `new`.
pub struct PitchCorrector {
    raw: NonNull<ffi::Corrector>,
}

// The C++ object has no thread affinity and this wrapper is its only owner, so a graph built on
// the control thread can carry it to the audio thread.
unsafe impl Send for PitchCorrector {}

impl PitchCorrector {
    pub fn new(rate: u32) -> Self {
        // SAFETY: the shim returns an owned object or null, and takes a plain number
        let raw = unsafe { ffi::aeon_corrector_new(f64::from(rate)) };
        Self { raw: NonNull::new(raw).expect("the corrector could not allocate") }
    }

    /// What to pull toward (`pitch_class_of(note)` for a melody note), how far from 0 to 1, and
    /// how fast in milliseconds. A new target counts from the last reading at once.
    pub fn set_target(&mut self, mask: PitchClassMask, strength: f32, retune_ms: f32) {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_set_target(self.raw.as_ptr(), mask, strength, retune_ms) }
    }

    /// Panics unless `input` and `output` are the same length.
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        assert_eq!(input.len(), output.len(), "input and output of one length");
        let frames = i32::try_from(input.len()).expect("a run that fits an i32");
        // SAFETY: both buffers hold `frames` samples, and a `&` and a `&mut` never overlap
        unsafe {
            ffi::aeon_corrector_process(
                self.raw.as_ptr(),
                input.as_ptr(),
                output.as_mut_ptr(),
                frames,
            )
        }
    }

    /// Forgets the voice it was hearing, for a new singer on the same slot. Keeps the target and
    /// allocates nothing.
    pub fn reset(&mut self) {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_reset(self.raw.as_ptr()) }
    }

    /// The newest reading, of where the voice was at its window's centre.
    pub fn detected(&self) -> Pitch {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_detected(self.raw.as_ptr()) }
    }

    /// Frames until the next reading to publish, one every 10 ms. A host that publishes them
    /// processes at most this many at a time.
    pub fn frames_until_reading(&self) -> usize {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_frames_until_reading(self.raw.as_ptr()) as usize }
    }

    /// How far the newest reading's centre trails the newest sample it read: half the window it
    /// took, which follows the voice.
    pub fn reading_lag_samples(&self) -> usize {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_reading_lag_samples(self.raw.as_ptr()) as usize }
    }

    /// The newest reading's distance to the class it is pulled toward, in semitones; 0 while
    /// unvoiced.
    pub fn error_semitones(&self) -> f32 {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_error_semitones(self.raw.as_ptr()) }
    }

    /// How far the output trails the input now; 0 while bypassed.
    pub fn latency_samples(&self) -> usize {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_latency_samples(self.raw.as_ptr()) as usize }
    }
}

impl Drop for PitchCorrector {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `aeon_corrector_new` and is freed once, here
        unsafe { ffi::aeon_corrector_free(self.raw.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::pitch::{EVERY_PITCH_CLASS, pitch_class_of};
    use crate::test_support::{
        RATE, buzz, cents_between, harmonic_tone, median_pitch, midi_hz, sung,
    };

    /// `input` through `corrector` in runs of `block` frames, as a host's buffers would arrive,
    /// under the allocation guard.
    fn corrected(corrector: &mut PitchCorrector, input: &[f32], block: usize) -> Vec<f32> {
        let mut output = vec![0.0; input.len()];
        assert_no_alloc(|| {
            for (input, output) in input.chunks(block).zip(output.chunks_mut(block)) {
                corrector.process(input, output);
            }
        });
        output
    }

    // The engine's done-when: harmonic tones from 82 to 880 Hz land on the asked class, the way
    // Aeon Karaoke's shifter test asks for half and whole tones either way. The tolerance is its
    // corrector test's 10 cents, which leaves room for the detector's own error.
    #[test]
    fn pulls_tones_from_82_to_880_hz_onto_the_asked_class() {
        let rate = f64::from(RATE);
        for target in [40, 45, 57, 69, 81] {
            for shift in [0.5, -0.5, 2.0, -2.0] {
                let input_hz = midi_hz(f64::from(target) - shift);
                // Below E2 a voice passes uncorrected, by design
                if input_hz < 82.0 {
                    continue;
                }
                let mut corrector = PitchCorrector::new(RATE);
                corrector.set_target(pitch_class_of(target), 1.0, 0.0);
                let output = corrected(&mut corrector, &harmonic_tone(input_hz, 2.0), 128);

                let cents = cents_between(median_pitch(&output, 0.5, 2.0), midi_hz(target.into()));
                assert!(
                    cents.abs() < 10.0,
                    "{input_hz:.1} Hz to MIDI {target} came out {cents:+.1} cents off"
                );
                // The shifter's floor plus at most an eighth over one period of the voice
                let bound = 64 + (1.125 * rate / input_hz) as usize;
                let latency = corrector.latency_samples();
                assert!(
                    latency > 0 && latency <= bound,
                    "latency {latency} at {input_hz:.1} Hz, bound {bound}"
                );
            }
        }
    }

    // A held note a quarter tone flat lands within 10 cents at full strength, and the detector
    // reports the pitch that was sung. A2 is the low voice whose long period once kept the
    // corrector from engaging at all.
    #[test]
    fn quarter_tone_flat_lands_on_the_note() {
        for midi in [69, 45] {
            let target = midi_hz(f64::from(midi));
            let flat = midi_hz(f64::from(midi) - 0.5);
            let mut corrector = PitchCorrector::new(RATE);
            corrector.set_target(pitch_class_of(midi), 1.0, 0.0);
            // 441 frames: runs that never line up with the detector's 480-frame hop
            let output = corrected(&mut corrector, &harmonic_tone(flat, 2.0), 441);

            let detected = f64::from(corrector.detected().hz);
            assert!(
                cents_between(detected, flat).abs() < 5.0,
                "detected {detected} Hz for {flat} Hz"
            );
            let cents = cents_between(median_pitch(&output, 1.0, 2.0), target);
            assert!(cents.abs() < 10.0, "output {cents:+.1} cents from {target:.1} Hz");
        }
    }

    /// Every reading of `voice`, 5 ms apart, with the frame it landed on.
    fn every_reading(corrector: &mut PitchCorrector, voice: &[f32]) -> Vec<(usize, Pitch)> {
        let mut output = [0.0; 240];
        let mut readings = Vec::with_capacity(voice.len() / 240);
        assert_no_alloc(|| {
            for (index, run) in voice.as_chunks::<240>().0.iter().enumerate() {
                corrector.process(run, &mut output);
                readings.push(((index + 1) * 240, corrector.detected()));
            }
        });
        readings
    }

    // The window follows the voice: the shortest that holds 2.2 periods, so a voice from 82 Hz up
    // stays in reach of the corrector and a high one is read in a quarter of the time. The error
    // says the corrector believes the reading as well as making it
    #[test]
    fn the_window_is_the_shortest_that_holds_the_voice() {
        for (hz, lag) in [
            (82.5, 1024),
            (98.0, 1024),
            (110.0, 512),
            (196.0, 512),
            (207.0, 256),
            (440.0, 256),
            (880.0, 256),
        ] {
            let mut corrector = PitchCorrector::new(RATE);
            corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
            corrected(&mut corrector, &harmonic_tone(hz, 0.5), 128);

            let read = corrector.detected();
            assert_eq!(corrector.reading_lag_samples(), lag, "{hz} Hz");
            assert!(read.confidence > 0.9, "{hz} Hz: confidence {}", read.confidence);
            assert!(cents_between(f64::from(read.hz), hz).abs() < 0.5, "read {} for {hz}", read.hz);
            let midi = 69.0 + 12.0 * (hz / 440.0).log2();
            let error = f64::from(corrector.error_semitones());
            assert!((error - (midi.round() - midi)).abs() < 0.01, "{hz} Hz: error {error}");
        }
    }

    // How long after a voice steps two semitones its reading has settled within 30 cents of the
    // new note, and stays. The longest window alone answers in 40 ms at every pitch. Readings land
    // 5 ms apart, so each bound is what was measured and one reading more
    #[test]
    fn a_pitch_step_is_read_within_the_voices_own_window() {
        let step_at = 24_000;
        for (hz, bound_ms) in [(440.0, 15.0), (220.0, 15.0), (110.0, 20.0), (82.5, 35.0)] {
            let up = hz * 2f64.powf(2.0 / 12.0);
            let voice = sung(48_000, &buzz(), |i| if i < step_at { hz } else { up });
            let readings = every_reading(&mut PitchCorrector::new(RATE), &voice);
            let last_astray = readings
                .iter()
                .rfind(|(_, read)| cents_between(f64::from(read.hz), up).abs() > 30.0)
                .expect("the old note is astray");
            let delay_ms = (last_astray.0 + 240 - step_at) as f64 / 48.0;
            assert!(delay_ms <= bound_ms, "{hz} Hz answered in {delay_ms} ms, bound {bound_ms}");
        }
    }

    // A real leap beyond the short windows' say goes to the longest window, which believes it as
    // soon as the new note fills most of it
    #[test]
    fn an_octave_leap_is_believed_within_the_longest_window() {
        let step_at = 24_000;
        for (from, to) in [(220.0, 440.0), (440.0, 220.0), (110.0, 220.0)] {
            let voice = sung(48_000, &buzz(), |i| if i < step_at { from } else { to });
            let readings = every_reading(&mut PitchCorrector::new(RATE), &voice);
            let last_astray = readings
                .iter()
                .rfind(|(_, read)| cents_between(f64::from(read.hz), to).abs() > 30.0)
                .expect("the old note is astray");
            let delay_ms = (last_astray.0 + 240 - step_at) as f64 / 48.0;
            assert!(delay_ms <= 48.0, "{from} to {to} Hz answered in {delay_ms} ms");
        }
    }

    // For a moment a vowel's second harmonic drowns its fundamental, and a short window dips at
    // half the period. The voice has not leapt an octave, so the reading stays on its note
    #[test]
    fn a_moment_of_loud_second_harmonic_does_not_pass_for_an_octave() {
        let (moment, frames) = (24_000..25_440, 48_000);
        let plain = sung(frames, &buzz(), |_| 220.0);
        let loud_second = sung(frames, &[0.15, 1.0], |_| 220.0);
        let voice: Vec<f32> = (0..frames)
            .map(|i| if moment.contains(&i) { loud_second[i] } else { plain[i] })
            .collect();
        let readings = every_reading(&mut PitchCorrector::new(RATE), &voice);
        for (frame, read) in readings.iter().filter(|(frame, _)| *frame > 4800) {
            let cents = cents_between(f64::from(read.hz), 220.0);
            assert!(cents.abs() < 100.0, "at {frame}: {cents:+.1} cents from 220 Hz");
        }
    }

    // On a steady glide of one cent a millisecond, a corrector that believed its reading's centre
    // would trail by half its window and a hop, each millisecond a cent. Reading where the voice
    // has gone leaves under a cent at a hard retune, whichever window the voice takes
    #[test]
    fn a_glide_is_corrected_where_the_voice_is_now() {
        for midi in [69, 57, 45] {
            let centre = midi_hz(f64::from(midi));
            // Five semitones either side of the note, which the shifter can reach all the way
            let voice = sung(RATE as usize, &buzz(), |i| {
                let cents_per_frame = 1.0 / 48.0;
                centre * 2f64.powf((i as f64 * cents_per_frame - 500.0) / 1200.0)
            });
            let mut corrector = PitchCorrector::new(RATE);
            corrector.set_target(pitch_class_of(midi), 1.0, 0.0);
            let output = corrected(&mut corrector, &voice, 128);
            let cents = cents_between(median_pitch(&output, 0.3, 0.7), centre);
            assert!(cents.abs() < 1.0, "MIDI {midi} came out {cents:+.1} cents off");
        }
    }

    // A new stream on the slot starts clean: nothing of the last singer comes out after a reset
    #[test]
    fn reset_forgets_the_last_voice() {
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(pitch_class_of(57), 1.0, 0.0);
        corrected(&mut corrector, &harmonic_tone(215.0, 1.0), 480);
        assert_no_alloc(|| corrector.reset());
        let output = corrected(&mut corrector, &[0.0; 4800], 480);
        assert!(output.iter().all(|s| *s == 0.0), "the last voice came out after a reset");
        assert_eq!(corrector.detected().hz, 0.0);
        assert_eq!(corrector.error_semitones(), 0.0);
    }

    // Key mode: a note between F and G, nearer the F# C major leaves out, goes to F, the nearer
    // class in the key
    #[test]
    fn key_mode_skips_classes_outside_the_key() {
        let c_major = 0b1010_1011_0101;
        let f4 = midi_hz(65.0);
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(c_major, 1.0, 20.0);
        let output = corrected(&mut corrector, &harmonic_tone(midi_hz(65.8), 1.5), 480);
        let cents = cents_between(median_pitch(&output, 0.8, 1.5), f4);
        assert!(cents.abs() < 10.0, "output {cents:+.1} cents from F4");
        let error = corrector.error_semitones();
        assert!((error + 0.8).abs() < 0.05, "error {error} semitones, expected -0.8");
    }

    // Off is the input untouched and undelayed, while the detector keeps reading
    #[test]
    fn strength_zero_is_the_input_itself() {
        let input = harmonic_tone(220.0, 0.5);
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(pitch_class_of(70), 0.0, 40.0);
        let output = corrected(&mut corrector, &input, 480);
        assert_eq!(output, input, "strength 0 changed the signal");
        assert_eq!(corrector.latency_samples(), 0);
        let detected = f64::from(corrector.detected().hz);
        assert!(
            cents_between(detected, 220.0).abs() < 5.0,
            "detected {detected} Hz while bypassed"
        );
    }

    // Silence has no pitch and nothing to correct
    #[test]
    fn silence_reads_as_unvoiced() {
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 0.0);
        let output = corrected(&mut corrector, &[0.0; 24_000], 480);
        assert_eq!(corrector.detected(), Pitch::default());
        assert_eq!(corrector.error_semitones(), 0.0);
        assert!(output.iter().all(|s| s.abs() < 1e-6), "silence came out as sound");
    }

    #[test]
    fn readings_come_every_10_ms() {
        let mut corrector = PitchCorrector::new(RATE);
        assert_eq!(corrector.frames_until_reading(), 480);
        corrector.process(&[0.0; 100], &mut [0.0; 100]);
        assert_eq!(corrector.frames_until_reading(), 380);
        assert_eq!(corrector.reading_lag_samples(), 1024);
    }
}
