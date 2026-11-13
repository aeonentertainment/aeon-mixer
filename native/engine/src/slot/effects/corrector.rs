use kernels::{EVERY_PITCH_CLASS, PitchCorrector};

use crate::block::{BLOCK, RATE};
use crate::params::CorrectorParams;
use crate::slot::effects::{Reading, Target, melody};
use crate::slot::guide::Guide;

/// With no guide every class is a target, so a voice goes to its nearest semitone.
const CHROMATIC: Target = Target { mask: EVERY_PITCH_CLASS, midi: None };

/// The slot's pitch corrector, following the song's melody guide. It starts from Aeon Karaoke's
/// `aeonpitch` element, ported off GStreamer; how it reads pitch is ours, decision 0010. It splits
/// a run at every note edge, so the target changes on
/// the sample the guide names, and at every reading the kernel has to publish. The slot owns the
/// guide and lends it to each `process`.
pub struct Corrector {
    kernel: PitchCorrector,
    /// The kernel reads and writes separate buffers; the slot's block is copied through this.
    scratch: [f32; BLOCK],
}

impl Corrector {
    /// The most `latency_frames` can read, 15 ms: the shifter's floor and 1.125 periods of the
    /// lowest voice it corrects, 82 Hz. It does not move with the voice, so it is what a latency
    /// budget counts.
    pub const MAX_LATENCY_FRAMES: u32 = 720;

    pub fn new() -> Self {
        Self { kernel: PitchCorrector::new(RATE), scratch: [0.0; BLOCK] }
    }

    /// Forgets the singer it was hearing, for a new stream on the slot.
    pub fn reset(&mut self) {
        self.kernel.reset();
    }

    /// How far the voice trails its input now; 0 with correction off.
    pub fn latency_frames(&self) -> usize {
        self.kernel.latency_samples()
    }

    /// Whether it is pulling the voice now.
    pub fn correcting(&self) -> bool {
        self.kernel.correcting()
    }

    /// One run of the slot's voice, in place, following `guide` when there is one. `frame` is
    /// where its first sample sits on the guide's clock, which docs/design.md section 3 derives
    /// from the playhead, and `semitones` the backing's key shift, which the guide follows.
    /// `on_reading` hears every 10 ms reading.
    pub fn process(
        &mut self,
        params: &CorrectorParams,
        guide: Option<&Guide>,
        frame: i64,
        semitones: i8,
        block: &mut [f32],
        mut on_reading: impl FnMut(Reading),
    ) {
        let strength = if params.enabled { params.strength } else { 0.0 };
        let correcting = strength > 0.0;
        let mut done = 0;
        while done < block.len() {
            let at = frame + done as i64;
            let (target, next_edge) = match guide {
                Some(guide) => {
                    (melody::target_at(guide, at, semitones), melody::next_edge_after(guide, at))
                }
                None => (CHROMATIC, None),
            };
            self.kernel.set_target(target.mask, strength, params.retune_ms);

            let until_reading = self.kernel.frames_until_reading();
            let mut frames = (block.len() - done).min(until_reading).min(BLOCK);
            if let Some(edge) = next_edge {
                // An edge is always after `at`, so a run is never empty
                frames = frames.min(usize::try_from(edge - at).unwrap_or(usize::MAX));
            }
            let run = &mut block[done..done + frames];
            self.scratch[..frames].copy_from_slice(run);
            self.kernel.process(&self.scratch[..frames], run);
            done += frames;

            if frames == until_reading {
                let pitch = self.kernel.published();
                let centre = at + frames as i64 - self.kernel.published_lag_samples() as i64;
                let position_ms = centre as f64 * 1000.0 / f64::from(RATE);
                // The kernel read with the target this run set, so the two belong together
                on_reading(Reading {
                    position_ms,
                    hz: pitch.hz,
                    confidence: pitch.confidence,
                    target_midi: target.midi.filter(|_| correcting),
                    error_semitones: if correcting {
                        self.kernel.published_error_semitones()
                    } else {
                        0.0
                    },
                });
            }
        }
    }
}

