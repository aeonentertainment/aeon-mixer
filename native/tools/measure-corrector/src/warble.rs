//! What correction does to a held note's movement at the rates a wobble is heard at. A note is one
//! the clip's annotation names, read past its attack and before its release as every note row
//! reads it, with the corrected voice set back onto the input's clock by the delay the slot
//! reported. One note has one pitch meant, so nothing here reads across a change of note.
//!
//! - **Pitch moved**: the output's pitch less the input's, in the band. Correction that follows
//!   the singer moves it by design, so this is the size of what correction does there.
//! - **Pitch movement kept**: the output's own movement in the band against the singer's, as a
//!   ratio. Under 1, correction took some of the singer's movement out; over 1, it put in movement
//!   the singer did not sing.
//!
//! A note is read only in a band its length resolves: three cycles of the band's lowest rate. So
//! there are two bands, each named in the rows: 3 to 12 Hz over notes read for a second, and 6 to
//! 12 Hz over notes read for half a second, which most sung notes are nearer to.
//!
//! They are measured against known voices, not against listeners. README.md beside this crate
//! says what they cannot tell.

use serde_json::{Value, json};

use crate::contour::{self, Point};
use crate::expression::{SLIP_CENTS, WARBLE_BAND, band_rms, bridged, note_contour, note_readings};
use crate::music::{median, percentile};
use crate::sung_clip::MeantNote;
use crate::take_delay::TakeDelay;

/// Short enough to keep 90 percent of a 12 Hz movement, long enough for an 82 Hz voice.
const PITCH_WINDOW: usize = 1024;
/// The whole band a wobble is heard in, and its upper half, which a shorter note still resolves.
const BANDS: [(&str, (f64, f64)); 2] =
    [("3_to_12_hz", WARBLE_BAND), ("6_to_12_hz", (6.0, WARBLE_BAND.1))];
/// A note is read in a band once it holds this many cycles of the band's lowest rate.
const CYCLES: f64 = 3.0;
/// The readings come every 10 ms.
const READINGS_PER_SECOND: f64 = 100.0;
/// A p95 needs this many notes; with fewer, the most a note read is given instead.
const FOR_A_P95: usize = 20;
/// Less movement in the band than this, and the singer held the note still.
const STILLEST_CENTS: f64 = 1.0;

/// Every warble row for a take, by band: `dry` sung over `notes`, `wet` what the slot made of it,
/// `delays` per block.
pub fn measure(dry: &[f32], wet: &[f32], delays: &[u32], notes: &[MeantNote]) -> Value {
    let sung = contour::read_with_window(dry, PITCH_WINDOW, 0);
    let mut heard =
        TakeDelay::new(delays).onto_input(&contour::read_with_window(wet, PITCH_WINDOW, 0));
    heard.sort_by(|a, b| a.ms.total_cmp(&b.ms));
    // Each note's readings, and what could be read of it
    let held: Vec<(usize, Option<_>)> = notes
        .iter()
        .map(|note| (note_readings(&sung, note).len(), note_and_shift(&sung, &heard, note)))
        .collect();

    let spread = |values: &[f64]| {
        let p95 = (values.len() >= FOR_A_P95).then(|| percentile(values, 0.95));
        let most = values.iter().copied().fold(f64::NAN, f64::max);
        json!({ "median": median(values), "p95": p95, "most": most })
    };
    let mut rows = serde_json::Map::new();
    for (name, (low, high)) in BANDS {
        let in_band = |series: &[f64]| band_rms(series, READINGS_PER_SECOND, low, high);
        let (mut moved, mut kept) = (Vec::new(), Vec::new());
        let resolved: Vec<_> = held
            .iter()
            .filter(|(readings, _)| *readings as f64 >= CYCLES * READINGS_PER_SECOND / low)
            .map(|(_, read)| read)
            .collect();
        for (sung, shift) in resolved.iter().copied().flatten() {
            moved.push(in_band(shift));
            let out: Vec<f64> = sung.iter().zip(shift).map(|(d, s)| d + s).collect();
            // A note held still has no movement of its own to keep
            if in_band(sung) >= STILLEST_CENTS {
                kept.push(in_band(&out) / in_band(sung));
            }
        }
        rows.insert(
            name.to_owned(),
            json!({
                "notes": moved.len(),
                "refused": resolved.len() - moved.len(),
                "pitch_moved_cents": spread(&moved),
                "pitch_movement_kept": spread(&kept),
            }),
        );
    }
    Value::Object(rows)
}

