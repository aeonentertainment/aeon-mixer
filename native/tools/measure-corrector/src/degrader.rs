//! A seeded way to make a good singer sing badly, in the ways people do: each note a little off,
//! a slow drift across the phrase, notes scooped into from below, and a wobble in held notes that
//! the singer did not mean. Deep Autotuner (Wager et al. 2020) and BERT-APC (2025) train on
//! singing detuned per note the same way; the ranges here keep every miss inside half a semitone,
//! so every mode, key mode included, can still tell which note was meant.

use std::f64::consts::TAU;

use crate::curve::Curve;
use crate::music::Lcg;
use crate::sung_clip::MeantNote;

/// Each note off by this much, either way: the misses #43 measured on vocadito run 17 to 30 cents
/// at the median. With the drift on top, a held note's miss stays under 50 cents.
const NOTE_OFF_CENTS: (f64, f64) = (10.0, 35.0);
/// Two slow sines a phrase long, together up to 12 cents.
const DRIFT: [(f64, f64); 2] = [(8.0, 0.07), (4.0, 0.17)];
/// Scoops start this far under the note and land over this long, on notes this long or more.
const SCOOP_CENTS: (f64, f64) = (60.0, 150.0);
const SCOOP_MS: (f64, f64) = (80.0, 150.0);
const SCOOPED_NOTE_MS: f64 = 250.0;
/// An unmeant wobble on held notes: 15 to 30 cents deep at 5 to 6.5 Hz, swelling and fading at
/// 1.3 Hz, from after the onset to the note's end.
const WOBBLE_CENTS: (f64, f64) = (15.0, 30.0);
const WOBBLE_HZ: (f64, f64) = (5.0, 6.5);
const WOBBLED_NOTE_MS: f64 = 600.0;

/// What part of a note a moment is in, for reading how well each kind of miss was put right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// Inside a note with no scoop running and no wobble: the miss is its offset and the drift.
    Held,
    Onset,
    Wobble,
}

#[derive(Clone, Copy, Debug)]
struct NoteMiss {
    note: MeantNote,
    off_cents: f64,
    scoop: Option<(f64, f64)>,
    /// Depth, rate and phase.
    wobble: Option<(f64, f64, f64)>,
}

/// One clip's misses, as the curve that sings them and the parts they fall in.
pub struct Degradation {
    pub curve: Curve,
    misses: Vec<NoteMiss>,
}

impl Degradation {
    /// The same seed and notes give the same misses on every run.
    pub fn new(notes: &[MeantNote], length_ms: f64, seed: u64) -> Self {
        let mut rng = Lcg(seed);
        let mut between = |(low, high): (f64, f64)| {
            // `next_u32` gives 31 bits
            low + (high - low) * f64::from(rng.next_u32()) / f64::from(1u32 << 31)
        };
        let misses: Vec<NoteMiss> = notes
            .iter()
            .map(|note| {
                let length = note.end_ms - note.start_ms;
                let sign = if between((0.0, 1.0)) < 0.5 { -1.0 } else { 1.0 };
                let off_cents = sign * between(NOTE_OFF_CENTS);
                let scoop = (length >= SCOOPED_NOTE_MS && between((0.0, 1.0)) < 0.5)
                    .then(|| (between(SCOOP_CENTS), between(SCOOP_MS)));
                let wobble = (length >= WOBBLED_NOTE_MS && between((0.0, 1.0)) < 0.4)
                    .then(|| (between(WOBBLE_CENTS), between(WOBBLE_HZ), between((0.0, TAU))));
                NoteMiss { note: *note, off_cents, scoop, wobble }
            })
            .collect();
        let drift_phase = [between((0.0, TAU)), between((0.0, TAU))];
        let offsets: Vec<f64> = misses.iter().map(|miss| miss.off_cents).collect();
        let curve =
            Curve::by_note(length_ms, notes, &offsets).plus(&Curve::sampled(length_ms, |ms| {
                let drift: f64 = DRIFT
                    .iter()
                    .zip(drift_phase)
                    .map(|((cents, hz), phase)| cents * (TAU * hz * ms / 1000.0 + phase).sin())
                    .sum();
                drift + ornament_at(&misses, ms)
            }));
        Self { curve, misses }
    }

    /// The part of a note `ms` falls in; `None` outside every note.
    pub fn part_at(&self, ms: f64) -> Option<Part> {
        let miss =
            self.misses.iter().find(|miss| miss.note.start_ms <= ms && ms < miss.note.end_ms)?;
        let into = ms - miss.note.start_ms;
        Some(match (miss.scoop, miss.wobble) {
            (Some((_, scoop_ms)), _) if into < scoop_ms => Part::Onset,
            (_, Some(_)) if into >= onset_ms(miss) => Part::Wobble,
            _ => Part::Held,
        })
    }
}

/// Where a note's wobble starts: after its scoop, or 150 ms in.
fn onset_ms(miss: &NoteMiss) -> f64 {
    miss.scoop.map_or(150.0, |(_, scoop_ms)| scoop_ms.max(150.0))
}

fn ornament_at(misses: &[NoteMiss], ms: f64) -> f64 {
    let Some(miss) = misses.iter().find(|miss| miss.note.start_ms <= ms && ms < miss.note.end_ms)
    else {
        return 0.0;
    };
    let into = ms - miss.note.start_ms;
    let scoop = miss.scoop.map_or(0.0, |(cents, scoop_ms)| {
        // Under the note, easing onto it, as a singer reaching up does
        let left = (1.0 - into / scoop_ms).max(0.0);
        -cents * left * left
    });
    let wobble = miss.wobble.map_or(0.0, |(cents, hz, phase)| {
        let from = onset_ms(miss);
        if into < from {
            return 0.0;
        }
        let seconds = (into - from) / 1000.0;
        let swell = 0.6 + 0.4 * (TAU * 1.3 * seconds).sin();
        // Eased in over its first cycle so the wobble has no step
        let ease = (seconds * hz).min(1.0);
        cents * swell * ease * (TAU * hz * seconds + phase).sin()
    });
    scoop + wobble
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notes() -> Vec<MeantNote> {
        (0..40)
            .map(|index| {
                let start = 200.0 + 900.0 * f64::from(index);
                MeantNote { midi: 57 + (index % 5) as u8, start_ms: start, end_ms: start + 800.0 }
            })
            .collect()
    }

    // Every kind of miss happens, each inside the range it promises, and one seed is one take
    #[test]
    fn a_seed_sings_every_kind_of_miss_within_its_range() {
        let notes = notes();
        let degradation = Degradation::new(&notes, 37_000.0, 7);
        let parts: Vec<Part> =
            notes.iter().filter_map(|n| degradation.part_at(n.start_ms + 1.0)).collect();
        assert!(parts.contains(&Part::Onset) && parts.contains(&Part::Held));
        assert!(notes.iter().any(|n| degradation.part_at(n.end_ms - 50.0) == Some(Part::Wobble)));
        for miss in &degradation.misses {
            let off = miss.off_cents.abs();
            assert!((NOTE_OFF_CENTS.0..=NOTE_OFF_CENTS.1).contains(&off), "{off}");
            // A held moment's whole miss is its offset and the drift, never past half a semitone
            let held = miss.note.start_ms + 200.0;
            if degradation.part_at(held) == Some(Part::Held) {
                assert!(degradation.curve.at(held).abs() < 50.0);
            }
        }
        let again = Degradation::new(&notes, 37_000.0, 7);
        assert_eq!(degradation.curve, again.curve);
        assert_ne!(degradation.curve, Degradation::new(&notes, 37_000.0, 8).curve);
    }
}
