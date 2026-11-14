use std::ptr::NonNull;

use crate::ffi;
use crate::pitch::{Pitch, PitchClassMask};

/// Pulls one mono voice from 82 Hz up toward the nearest allowed pitch class. Every 10 ms the
/// detector reads the input; the distance to the nearest class in the target, scaled by the
/// strength and eased in over the retune time, becomes a transposition a time-domain shifter
/// applies. The output trails the input by at most one period of the voice and 0.25 ms, 1.3 ms
/// when shifting six semitones up. At strength 0 the output is the input itself with no delay,
/// and turning the strength up from or down to 0 crossfades over 20 ms; the detector reads either
/// way.
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

    /// The newest detector reading.
    pub fn detected(&self) -> Pitch {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_detected(self.raw.as_ptr()) }
    }

    /// Frames until the next reading. A host that publishes every reading processes at most this
    /// many at a time.
    pub fn frames_until_reading(&self) -> usize {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_frames_until_reading(self.raw.as_ptr()) as usize }
    }

    /// How far a reading's centre trails the newest sample it read: half the detector's window.
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

    /// Whether it is pulling the voice now: a voice it hears, a class to pull it to, and a
    /// strength over 0. What a singer hears switch on and off.
    pub fn correcting(&self) -> bool {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_correcting(self.raw.as_ptr()) }
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
    use crate::test_support::{RATE, cents_between, harmonic_tone, median_pitch, midi_hz};

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
                // The shifter's floor at this ratio, and one period of the voice
                let ratio = 2f64.powf(shift / 12.0);
                let bound = (5.0 + ((ratio - 1.0).max(0.0) * 128.0).ceil() + rate / input_hz + 2.0)
                    as usize;
                let latency = corrector.latency_samples();
                assert!(
                    latency > 0 && latency <= bound,
                    "latency {latency} at {input_hz:.1} Hz, bound {bound}"
                );
            }
        }
    }

    // The shifter trails by one period of the voice and the room a jump's crossfade needs, and no
    // more: a voice held 30 cents either side of a note, from 82 to 880 Hz, never trails by more
    // than 5 samples, what a shift up gains on the input during a 128-sample crossfade, and one
    // period and two samples. The shifter before #112 held room for six semitones whatever the
    // shift and let a jump overshoot by an eighth of a period: it trailed the 83.8 Hz voice by 702
    // samples, against 580 here.
    #[test]
    fn a_held_voice_trails_by_at_most_one_period_and_the_crossfades_room() {
        let rate = f64::from(RATE);
        for target in [40, 45, 57, 69, 81] {
            for cents in [30.0, -30.0] {
                let input_hz = midi_hz(f64::from(target) + cents / 100.0);
                if input_hz < 82.0 {
                    continue;
                }
                let mut corrector = PitchCorrector::new(RATE);
                corrector.set_target(EVERY_PITCH_CLASS, 1.0, 0.0);
                let input = harmonic_tone(input_hz, 2.0);
                let mut output = vec![0.0; input.len()];
                let mut most = 0;
                assert_no_alloc(|| {
                    for (input, output) in input.chunks(128).zip(output.chunks_mut(128)) {
                        corrector.process(input, output);
                        most = most.max(corrector.latency_samples());
                    }
                });

                let ratio = 2f64.powf(-cents / 1200.0);
                let crossfade_room = ((ratio - 1.0).max(0.0) * 128.0).ceil();
                let bound = 5.0 + crossfade_room + rate / input_hz + 2.0;
                assert!(
                    most as f64 <= bound,
                    "{input_hz:.1} Hz trailed by {most} samples, bound {bound:.1}"
                );
                let cents_out =
                    cents_between(median_pitch(&output, 1.0, 2.0), midi_hz(target.into()));
                assert!(
                    cents_out.abs() < 5.0,
                    "{input_hz:.1} Hz came out {cents_out:+.1} cents off"
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

    // It is correcting while it hears a voice and pulls it, and not at strength 0 or once the voice
    // has stopped
    #[test]
    fn correcting_is_a_heard_voice_and_a_pull() {
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
        corrected(&mut corrector, &harmonic_tone(220.0, 0.5), 128);
        assert!(corrector.correcting(), "a sung tone at strength 1");
        corrector.set_target(EVERY_PITCH_CLASS, 0.0, 40.0);
        assert!(!corrector.correcting(), "strength 0");
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
        corrected(&mut corrector, &vec![0.0; 24_000], 128);
        assert!(!corrector.correcting(), "half a second of silence");
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
