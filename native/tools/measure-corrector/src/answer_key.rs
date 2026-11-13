//! The original vocal as the answer key. A good singer is left alone; a singer who misses is
//! brought to the melody without damage. docs/decisions/0015-the-answer-key.md has the research
//! and how far each row can be trusted.
//!
//! 1. **The key.** Each note's centre is moved onto its note with the singer's movement left on
//!    it, so the key is a voice that sings in tune and sounds like itself. A studio vocal moves
//!    little; vocadito's singers move 17 to 30 cents.
//! 2. **Left alone.** The key through the slot. Everything the corrector moves is damage.
//! 3. **Broken and restored.** The degrader sings the key badly in known ways and the slot puts
//!    it right. What is left against the key is the error, and the degrader's own round trip,
//!    bent and bent back, is the floor no row can go under.
//! 4. **As sung.** The clip itself through the slot, for how often correction lets go of a note
//!    and takes it back.

use engine::params::CleanupParams;
use serde_json::{Value, json};

use crate::artefacts;
use crate::bend::bend;
use crate::chain::Take;
use crate::condition::Condition;
use crate::contour::{self, Point};
use crate::curve::Curve;
use crate::degrader::{Degradation, Part};
use crate::expression::{self, Envelope, HAS_VIBRATO_CENTS, VIBRATO_BAND, band_rms, note_contour};
use crate::flapping;
use crate::music::{cents_off_class, median, percentile};
use crate::pitch_error::{Moment, moments};
use crate::spectral::{SpectralDistance, SpectralRuler};
use crate::sung_clip::{MeantNote, SungClip};
use crate::sung_rows::render;
use crate::take_delay::TakeDelay;
use crate::variant::Variant;
use crate::vibrato;

/// The answer key measures the corrector alone, so the slot's cleanup stays off for it.
const OFF: CleanupParams = CleanupParams { enabled: false };

/// Past a note's first moments before reading its centre, as the intonation row does.
const CENTRE_LEAD_MS: f64 = 30.0;
const CENTRE_TAIL_MS: f64 = 10.0;

/// A voice, its readings and the clock its readings are on.
struct Heard<'a> {
    samples: Vec<f32>,
    points: Vec<Point>,
    delay: TakeDelay<'a>,
}

impl<'a> Heard<'a> {
    fn dry(samples: Vec<f32>) -> Self {
        let points = contour::read(&samples, 0);
        Self { samples, points, delay: TakeDelay::NONE }
    }

    fn take(take: &'a Take) -> Self {
        let samples = take.voice();
        let delay = TakeDelay::new(&take.delays);
        let points = delay.onto_input(&contour::read(&samples, 0));
        Self { samples, points, delay }
    }
}

/// Every answer-key row for `clip`, under each variant.
pub fn rows(clip: &SungClip, variants: &[Variant]) -> Result<Value, String> {
    let length_ms = clip.seconds() * 1000.0;
    let original = contour::read(&clip.samples, 0);
    let snap = centre_snap(&original, &clip.notes);
    let key_clip = SungClip {
        name: clip.name.clone(),
        samples: bend(&clip.samples, &Curve::by_note(length_ms, &clip.notes, &snap)),
        notes: clip.notes.clone(),
        second_notes: clip.second_notes.clone(),
        truth: None,
        key: clip.key.clone(),
    };
    let key = Heard::dry(key_clip.samples.clone());
    let degradation = Degradation::new(&clip.notes, length_ms, seed_of(&clip.name));
    let broken_clip = SungClip {
        name: clip.name.clone(),
        samples: bend(&key.samples, &degradation.curve),
        notes: clip.notes.clone(),
        second_notes: clip.second_notes.clone(),
        truth: None,
        key: clip.key.clone(),
    };
    let broken = Heard::dry(broken_clip.samples.clone());
    let round_trip = Heard::dry(bend(&broken.samples, &degradation.curve.undone()));

    let ruler = SpectralRuler::new();
    let in_notes = |moment: &Moment| inside(&clip.notes, moment.ms);
    let applied: Vec<Moment> =
        moments(&key.points, &broken.points).into_iter().filter(in_notes).collect();
    let asked: Vec<f64> =
        applied.iter().map(|moment| degradation.curve.at(moment.ms).abs()).collect();
    let floor_moments: Vec<Moment> =
        moments(&key.points, &round_trip.points).into_iter().filter(in_notes).collect();
    let floor = spectra(&ruler, &key, &round_trip, &clip.notes);

    let mut by_variant = serde_json::Map::new();
    for variant in variants {
        let left_take =
            render(&key_clip, Condition::Direct, variant.guide(&key_clip)?, variant, OFF).take;
        let left = Heard::take(&left_take);
        let restored_take =
            render(&broken_clip, Condition::Direct, variant.guide(&broken_clip)?, variant, OFF)
                .take;
        let restored = Heard::take(&restored_take);
        let sung_take = render(clip, Condition::Direct, variant.guide(clip)?, variant, OFF).take;
        let flapping = flapping::measure(&sung_take.readings, &sung_take.engaged, &clip.notes);
        by_variant.insert(
            variant.label.clone(),
            json!({
                "flapping": flapping.json(),
                "left_alone": left_alone(&ruler, &key, &left, &key_clip),
                "restored": restored_row(&ruler, &key, &broken, &restored, &clip.notes, &degradation),
            }),
        );
    }
    let snap_moved: Vec<f64> = snap.iter().map(|cents| cents.abs()).collect();
    Ok(json!({
        "key_moved_median_cents": median(&snap_moved),
        "degradation": {
            "asked_median_cents": median(&asked),
            "applied_median_cents": median(&magnitudes(&applied)),
        },
        "floor": {
            "pitch_median_cents": median(&magnitudes(&floor_moments)),
            "pitch_p95_cents": percentile(&magnitudes(&floor_moments), 0.95),
            "lsd_db": floor.lsd_db,
            "mcd_db": floor.mcd_db,
        },
        "variants": by_variant,
    }))
}

