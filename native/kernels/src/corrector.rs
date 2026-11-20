use std::ptr::NonNull;

use crate::ffi;
use crate::pitch::{Pitch, PitchClassMask};

/// Pulls one mono voice from 82 Hz up toward the nearest allowed pitch class. Every 5 ms it reads
/// the input, from the shortest window that can vouch for the voice; the distance from where the
/// voice is now to the nearest class in the target, scaled by the strength and eased in over the
/// retune time, becomes a transposition a time-domain shifter applies. Once the voice swings around
/// a held note, the ease slows to 120 ms, so the note's centre is corrected and the vibrato is the
/// singer's. The output trails the input by at most one period of the voice and 0.25 ms, 1.3 ms
/// when shifting six semitones up, and a rest under -50 dBFS moves it down. At strength 0 the
/// output is the input itself with no delay, and turning the strength up from or down to 0
/// crossfades over 20 ms; it reads either way. Decisions 0010, 0013 and 0021 in docs/decisions say
/// why.
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

    /// The newest reading, every 5 ms, of where the voice was at its window's centre.
    pub fn detected(&self) -> Pitch {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_detected(self.raw.as_ptr()) }
    }

    /// The newest reading to publish, one every 10 ms: the longest window's, whichever window the
    /// corrector steered from, so a host's readings stay evenly spaced and in order.
    pub fn published(&self) -> Pitch {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_published(self.raw.as_ptr()) }
    }

    /// How far a published reading's centre trails the newest sample it read: half the longest
    /// window.
    pub fn published_lag_samples(&self) -> usize {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_published_lag_samples(self.raw.as_ptr()) as usize }
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

    /// Whether it is pulling the voice now: a voice it hears, a class to pull it to, and a
    /// strength over 0. What a singer hears switch on and off.
    pub fn correcting(&self) -> bool {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_correcting(self.raw.as_ptr()) }
    }

    /// The published reading's distance to the class it is pulled toward, in semitones and in
    /// the singer's octave: what a host gets beside that reading. 0 while unvoiced, or when that
    /// reading is under the bar a voice must clear.
    pub fn published_error_semitones(&self) -> f32 {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_published_error_semitones(self.raw.as_ptr()) }
    }

    /// How far the output trails the input now; 0 while bypassed.
    pub fn latency_samples(&self) -> usize {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_latency_samples(self.raw.as_ptr()) as usize }
    }

    /// The transposition the newest output got, in semitones; 0 while bypassed.
    pub fn shift_semitones(&self) -> f32 {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_corrector_shift_semitones(self.raw.as_ptr()) }
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
    use crate::fixtures::{
        BUZZ, Lcg, at_db, harmonic_voice, midi_hz, pink_noise, rms, room, steepest_step, through,
    };
    use crate::pitch::{EVERY_PITCH_CLASS, pitch_class_of};
    use crate::test_support::{RATE, cents_between, cents_track, median_pitch};

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
                let output = corrected(
                    &mut corrector,
                    &harmonic_voice(2 * RATE as usize, &BUZZ, |_| input_hz),
                    128,
                );

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
                let input = harmonic_voice(2 * RATE as usize, &BUZZ, |_| input_hz);
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
            let output =
                corrected(&mut corrector, &harmonic_voice(2 * RATE as usize, &BUZZ, |_| flat), 441);

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
        corrected(&mut corrector, &harmonic_voice(RATE as usize / 2, &BUZZ, |_| 220.0), 128);
        assert!(corrector.correcting(), "a sung tone at strength 1");
        corrector.set_target(EVERY_PITCH_CLASS, 0.0, 40.0);
        assert!(!corrector.correcting(), "strength 0");
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
        corrected(&mut corrector, &vec![0.0; 24_000], 128);
        assert!(!corrector.correcting(), "half a second of silence");
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
            corrected(&mut corrector, &harmonic_voice(RATE as usize / 2, &BUZZ, |_| hz), 128);

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
            let voice = harmonic_voice(48_000, &BUZZ, |i| if i < step_at { hz } else { up });
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
            let voice = harmonic_voice(48_000, &BUZZ, |i| if i < step_at { from } else { to });
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
        let plain = harmonic_voice(frames, &BUZZ, |_| 220.0);
        let loud_second = harmonic_voice(frames, &[0.15, 1.0], |_| 220.0);
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
            let voice = harmonic_voice(RATE as usize, &BUZZ, |i| {
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

    /// A voice at `cents_at(seconds)` cents from A3: silent where that says `None`.
    fn voice_at(seconds: f64, cents_at: impl Fn(f64) -> Option<f64>) -> Vec<f32> {
        let a3 = midi_hz(57.0);
        let frames = (seconds * f64::from(RATE)) as usize;
        let hz = |i: usize| {
            cents_at(i as f64 / f64::from(RATE)).map_or(0.0, |c| a3 * 2f64.powf(c / 1200.0))
        };
        harmonic_voice(frames, &BUZZ, hz)
    }

    fn vibrato(depth: f64, rate: f64, seconds: f64) -> f64 {
        depth * (std::f64::consts::TAU * rate * seconds).sin()
    }

    fn mean(values: &[f64]) -> f64 {
        values.iter().sum::<f64>() / values.len() as f64
    }

    /// How far `values` swing about their mean, RMS.
    fn swing(values: &[f64]) -> f64 {
        let centre = mean(values);
        (values.iter().map(|v| (v - centre).powi(2)).sum::<f64>() / values.len() as f64).sqrt()
    }

    // #46's done-when: a voice 30 cents flat with 50 cents of vibrato at 5.5 Hz comes out with its
    // centre within 5 cents of the note and its vibrato within 10 percent of the voice's, at full
    // strength and the default 40 ms retune
    #[test]
    fn a_held_note_is_corrected_and_its_vibrato_kept() {
        let voice = voice_at(4.0, |t| Some(-30.0 + vibrato(50.0, 5.5, t)));
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(pitch_class_of(57), 1.0, 40.0);
        let output = corrected(&mut corrector, &voice, 128);

        let (sung, heard) = (
            cents_track(&voice, 1.0, 3.9, midi_hz(57.0)),
            cents_track(&output, 1.0, 3.9, midi_hz(57.0)),
        );
        assert!(mean(&heard).abs() < 5.0, "centre {:+.1} cents from the note", mean(&heard));
        let kept = swing(&heard) / swing(&sung);
        assert!((kept - 1.0).abs() < 0.1, "kept {kept:.2} of the vibrato");
    }

    // A voice whose vibrato crosses the midpoint of A3 and B flat stays on one note: every reading
    // is nearer A3 than B flat, where a target that flipped each half cycle swung it a semitone
    #[test]
    fn a_voice_on_the_midpoint_holds_one_note() {
        let voice = voice_at(4.0, |t| Some(50.0 + vibrato(30.0, 5.5, t)));
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
        let output = corrected(&mut corrector, &voice, 128);

        let heard = cents_track(&output, 1.0, 3.9, midi_hz(57.0));
        let lands_on = |cents: f64| (cents / 100.0).round();
        assert!(
            heard.iter().all(|c| lands_on(*c) == lands_on(heard[0])),
            "the note flipped: {heard:?}"
        );
        let kept = swing(&heard) / swing(&cents_track(&voice, 1.0, 3.9, midi_hz(57.0)));
        assert!((kept - 1.0).abs() < 0.1, "kept {kept:.2} of the swing");
    }

    // A note that starts with a scoop lands at the retune, as before the centre held notes: a
    // 100 cent scoop over 150 ms from silence is within 10 cents of the note 40 ms after it ends,
    // read in a 21 ms window
    #[test]
    fn a_note_that_starts_with_a_scoop_lands_at_the_retune() {
        let voice =
            voice_at(1.5, |t| (t >= 0.5).then(|| -100.0 * (1.0 - ((t - 0.5) / 0.15).min(1.0))));
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(pitch_class_of(57), 1.0, 40.0);
        let output = corrected(&mut corrector, &voice, 128);

        let heard = cents_track(&output, 0.65 + 0.040, 1.5, midi_hz(57.0));
        assert!(heard.iter().all(|c| c.abs() < 10.0), "after the scoop: {heard:?}");
    }

    // A held note with vibrato that leaps two semitones lands on the new one at the retune, not at
    // the held centre's pace: within 10 cents, on average over each 200 ms, from 150 ms after
    #[test]
    fn a_held_note_that_leaps_lands_on_the_next() {
        let voice =
            voice_at(3.0, |t| Some(if t < 1.5 { 0.0 } else { 200.0 } + vibrato(40.0, 5.5, t)));
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
        let output = corrected(&mut corrector, &voice, 128);

        let heard = cents_track(&output, 1.65, 3.0, midi_hz(59.0));
        for window in heard.chunks(20) {
            assert!(mean(window).abs() < 10.0, "centred {:+.1} cents from B3", mean(window));
        }
    }

    // A new stream on the slot starts clean: nothing of the last singer comes out after a reset
    #[test]
    fn reset_forgets_the_last_voice() {
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(pitch_class_of(57), 1.0, 0.0);
        corrected(&mut corrector, &harmonic_voice(RATE as usize, &BUZZ, |_| 215.0), 480);
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
        let output = corrected(
            &mut corrector,
            &harmonic_voice(3 * RATE as usize / 2, &BUZZ, |_| midi_hz(65.8)),
            480,
        );
        let cents = cents_between(median_pitch(&output, 0.8, 1.5), f4);
        assert!(cents.abs() < 10.0, "output {cents:+.1} cents from F4");
        let error = corrector.error_semitones();
        assert!((error + 0.8).abs() < 0.05, "error {error} semitones, expected -0.8");
    }

    // Off is the input untouched and undelayed, while the detector keeps reading
    #[test]
    fn strength_zero_is_the_input_itself() {
        let input = harmonic_voice(RATE as usize / 2, &BUZZ, |_| 220.0);
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

    /// One jump of the shifter's read point: when, how far, and how far that lands from a whole
    /// number of the voice's periods, as a fraction of one. A splice that keeps a voice whole lands
    /// near 0; 0.5 crossfades two points of the waveform half a cycle apart.
    struct Splice {
        at: usize,
        misaligned: f64,
        /// The output's level in the crossfade after the jump against the 10 ms before it, in dB.
        dip_db: f64,
    }

    /// `input` through a corrector at full strength on every class, a frame at a time, under the
    /// allocation guard: the output, and the delay after each frame.
    fn walked(input: &[f32]) -> (Vec<f32>, Vec<f64>) {
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
        let mut output = vec![0.0; input.len()];
        let mut delays = Vec::with_capacity(input.len());
        assert_no_alloc(|| {
            for (sample, out) in input.iter().zip(output.iter_mut()) {
                let mut one = [0.0];
                corrector.process(std::slice::from_ref(sample), &mut one);
                *out = one[0];
                delays.push(corrector.latency_samples() as f64);
            }
        });
        (output, delays)
    }

    /// Every jump of the read point in `input` through `walked`, with `hz_at` the voice's true
    /// pitch at each sample.
    fn splices(input: &[f32], hz_at: impl Fn(usize) -> f64) -> Vec<Splice> {
        let (output, delays) = walked(input);
        let level = |from: usize, to: usize| {
            let slice = &output[from..to.min(output.len())];
            (slice.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / slice.len() as f64).sqrt()
        };
        (1..delays.len())
            .filter(|&n| {
                (delays[n] - delays[n - 1]).abs() >= 4.0 && n > 480 && n + 128 < delays.len()
            })
            .map(|n| {
                let jump = (delays[n] - delays[n - 1]).abs();
                // Judged against the voice at both ends, so a splice across a change of note is
                // read in the way that is kindest to it
                let misaligned = [delays[n - 1], delays[n]]
                    .map(|delay| {
                        let period = f64::from(RATE) / hz_at(n.saturating_sub(delay as usize));
                        (jump - (jump / period).round() * period).abs() / period
                    })
                    .into_iter()
                    .fold(f64::INFINITY, f64::min);
                let dip_db = 20.0 * (level(n, n + 128) / level(n - 480, n).max(1e-9)).log10();
                Splice { at: n, misaligned, dip_db }
            })
            .collect()
    }

    fn report(name: &str, splices: &[Splice], seconds: f64) -> usize {
        let bad: Vec<&Splice> = splices.iter().filter(|s| s.misaligned > 0.1).collect();
        let worst = splices.iter().map(|s| s.misaligned).fold(0.0, f64::max);
        let dip = bad.iter().map(|s| s.dip_db).fold(0.0, f64::min);
        eprintln!(
            "{name}: {} jumps, {} misaligned past a tenth of a period ({:.1} a second), worst {worst:.2} of a period, deepest dip after one {dip:.1} dB, first at {:?}",
            splices.len(),
            bad.len(),
            bad.len() as f64 / seconds,
            bad.first().map(|s| s.at)
        );
        bad.len()
    }

    // A bass near E2 with vibrato dips under the 82 Hz the reader corrects from on every cycle,
    // and the corrector lets go there. The voice still sounds, so the shifter must keep it whole
    #[test]
    fn a_low_voice_dipping_under_the_reader_is_spliced_whole() {
        let seconds = 3.0;
        for (centre, depth) in [(40.0, 50.0), (40.2, 40.0)] {
            let hz = |i: usize| {
                let cents = vibrato(depth, 5.5, i as f64 / f64::from(RATE));
                midi_hz(centre) * 2f64.powf(cents / 1200.0)
            };
            let voice = harmonic_voice((seconds * f64::from(RATE)) as usize, &BUZZ, hz);
            let name = format!("MIDI {centre} with {depth} c vibrato");
            assert_eq!(report(&name, &splices(&voice, hz), seconds), 0, "{name}");
        }
    }

    // A breathy note: noise as loud as the voice comes over it for 75 ms in every 300, long enough
    // for the reader to stop trusting it. The voice sounds throughout and must stay whole
    #[test]
    fn a_breathy_note_the_reader_doubts_is_spliced_whole() {
        let seconds = 3.0;
        let hz = midi_hz(50.3);
        let frames = (seconds * f64::from(RATE)) as usize;
        let mut rng = Lcg::new(0x2545_f491);
        let noise: Vec<f32> = (0..frames).map(|_| 0.45 * rng.sample()).collect();
        let voice: Vec<f32> = harmonic_voice((seconds * f64::from(RATE)) as usize, &BUZZ, |_| hz)
            .into_iter()
            .enumerate()
            .map(|(i, sample)| {
                let breathing = i % (RATE as usize * 3 / 10) < RATE as usize * 3 / 40;
                sample + if breathing { noise[i] } else { 0.0 }
            })
            .collect();
        let bad = report("breathy D3", &splices(&voice[..frames], |_| hz), seconds);
        assert_eq!(bad, 0);
    }

    /// The shifter's quietest audible level, -50 dBFS, as RMS over the three crossfades it judges.
    const QUIETEST: f32 = 0.003_16;

    /// Whether the 384 samples of `signal` before `end` are under the shifter's level floor.
    fn quiet(signal: &[f32], end: usize) -> bool {
        rms(&signal[end.saturating_sub(384)..end]) < QUIETEST
    }

    /// White noise at `db` dBFS RMS: a quiet room, or bleed.
    fn room_noise(frames: usize, db: f32) -> Vec<f32> {
        let mut rng = Lcg::new(0x00c0_ffee);
        at_db((0..frames).map(|_| rng.sample()).collect(), db)
    }

    /// A line on A3 `cents` off, a rest from 0.6 to 1.0 s holding `rest`, then the same line again
    /// from 1.0 s with a 5 ms onset, as a singer's next line starts.
    fn line_rest_line(cents: f64, rest: &[f32]) -> Vec<f32> {
        let rate = f64::from(RATE);
        let (rest_at, line_at) = ((0.6 * rate) as usize, (1.0 * rate) as usize);
        let hz = midi_hz(57.0 + cents / 100.0);
        harmonic_voice((1.6 * rate) as usize, &BUZZ, |_| hz)
            .into_iter()
            .enumerate()
            .map(|(i, sample)| match i {
                _ if i < rest_at => sample,
                _ if i < line_at => rest[i - rest_at],
                _ => sample * ((i - line_at) as f32 / 240.0).min(1.0),
            })
            .collect()
    }

    /// How many rest moves `delays` makes between frames `from` and `to`: jumps down that land at
    /// the resting point, a quarter of the voice's `period` above the shifter's floor. A rest move
    /// splices `input` under the floor at both ends, the read point it left and the one it landed
    /// on, so one that splices sound fails here.
    fn rest_moves(input: &[f32], delays: &[f64], period: f64, from: usize, to: usize) -> usize {
        (from.max(1)..to)
            .filter(|&n| delays[n - 1] - delays[n] >= 2.0)
            .filter(|&n| {
                [5.0, 8.0].iter().any(|floor| (delays[n] - floor - 0.25 * period).abs() <= 1.5)
            })
            .inspect(|&n| {
                let (read, landing) = (n - delays[n - 1] as usize, n - delays[n] as usize);
                assert!(quiet(input, read) && quiet(input, landing), "a move at {n} spliced sound");
            })
            .count()
    }

    // A rest under -50 dBFS keeps the read point at its resting point for as long as it lasts, so
    // it may move many times in one rest. None of it is heard: every move splices input under the
    // floor at both ends, the output stays under it through the rest, and the next line's first
    // period is whole. A voice shifted up, whose read point closes on the floor as the shift eases
    // in the rest and jumps a period back, is the case that once left a line starting high
    #[test]
    fn a_rest_move_is_never_heard() {
        let rate = f64::from(RATE);
        let (rest_at, line_at) = ((0.6 * rate) as usize, (1.0 * rate) as usize);
        let rest = room_noise(line_at - rest_at, -60.0);
        for cents in [-40.0, 40.0] {
            let input = line_rest_line(cents, &rest);
            let (output, delays) = walked(&input);
            let period = rate / midi_hz(57.0 + cents / 100.0);
            let moves = rest_moves(&input, &delays, period, rest_at, line_at);
            assert!(moves > 0, "{cents:+} c: the rest never moved the delay");
            // Shifted down, the easing shift lets the delay climb all through the rest, and the
            // rest keeps taking it back
            if cents > 0.0 {
                assert!(moves > 1, "{cents:+} c: one move in a rest the delay climbs through");
            }
            for end in (rest_at + 1_200..line_at).step_by(128) {
                assert!(quiet(&output, end), "{cents:+} c: the rest is heard at {end}");
            }
            let starts = line_at + delays[line_at] as usize;
            let first = starts..starts + period as usize;
            assert!(
                delays[first.clone()].windows(2).all(|d| (d[1] - d[0]).abs() < 2.0),
                "{cents:+} c: a splice in the line's first period"
            );
            let dry = steepest_step(&input[line_at..line_at + period as usize]);
            let wet = steepest_step(&output[first]);
            assert!(wet <= 1.5 * dry, "{cents:+} c: a step of {wet} where the voice has {dry}");
            assert!(
                delays[line_at] < 5.0 + 0.25 * period + 8.0,
                "{cents:+} c: the line starts at {}",
                delays[line_at]
            );
        }
    }

    // The room is rarely silent. Bleed 2 dB over the floor through the whole rest holds the delay
    // where the line left it, as decision 0024's row says; 2 dB under, the rest moves it. Bleed
    // that arrives halfway through reaches the newest end of a splice first, and from then on no
    // move is made. A rest shorter than a crossfade is never judged a rest
    #[test]
    fn the_room_is_not_silent() {
        let rate = f64::from(RATE);
        let (rest_at, line_at) = ((0.6 * rate) as usize, (1.0 * rate) as usize);
        let frames = line_at - rest_at;
        let period = rate / midi_hz(57.4);
        let moves_with = |rest: &[f32]| {
            let input = line_rest_line(40.0, rest);
            let (_, delays) = walked(&input);
            rest_moves(&input, &delays, period, rest_at, line_at)
        };
        assert_eq!(
            moves_with(&room_noise(frames, -48.0)),
            0,
            "bleed over the floor moved the delay"
        );
        assert!(
            moves_with(&room_noise(frames, -52.0)) > 0,
            "a rest under the floor did not move it"
        );
        let mut arriving = vec![0.0; frames / 2];
        arriving.extend(room_noise(frames - frames / 2, -48.0));
        assert!(moves_with(&arriving) > 0, "the quiet half did not move the delay");

        let gap = 96;
        let input: Vec<f32> = harmonic_voice((1.2 * rate) as usize, &BUZZ, |_| midi_hz(57.4))
            .into_iter()
            .enumerate()
            .map(|(i, sample)| if (rest_at..rest_at + gap).contains(&i) { 0.0 } else { sample })
            .collect();
        let (_, delays) = walked(&input);
        assert!(
            rest_moves(&input, &delays, period, rest_at, rest_at + 2_000) == 0,
            "a 2 ms gap moved the delay"
        );
    }

    // A rest still moves the read point down: a line that starts after one starts near the floor
    #[test]
    fn a_rest_still_moves_the_delay_down() {
        let line = voice_at(2.0, |t| if (0.8..1.2).contains(&t) { None } else { Some(-40.0) });
        let mut corrector = PitchCorrector::new(RATE);
        corrector.set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
        let mut at_line_start = 0;
        for (index, block) in line.chunks(128).enumerate() {
            corrector.process(block, &mut [0.0; 128][..block.len()]);
            if (1.19..1.2).contains(&((index * 128) as f64 / f64::from(RATE))) {
                at_line_start = corrector.latency_samples();
            }
        }
        let bound = 5.0 + 0.25 * f64::from(RATE) / midi_hz(56.6) + 8.0;
        assert!((at_line_start as f64) < bound, "{at_line_start} samples at the line's start");
    }

    /// What makes the reader doubt a voice in the sweep, from `at` frames on or throughout.
    #[derive(Clone, Copy, Debug)]
    enum Doubt {
        Breath { db: f32 },
        Bleed { db: f32, throughout: bool },
    }

    /// A karaoke room's backing as the phone hears it: a detuned chord on other notes over a
    /// drum-like bed, through a small room.
    fn bleed(frames: usize) -> Vec<f32> {
        let chord = [48.07, 55.0, 63.9]
            .map(|midi| harmonic_voice(frames, &BUZZ, |_| midi_hz(midi)))
            .into_iter()
            .fold(vec![0.0f32; frames], |mix, tone| {
                mix.iter().zip(&tone).map(|(a, b)| a + b).collect()
            });
        let bed = pink_noise(frames, 0x00be_d5ed);
        let backing: Vec<f32> = chord.iter().zip(&bed).map(|(c, b)| c + 2.0 * b).collect();
        through(&backing, &room(1_440, 1.0, 0.4, 0.3, 0x0000_7007))
    }

    // The class r4630074211 and r4630816952 named: no splice of the read point may cut a sounding
    // voice out of phase, whatever note it has moved to while the reader doubts it. Notes across
    // the corrector's range leap a second, a third, a fifth and an octave either way, 40 cents off,
    // under breath and under a backing's bleed at several levels. The rest move cuts none of them:
    // it moves only under the level floor. What is left are period jumps taken on the reader's
    // last period after the voice moved under bleed, 15 of these 366 cases with the worst 0.44 of a
    // period, where `main`'s corrector leaves 11; the count may only go down
    #[test]
    fn no_splice_cuts_a_sounding_voice_that_changed_note_under_doubt() {
        let rate = f64::from(RATE);
        let frames = (1.6 * rate) as usize;
        let (doubt_at, leap_at) = ((0.5 * rate) as usize, (0.7 * rate) as usize);
        let doubts = [
            Doubt::Breath { db: 0.0 },
            Doubt::Breath { db: -6.0 },
            Doubt::Bleed { db: -12.0, throughout: false },
            Doubt::Bleed { db: -6.0, throughout: false },
            Doubt::Bleed { db: 0.0, throughout: false },
            Doubt::Bleed { db: -6.0, throughout: true },
        ];
        let bleed = bleed(frames);
        let (mut cases, mut failing, mut bad, mut worst) = (0, Vec::new(), 0, 0.0f64);
        for (index, from) in
            [40.0, 45.0, 50.0, 55.0, 57.0, 62.0, 67.0, 70.0].into_iter().enumerate()
        {
            for interval in [2.0, -2.0, 4.0, -4.0, 7.0, -7.0, 12.0, -12.0] {
                let to = from + interval;
                if !(36.0..=84.0).contains(&to) {
                    continue;
                }
                let off = if index % 2 == 0 { 0.4 } else { -0.4 };
                let hz = |i: usize| midi_hz(if i < leap_at { from } else { to } + off);
                let voice = harmonic_voice(frames, &BUZZ, hz);
                let level = rms(&voice);
                for doubt in doubts {
                    let (noise, start): (Vec<f32>, usize) = match doubt {
                        Doubt::Breath { db } => {
                            let mut rng = Lcg::new(0x2545_f491);
                            let white: Vec<f32> = (0..frames).map(|_| rng.sample()).collect();
                            (at_db(white, 20.0 * level.log10() + db), doubt_at)
                        }
                        Doubt::Bleed { db, throughout } => {
                            let start = if throughout { 0 } else { doubt_at };
                            (at_db(bleed.clone(), 20.0 * level.log10() + db), start)
                        }
                    };
                    let input: Vec<f32> = voice
                        .iter()
                        .zip(&noise)
                        .enumerate()
                        .map(|(i, (v, n))| {
                            let fade = (i.saturating_sub(start) as f32 / 960.0).min(1.0);
                            v + if i >= start { fade * n } else { 0.0 }
                        })
                        .collect();
                    cases += 1;
                    let wrong: Vec<f64> = splices(&input, hz)
                        .iter()
                        .map(|s| s.misaligned)
                        .filter(|m| *m > 0.1)
                        .collect();
                    if !wrong.is_empty() {
                        bad += wrong.len();
                        worst = wrong.iter().copied().fold(worst, f64::max);
                        failing.push(format!("{from}->{to} {doubt:?}: {}", wrong.len()));
                    }
                }
            }
        }
        eprintln!(
            "SWEEP {cases} cases, {} failing, {bad} misaligned splices, worst {worst:.2} of a period",
            failing.len()
        );
        for case in &failing {
            eprintln!("SWEEP FAIL {case}");
        }
        assert!(failing.len() <= 15, "{} of {cases} cases cut a sounding voice", failing.len());
    }
}