/// A note's sung contour, and the corrected pitch less the sung at each of its readings, both in
/// cents with the detector's slips bridged; `None`, the note refused, when under four fifths of
/// either can be read. A slip is the same thing on either: `SLIP_CENTS` from the note's median,
/// well past a step between scale notes, so a target the corrector changes mid-note is kept.
fn note_and_shift(
    sung: &[Point],
    heard: &[Point],
    note: &MeantNote,
) -> Option<(Vec<f64>, Vec<f64>)> {
    let shift: Vec<Option<f64>> = note_readings(sung, note)
        .iter()
        .map(|point| Some(100.0 * (at(heard, point.ms).filter(|_| point.voiced())? - point.midi())))
        .collect();
    Some((note_contour(sung, note)?, bridged(&shift, SLIP_CENTS)?))
}

/// The output's pitch at `ms` on the input's clock, from the two readings around it, as MIDI.
/// Two readings 10 ms apart that disagree by more than a slip are one of the detector's, and a
/// blend of them would sit anywhere between, so there is no reading there.
fn at(heard: &[Point], ms: f64) -> Option<f64> {
    let after = heard.partition_point(|point| point.ms < ms);
    let (before, next) = (heard.get(after.checked_sub(1)?)?, heard.get(after)?);
    let apart = 100.0 * (next.midi() - before.midi()).abs();
    if !before.voiced() || !next.voiced() || next.ms - before.ms > 15.0 || apart > SLIP_CENTS {
        return None;
    }
    let t = (ms - before.ms) / (next.ms - before.ms).max(1e-9);
    Some(before.midi() + t * (next.midi() - before.midi()))
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use engine::block::RATE;
    use kernels::fixtures::{self, BUZZ};

    use super::*;

    const A3: f64 = 220.0;

    /// One note of `ms` from the start, on A3.
    fn held(ms: f64) -> [MeantNote; 1] {
        [MeantNote { midi: 57, start_ms: 0.0, end_ms: ms }]
    }

    /// `ms` of a sung vowel `cents_at(ms)` from A3.
    fn sung(ms: f64, cents_at: impl Fn(f64) -> f64) -> Vec<f32> {
        let frames = (ms * f64::from(RATE) / 1000.0) as usize;
        fixtures::harmonic_voice(frames, &BUZZ, |i| {
            A3 * 2f64.powf(cents_at(i as f64 * 1000.0 / f64::from(RATE)) / 1200.0)
        })
    }

    /// `depth` cents of vibrato at 7 Hz, `late_ms` late.
    fn vibrato(depth: f64, late_ms: f64) -> impl Fn(f64) -> f64 {
        move |ms| depth * (TAU * 7.0 * (ms - late_ms) / 1000.0).sin()
    }

    fn median_of(row: &Value, band: &str, name: &str) -> f64 {
        row[band][name]["median"].as_f64().unwrap()
    }

    // The voice through unchanged moves nothing and keeps all its movement, in both bands
    #[test]
    fn a_voice_left_alone_has_no_warble() {
        let dry = sung(3000.0, vibrato(30.0, 0.0));
        let row = measure(&dry, &dry, &[], &held(3000.0));
        for band in ["3_to_12_hz", "6_to_12_hz"] {
            assert_eq!(row[band]["notes"], 1, "{row}");
            assert!(median_of(&row, band, "pitch_moved_cents") < 0.05, "{row}");
            assert!((median_of(&row, band, "pitch_movement_kept") - 1.0).abs() < 0.01, "{row}");
        }
    }

    // Half the vibrato, 20 ms late, moves the voice by the difference of the two vibratos and
    // keeps half the singer's movement
    #[test]
    fn half_the_vibrato_late_keeps_half() {
        let (dry, wet) = (sung(3000.0, vibrato(30.0, 0.0)), sung(3000.0, vibrato(15.0, 20.0)));
        let row = measure(&dry, &wet, &[], &held(3000.0));
        let late = TAU * 7.0 * 0.020;
        let difference = (15.0 * late.cos() - 30.0).hypot(15.0 * late.sin());
        for band in ["3_to_12_hz", "6_to_12_hz"] {
            let moved = median_of(&row, band, "pitch_moved_cents");
            assert!((moved - difference / 2f64.sqrt()).abs() < 1.5, "{row}");
            assert!((median_of(&row, band, "pitch_movement_kept") - 0.5).abs() < 0.03, "{row}");
        }
    }

    // A 9 Hz wobble of 20 cents the singer did not sing, over their 10 cents of vibrato, moves the
    // voice by its own RMS and more than doubles the movement in the band
    #[test]
    fn a_wobble_the_singer_did_not_sing_is_kept_over_one() {
        let wobble = |ms: f64| vibrato(10.0, 0.0)(ms) + 20.0 * (TAU * 9.0 * ms / 1000.0).sin();
        let (dry, wet) = (sung(3000.0, vibrato(10.0, 0.0)), sung(3000.0, wobble));
        let row = measure(&dry, &wet, &[], &held(3000.0));
        let rms = 20.0 / 2f64.sqrt();
        for band in ["3_to_12_hz", "6_to_12_hz"] {
            assert!((median_of(&row, band, "pitch_moved_cents") - rms).abs() < 0.15 * rms, "{row}");
            let kept = median_of(&row, band, "pitch_movement_kept");
            assert!((kept - 5f64.sqrt()).abs() < 0.15 * 5f64.sqrt(), "{row}");
        }
    }

    // Two notes sung legato a whole tone apart with no gap, each 1.2 s with its own vibrato. A
    // corrector that only follows the step, 30 ms late and with a different pull on each note,
    // moves nothing in the band and keeps each note's movement; one that also halves the second
    // note's vibrato reads that on the second note alone
    #[test]
    fn a_change_of_note_is_not_movement() {
        let notes = [
            MeantNote { midi: 57, start_ms: 0.0, end_ms: 1200.0 },
            MeantNote { midi: 59, start_ms: 1200.0, end_ms: 2400.0 },
        ];
        let legato = |step_ms: f64, pulls: [f64; 2], depths: [f64; 2]| {
            sung(2400.0, move |ms| {
                let second = usize::from(ms >= step_ms);
                [0.0, 200.0][second] + pulls[second] + vibrato(depths[second], 0.0)(ms)
            })
        };
        let dry = legato(1200.0, [-20.0, 25.0], [20.0, 30.0]);
        let followed = legato(1230.0, [0.0, 0.0], [20.0, 30.0]);
        let halved = legato(1230.0, [0.0, 0.0], [20.0, 15.0]);
        let row = measure(&dry, &followed, &[], &notes);
        for band in ["3_to_12_hz", "6_to_12_hz"] {
            assert_eq!(row[band]["notes"], 2, "{row}");
            let most = row[band]["pitch_moved_cents"]["most"].as_f64().unwrap();
            assert!(most < 1.0, "following the step moved {most} cents in {band}");
            let kept = &row[band]["pitch_movement_kept"];
            assert!((kept["most"].as_f64().unwrap() - 1.0).abs() < 0.03, "{row}");
        }
        // Read alone, the first note is untouched and the second kept half its 30 cents
        for (note, kept, moved) in [(0, 1.0, 0.0), (1, 0.5, 15.0 / 2f64.sqrt())] {
            let row = measure(&dry, &halved, &[], &notes[note..=note]);
            for band in ["3_to_12_hz", "6_to_12_hz"] {
                assert!(
                    (median_of(&row, band, "pitch_movement_kept") - kept).abs() < 0.03,
                    "{row}"
                );
                assert!((median_of(&row, band, "pitch_moved_cents") - moved).abs() < 1.5, "{row}");
            }
        }
        // Read as one stretch across the step, as a voiced run would be, following reads as movement
        let run = [MeantNote { midi: 58, start_ms: 0.0, end_ms: 2400.0 }];
        let across = measure(&dry, &followed, &[], &run);
        assert!(median_of(&across, "3_to_12_hz", "pitch_moved_cents") > 3.0, "{across}");
    }

    // A sung voice whose fundamental and odd harmonics fade for 60 ms, which the detector reads
    // an octave up and sure of it, in the input and so in the output: the slip is the detector's,
    // not movement, and half the vibrato, 20 ms late, still keeps half
    #[test]
    fn a_slip_of_the_detector_on_the_sung_voice_is_not_movement() {
        let slipped = |depth: f64, late_ms: f64| {
            let whole = sung(3000.0, vibrato(depth, late_ms));
            let even: Vec<f64> =
                BUZZ.iter().enumerate().map(|(h, l)| if h % 2 == 1 { *l } else { 0.0 }).collect();
            let octave = fixtures::harmonic_voice(whole.len(), &even, |i| {
                let ms = i as f64 * 1000.0 / f64::from(RATE);
                A3 * 2f64.powf(vibrato(depth, late_ms)(ms) / 1200.0)
            });
            // Odd harmonics fade out over 40 ms, stay out 60 ms and come back over 40 ms
            let odd = |i: usize| {
                let ms = i as f64 * 1000.0 / f64::from(RATE);
                ((ms - 1460.0).min(1600.0 - ms) / 40.0).clamp(0.0, 1.0) as f32
            };
            whole.iter().zip(&octave).enumerate().map(|(i, (w, o))| w + odd(i) * (o - w)).collect()
        };
        // The output runs 450 frames late and says so, so its readings fall just before the input's
        let (dry, on_time): (Vec<f32>, Vec<f32>) = (slipped(30.0, 0.0), slipped(15.0, 20.0));
        let wet: Vec<f32> = [vec![0.0; 450], on_time].concat();
        let delays = vec![450; dry.len() / engine::block::BLOCK + 1];
        let octave_up = contour::read_with_window(&dry, PITCH_WINDOW, 0)
            .iter()
            .filter(|p| p.voiced() && (1460.0..1600.0).contains(&p.ms) && p.hz > 400.0)
            .count();
        assert!(octave_up >= 3, "the detector slipped on {octave_up} readings");
        let row = measure(&dry, &wet, &delays, &held(3000.0));
        for band in ["3_to_12_hz", "6_to_12_hz"] {
            assert_eq!(row[band]["notes"], 1, "{row}");
            assert!((median_of(&row, band, "pitch_movement_kept") - 0.5).abs() < 0.05, "{row}");
        }
    }

    // The detector reads the output alone an octave up for 50 ms, its readings falling just before
    // the input's. A blend of a reading and the slip beside it would sit 72 cents off, inside the
    // reach, and the bridge would draw a ramp from it: there is no reading there, and a steady 20
    // cent pull reads as moving nothing
    #[test]
    fn a_slip_of_the_detector_on_the_output_alone_is_not_movement() {
        let reading = |ms: f64, cents: f64| Point {
            ms,
            hz: (A3 * 2f64.powf(cents / 1200.0)) as f32,
            confidence: 1.0,
        };
        let sung: Vec<Point> = (0..150)
            .map(|k| reading(10.0 * f64::from(k), vibrato(30.0, 0.0)(10.0 * f64::from(k))))
            .collect();
        let heard: Vec<Point> = (0..151)
            .map(|k| {
                let ms = 10.0 * f64::from(k) - 9.4;
                let slip = if (70..75).contains(&k) { 1200.0 } else { 0.0 };
                reading(ms, 20.0 + vibrato(30.0, 0.0)(ms) + slip)
            })
            .collect();
        let (_, shift) = note_and_shift(&sung, &heard, &held(1500.0)[0]).unwrap();
        let moved = band_rms(&shift, READINGS_PER_SECOND, WARBLE_BAND.0, WARBLE_BAND.1);
        assert!(moved < 0.5, "a pull held steady moved {moved} cents: {shift:?}");
    }

    // A corrector in key mode can flip its target between two scale notes on a held note, which
    // is warble if anything is. Here the pull swings between 60 cents up and 60 down, 120 apart,
    // up for three tenths of each 5 Hz cycle and eased over 5 ms: the rows read that swing as it
    // was applied, and the note is not refused. A reach under a scale step would bridge it away
    #[test]
    fn a_target_flipping_between_two_scale_notes_is_movement() {
        let flips: Vec<f64> = (0..300_000)
            .scan(-60.0, |pull, tick| {
                let ms = f64::from(tick) / 100.0;
                let target = if (TAU * 5.0 * ms / 1000.0).sin() > 0.588 { 60.0 } else { -60.0 };
                *pull += (target - *pull) * (1.0 - (-0.01f64 / 5.0).exp());
                Some(*pull)
            })
            .collect();
        let pull = |ms: f64| flips[((ms * 100.0) as usize).min(flips.len() - 1)];
        let dry = sung(3000.0, vibrato(20.0, 0.0));
        let wet = sung(3000.0, |ms| vibrato(20.0, 0.0)(ms) + pull(ms));
        let row = measure(&dry, &wet, &[], &held(3000.0));
        let applied: Vec<f64> = (6..297).map(|k| pull(10.0 * f64::from(k))).collect();
        for (band, (low, high)) in BANDS {
            assert_eq!(
                (row[band]["notes"].as_u64(), row[band]["refused"].as_u64()),
                (Some(1), Some(0))
            );
            let expected = band_rms(&applied, READINGS_PER_SECOND, low, high);
            let moved = median_of(&row, band, "pitch_moved_cents");
            assert!(
                expected > 10.0 && (moved - expected).abs() < 0.15 * expected,
                "{moved} of {expected}"
            );
        }
    }

    // A note the singer does not voice for four fifths of its length is refused, and counted
    #[test]
    fn a_note_that_cannot_be_read_is_counted() {
        let dry = sung(1500.0, vibrato(30.0, 0.0));
        let silent_after_a_third: Vec<f32> =
            dry.iter().enumerate().map(|(i, s)| if i < 24_000 { *s } else { 0.0 }).collect();
        let row = measure(&silent_after_a_third, &silent_after_a_third, &[], &held(1500.0));
        for band in ["3_to_12_hz", "6_to_12_hz"] {
            assert_eq!(
                (row[band]["notes"].as_u64(), row[band]["refused"].as_u64()),
                (Some(0), Some(1))
            );
        }
    }

    // A note is reported only in a band it resolves: 0.3 s in neither, 0.7 s in 6 to 12 Hz alone,
    // 1.2 s in both
    #[test]
    fn a_short_note_is_not_read_in_a_band_it_cannot_resolve() {
        for (ms, in_whole, in_upper) in [(300.0, 0, 0), (700.0, 0, 1), (1200.0, 1, 1)] {
            let dry = sung(ms, vibrato(30.0, 0.0));
            let row = measure(&dry, &dry, &[], &held(ms));
            assert_eq!(row["3_to_12_hz"]["notes"], in_whole, "{ms} ms: {row}");
            assert_eq!(row["6_to_12_hz"]["notes"], in_upper, "{ms} ms: {row}");
        }
    }
}