/// For each note, the move that puts its centre on the note: zero for a note with under three
/// voiced readings.
fn centre_snap(points: &[Point], notes: &[MeantNote]) -> Vec<f64> {
    notes
        .iter()
        .map(|note| {
            let sung: Vec<f64> = points
                .iter()
                .filter(|point| {
                    point.voiced()
                        && note.start_ms + CENTRE_LEAD_MS <= point.ms
                        && point.ms < note.end_ms - CENTRE_TAIL_MS
                })
                .map(Point::midi)
                .collect();
            if sung.len() < 3 { 0.0 } else { -cents_off_class(median(&sung), f64::from(note.midi)) }
        })
        .collect()
}

fn left_alone(ruler: &SpectralRuler, key: &Heard, left: &Heard, clip: &SungClip) -> Value {
    let moved: Vec<Moment> = moments(&key.points, &left.points)
        .into_iter()
        .filter(|m| inside(&clip.notes, m.ms))
        .collect();
    let sustained = clip.second_notes.as_deref().unwrap_or(&clip.notes);
    let expression = expression::compare(
        &key.points,
        &left.points,
        &Envelope::of(&key.samples, 0),
        &Envelope::of(&left.samples, 0),
        sustained,
    );
    let spectra = spectra(ruler, key, left, &clip.notes);
    json!({
        "moved_median_cents": median(&magnitudes(&moved)),
        "moved_p95_cents": percentile(&magnitudes(&moved), 0.95),
        "vibrato_notes": expression.vibrato_notes,
        "vibrato_depth_kept": expression.vibrato_kept,
        "vibrato_rate_moved_hz": vibrato_rate_moved(&key.points, &left.points, sustained),
        "lsd_db": spectra.lsd_db,
        "mcd_db": spectra.mcd_db,
        "artefacts": artefacts::compare(&key.samples, &left.samples, 0, &left.delay, &clip.notes),
    })
}

fn restored_row(
    ruler: &SpectralRuler,
    key: &Heard,
    broken: &Heard,
    restored: &Heard,
    notes: &[MeantNote],
    degradation: &Degradation,
) -> Value {
    let errors: Vec<Moment> = moments(&key.points, &restored.points)
        .into_iter()
        .filter(|m| inside(notes, m.ms))
        .collect();
    let part = |wanted| -> Vec<f64> {
        errors
            .iter()
            .filter(|moment| degradation.part_at(moment.ms) == Some(wanted))
            .map(|moment| moment.cents.abs())
            .collect()
    };
    let spectra = spectra(ruler, key, restored, notes);
    json!({
        "median_cents": median(&magnitudes(&errors)),
        "p95_cents": percentile(&magnitudes(&errors), 0.95),
        "held_median_cents": median(&part(Part::Held)),
        "onset_median_cents": median(&part(Part::Onset)),
        "wobble_median_cents": median(&part(Part::Wobble)),
        "lsd_db": spectra.lsd_db,
        "mcd_db": spectra.mcd_db,
        "artefacts": artefacts::compare(&broken.samples, &restored.samples, 0, &restored.delay, notes),
    })
}

/// The spectral distances of `heard` from the key, over frames the key sings inside a note.
fn spectra(
    ruler: &SpectralRuler,
    key: &Heard,
    heard: &Heard,
    notes: &[MeantNote],
) -> SpectralDistance {
    let voiced = |ms| inside(notes, ms) && voiced_at(&key.points, ms);
    ruler.distance(&key.samples, &heard.samples, &heard.delay, voiced)
}