impl Default for Corrector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::PI;

    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::slot::guide::Note;

    const ON: CorrectorParams = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };
    const C_MAJOR: u16 = 0b1010_1011_0101;

    /// Twelve harmonics at `midi`, the buzz of a sung vowel.
    fn tone(midi: f64, frames: usize) -> Vec<f32> {
        let hz = 440.0 * 2f64.powf((midi - 69.0) / 12.0);
        (0..frames)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                let sample: f64 =
                    (1..=12).map(|h| (2.0 * PI * hz * h as f64 * t).sin() / h as f64).sum();
                (0.25 * sample) as f32
            })
            .collect()
    }

    // The done-when: a voice between A and B flat is pulled to A in C major's key and to B flat
    // while the guide's B flat sounds. Each block is checked after it renders, so the block that
    // holds an edge must already have turned to the new target.
    #[test]
    fn the_target_changes_within_one_block_of_each_edge() {
        let (start, end) = (30_000 + 37, 60_000 + 101);
        let note = Note { midi: 70, start_frame: start, end_frame: end };
        let guide = Guide { notes: vec![note], key_mask: C_MAJOR };
        let mut corrector = Corrector::new();

        let mut voice = tone(69.3, 90_000);
        let mut errors = Vec::with_capacity(voice.len() / BLOCK);
        assert_no_alloc(|| {
            for (index, block) in voice.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                let frame = (index * BLOCK) as i64;
                corrector.process(&ON, Some(&guide), frame, 0, block, |_| {});
                errors.push((frame as u64, corrector.kernel.error_semitones()));
            }
        });

        // Skip the first 50 ms, before the detector has heard the voice
        for (first, error) in errors.into_iter().filter(|(first, _)| *first >= 2400) {
            let last = first + BLOCK as u64 - 1;
            let toward_b_flat = (start..end).contains(&last);
            let expected = if toward_b_flat { 0.7 } else { -0.3 };
            assert!(
                (error - expected).abs() < 0.05,
                "block at {first}: error {error}, expected {expected}"
            );
        }
    }

    // Readings every 10 ms, each placed at the centre of the window it read. The corrector steers
    // from a short window on A3 and A4 and from the longest through the pause between them; what it
    // publishes is the longest window's either way, so a reading never lands out of order
    #[test]
    fn readings_come_every_10_ms_at_their_centre() {
        let mut corrector = Corrector::new();
        let mut voice = tone(57.0, 19_200);
        voice.resize(24_000, 0.0);
        voice.extend(tone(69.0, RATE as usize - 24_000));
        let mut readings = Vec::with_capacity(200);
        let start = 4_800;
        assert_no_alloc(|| {
            for (index, block) in voice.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                corrector.process(&ON, None, start + (index * BLOCK) as i64, 0, block, |r| {
                    readings.push(r)
                });
            }
        });

        // 375 blocks of 128 frames hold 100 hops of 480
        assert_eq!(readings.len(), 100);
        let lag_ms = 1024.0 / 48.0;
        assert!((readings[0].position_ms - (100.0 + 10.0 - lag_ms)).abs() < 1e-9);
        for pair in readings.windows(2) {
            assert!((pair[1].position_ms - pair[0].position_ms - 10.0).abs() < 1e-9);
        }
        let heard = readings.last().unwrap();
        assert!(
            (heard.hz - 440.0).abs() < 0.5 && heard.confidence > 0.9,
            "read {heard:?} for 440 Hz"
        );
    }

    // A reading's error is against that reading: on a 50 cent vibrato at 5.5 Hz, where a short
    // window and the published one read the voice 16 ms apart, every sure reading's error is the
    // guide note less its own pitch, in the singer's octave, to within a thousandth of a semitone
    #[test]
    fn a_readings_error_is_against_that_reading() {
        let guide = Guide {
            notes: vec![Note { midi: 57, start_frame: 0, end_frame: u64::from(RATE) * 2 }],
            key_mask: 0,
        };
        let mut corrector = Corrector::new();
        let mut phase = 0.0f64;
        let mut voice: Vec<f32> = (0..RATE as usize)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                let midi = 57.2 + 0.5 * (2.0 * PI * 5.5 * t).sin();
                phase += 440.0 * 2f64.powf((midi - 69.0) / 12.0) / f64::from(RATE);
                let sample: f64 =
                    (1..=12).map(|h| (2.0 * PI * h as f64 * phase).sin() / h as f64).sum();
                (0.25 * sample) as f32
            })
            .collect();
        let mut readings = Vec::with_capacity(100);
        assert_no_alloc(|| {
            for (index, block) in voice.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                let frame = (index * BLOCK) as i64;
                corrector.process(&ON, Some(&guide), frame, 0, block, |r| readings.push(r));
            }
        });
        let sure: Vec<_> = readings.iter().filter(|r| r.confidence >= 0.9).collect();
        assert!(sure.len() > 80, "{} sure readings", sure.len());
        for reading in sure {
            let target = f32::from(reading.target_midi.expect("a guide note"));
            let against = target - (69.0 + 12.0 * (reading.hz / 440.0).log2());
            let folded = against - 12.0 * (against / 12.0).round();
            assert!(
                (reading.error_semitones - folded).abs() < 1e-3,
                "at {:.0} ms: error {} for a reading {} off",
                reading.position_ms,
                reading.error_semitones,
                folded
            );
        }
    }

    // With no guide the class a reading is pulled to is the nearest to that reading, so across a
    // clean step from C4 to D4, in tune on both, no sure reading's error is over 0.7 semitones,
    // though the corrector steers by a newer reading than the one it publishes. Taking the class
    // from the steering reading gave 1.93 here, with the step 60 frames into a hop
    #[test]
    fn a_step_with_no_guide_keeps_each_errors_class_on_its_reading() {
        let mut corrector = Corrector::new();
        let step = RATE as usize / 2 + 60;
        let mut phase = 0.0f64;
        let mut voice: Vec<f32> = (0..RATE as usize)
            .map(|i| {
                let midi = if i < step { 60.0 } else { 62.0 };
                phase += 440.0 * 2f64.powf((midi - 69.0) / 12.0) / f64::from(RATE);
                let sample: f64 =
                    (1..=12).map(|h| (2.0 * PI * h as f64 * phase).sin() / h as f64).sum();
                (0.25 * sample) as f32
            })
            .collect();
        let mut readings = Vec::with_capacity(100);
        assert_no_alloc(|| {
            for (index, block) in voice.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                let frame = (index * BLOCK) as i64;
                corrector.process(&ON, None, frame, 0, block, |r| readings.push(r));
            }
        });
        let worst = readings
            .iter()
            .filter(|r| r.confidence >= 0.9)
            .map(|r| r.error_semitones.abs())
            .fold(0.0, f32::max);
        assert!(worst <= 0.7, "a sure reading's error reached {worst} semitones across the step");
    }

    // The ceiling a host budgets with holds on the lowest voice the corrector takes, pulled as far
    // as a shift goes each way, and it is close: a bound nothing reaches would overstate the cost.
    #[test]
    fn the_lowest_voice_stays_under_the_ceiling_at_any_shift() {
        let mut highest = 0;
        for target in [34, 37, 39, 41, 43, 46] {
            let guide = Guide {
                notes: vec![Note { midi: target, start_frame: 0, end_frame: 96_000 }],
                key_mask: 0,
            };
            let mut corrector = Corrector::new();
            let mut voice = tone(40.04, 96_000);
            for (index, block) in voice.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                let frame = (index * BLOCK) as i64;
                assert_no_alloc(|| corrector.process(&ON, Some(&guide), frame, 0, block, |_| {}));
                highest = highest.max(corrector.latency_frames() as u32);
            }
        }
        assert!(
            (Corrector::MAX_LATENCY_FRAMES - 48..=Corrector::MAX_LATENCY_FRAMES).contains(&highest),
            "the delay reached {highest} frames against {}",
            Corrector::MAX_LATENCY_FRAMES
        );
    }

    // Off is the singer's own voice, with no delay, and the readings still come
    #[test]
    fn off_is_the_voice_itself_and_still_reads() {
        let mut corrector = Corrector::new();
        let input = tone(69.4, 120 * BLOCK);
        let mut voice = input.clone();
        let mut read = 0;
        let off = CorrectorParams { enabled: false, ..ON };
        assert_no_alloc(|| {
            for (index, block) in voice.as_chunks_mut::<BLOCK>().0.iter_mut().enumerate() {
                corrector.process(&off, None, (index * BLOCK) as i64, 0, block, |_| read += 1);
            }
        });
        assert_eq!(voice, input);
        assert_eq!(corrector.latency_frames(), 0);
        // 120 blocks of 128 frames hold 32 hops of 480
        assert_eq!(read, 32);
    }
}