/// On held notes with clear vibrato in the key, how far the output's rate moved, median, in Hz.
fn vibrato_rate_moved(key: &[Point], output: &[Point], notes: &[MeantNote]) -> Option<f64> {
    let moved: Vec<f64> = notes
        .iter()
        .filter(|note| note.end_ms - note.start_ms >= 500.0)
        .filter_map(|note| Some((note_contour(key, note)?, note_contour(output, note)?)))
        .filter(|(dry, _)| {
            band_rms(dry, 100.0, VIBRATO_BAND.0, VIBRATO_BAND.1) >= HAS_VIBRATO_CENTS
        })
        .map(|(dry, wet)| (vibrato::rate_hz(&wet, 100.0) - vibrato::rate_hz(&dry, 100.0)).abs())
        .collect();
    (!moved.is_empty()).then(|| median(&moved))
}

fn inside(notes: &[MeantNote], ms: f64) -> bool {
    let next = notes.partition_point(|note| note.start_ms <= ms);
    next.checked_sub(1).is_some_and(|index| ms < notes[index].end_ms)
}

fn voiced_at(points: &[Point], ms: f64) -> bool {
    let index = points.partition_point(|point| point.ms < ms);
    points.get(index).is_some_and(Point::voiced)
}

fn magnitudes(moments: &[Moment]) -> Vec<f64> {
    moments.iter().map(|moment| moment.cents.abs()).collect()
}

/// A seed from the clip's name, so each clip has its own misses and every run the same ones.
fn seed_of(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;
    use crate::music::hz_of;
    use crate::synthetic::harmonic_voice;

    /// Six held notes, each sung `misses[i]` cents off with a 5.5 Hz vibrato of 20 cents.
    fn singer(misses: [f64; 6]) -> SungClip {
        let notes: Vec<MeantNote> = (0..6)
            .map(|i| {
                let start = 200.0 + 800.0 * i as f64;
                MeantNote {
                    midi: [57, 60, 64, 62, 59, 57][i],
                    start_ms: start,
                    end_ms: start + 700.0,
                }
            })
            .collect();
        let samples = harmonic_voice(48 * 5200, |ms| {
            notes.iter().zip(misses).find(|(n, _)| n.start_ms <= ms && ms < n.end_ms).map_or(
                0.0,
                |(note, miss)| {
                    let wobble = 20.0 * (TAU * 5.5 * ms / 1000.0).sin();
                    hz_of(f64::from(note.midi)) * 2f64.powf((miss + wobble) / 1200.0)
                },
            )
        });
        SungClip {
            name: "known".to_owned(),
            samples,
            notes,
            second_notes: None,
            truth: None,
            key: "C".to_owned(),
        }
    }

    fn variant(label: &str) -> Variant {
        Variant::standard().into_iter().find(|v| v.label == label).unwrap()
    }

    // A singer 20 cents off on every note, either way. The key puts each note within 3 cents of
    // where it was moved, the degrader's own round trip costs under 3 cents at the median, and the
    // degradation reads back within 15 percent of what was asked. Off leaves the key exactly
    // alone; full strength puts held notes back to under half the miss the degrader sang. One run
    // repeats exactly.
    #[test]
    fn the_key_is_found_broken_and_restored_within_its_floor() {
        let clip = singer([20.0, -20.0, 20.0, -20.0, 20.0, -20.0]);
        let variants = [variant("off"), variant("melody-1.0")];
        let rows = rows(&clip, &variants).unwrap();
        let read = |path: &str| rows.pointer(path).and_then(Value::as_f64).unwrap_or(f64::NAN);
        assert!((read("/key_moved_median_cents") - 20.0).abs() < 3.0, "{rows:#}");
        assert!(read("/floor/pitch_median_cents") < 3.0, "{rows:#}");
        let (asked, applied) =
            (read("/degradation/asked_median_cents"), read("/degradation/applied_median_cents"));
        assert!((applied - asked).abs() < 0.15 * asked, "asked {asked}, applied {applied}");

        assert!(read("/variants/off/left_alone/moved_p95_cents") < 1e-6, "{rows:#}");
        assert!(read("/variants/off/left_alone/mcd_db") < 1e-6, "{rows:#}");
        let broken = read("/variants/off/restored/held_median_cents");
        let restored = read("/variants/melody-1.0/restored/held_median_cents");
        assert!(
            broken > 10.0 && restored < 0.5 * broken,
            "held {broken} broken, {restored} restored"
        );

        assert_eq!(rows, super::rows(&clip, &variants).unwrap());
    }
}
