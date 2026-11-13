//! How often correction lets go of a sung note and takes it back: the back and forth a singer
//! hears as tuned and untuned in turn. Counted from whether the corrector was pulling the voice at
//! each published reading, 10 ms apart, inside each meant note past its first moments, per second
//! of those notes.

use serde_json::{Value, json};

use crate::contour::{LOWEST_CORRECTED_HZ, Point, VOICED_CONFIDENCE};
use crate::sung_clip::MeantNote;

/// Past a note's first moments, where correction is still arriving, and before its end.
const NOTE_LEAD_MS: f64 = 30.0;
const NOTE_TAIL_MS: f64 = 10.0;

#[derive(Debug, Default, PartialEq)]
pub struct Flapping {
    pub note_seconds: f64,
    pub readings: usize,
    pub acting: usize,
    /// Correction letting go between two readings in one note.
    pub drops: usize,
    /// Correction taking the note back.
    pub returns: usize,
    /// Drops by the reading they came at: confidence under the corrector's bar, a sure reading
    /// under 82 Hz, and a sure reading the gate passes, where the corrector's own reading or its
    /// hold let go. The corrector votes with its own readings, which may not be the published one,
    /// so the split says what a host saw, not why the kernel decided.
    pub drops_unsure: usize,
    pub drops_low: usize,
    pub drops_sure: usize,
    /// Readings inside notes that fail the gate, whether or not correction let go on them.
    pub unsure_readings: usize,
}

/// `readings`, and whether the corrector was pulling the voice as each came out, against `notes`.
pub fn measure(readings: &[Point], engaged: &[bool], notes: &[MeantNote]) -> Flapping {
    let mut flapping = Flapping {
        note_seconds: notes
            .iter()
            .map(|note| (note.end_ms - NOTE_TAIL_MS - note.start_ms - NOTE_LEAD_MS).max(0.0))
            .sum::<f64>()
            / 1000.0,
        ..Flapping::default()
    };
    let passes_gate =
        |point: &Point| point.hz >= LOWEST_CORRECTED_HZ && point.confidence >= VOICED_CONFIDENCE;
    let mut last: Option<(usize, bool)> = None;
    for (point, &acting) in readings.iter().zip(engaged) {
        let Some(note) = note_holding(notes, point.ms) else {
            last = None;
            continue;
        };
        flapping.readings += 1;
        flapping.acting += usize::from(acting);
        flapping.unsure_readings += usize::from(!passes_gate(point));
        if let Some((_, was_acting)) = last.filter(|(last_note, _)| *last_note == note) {
            match (was_acting, acting) {
                (true, false) => {
                    flapping.drops += 1;
                    if point.confidence < VOICED_CONFIDENCE {
                        flapping.drops_unsure += 1;
                    } else if point.hz < LOWEST_CORRECTED_HZ {
                        flapping.drops_low += 1;
                    } else {
                        flapping.drops_sure += 1;
                    }
                }
                (false, true) => flapping.returns += 1,
                (true, true) | (false, false) => {}
            }
        }
        last = Some((note, acting));
    }
    flapping
}

impl Flapping {
    pub fn json(&self) -> Value {
        let per_second = |count: usize| count as f64 / self.note_seconds.max(f64::EPSILON);
        let share = |count: usize, of: usize| 100.0 * count as f64 / of.max(1) as f64;
        json!({
            "note_seconds": self.note_seconds,
            "acting_pct": share(self.acting, self.readings),
            "switches_per_second": per_second(self.drops + self.returns),
            "drops_per_second": per_second(self.drops),
            "drops_unsure_pct": share(self.drops_unsure, self.drops),
            "drops_under_82_hz_pct": share(self.drops_low, self.drops),
            "drops_at_a_sure_reading_pct": share(self.drops_sure, self.drops),
            "unsure_readings_per_second": per_second(self.unsure_readings),
        })
    }
}

/// The note whose trimmed span holds `ms`, by index.
fn note_holding(notes: &[MeantNote], ms: f64) -> Option<usize> {
    let next = notes.partition_point(|note| note.start_ms + NOTE_LEAD_MS <= ms);
    let index = next.checked_sub(1)?;
    (ms < notes[index].end_ms - NOTE_TAIL_MS).then_some(index)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(ms: f64, sure: bool) -> Point {
        Point { ms, hz: 220.0, confidence: if sure { 0.99 } else { 0.5 } }
    }

    // One note held 1 s: correction drops twice on unsure readings and comes back, and once on a
    // reading that clears the bar. A note correction never holds counts nothing, and neither do
    // readings outside a note or a switch across a note edge
    #[test]
    fn counts_switches_inside_notes_per_second_of_note() {
        let notes = [
            MeantNote { midi: 57, start_ms: 0.0, end_ms: 1040.0 },
            MeantNote { midi: 59, start_ms: 1040.0, end_ms: 2080.0 },
        ];
        let mut readings = Vec::new();
        let mut engaged = Vec::new();
        for i in 0..200 {
            let ms = f64::from(i) * 10.0 + 5.0;
            let unsure = (300.0..340.0).contains(&ms) || (600.0..620.0).contains(&ms);
            let let_go = unsure || (800.0..810.0).contains(&ms);
            readings.push(point(ms, !unsure));
            engaged.push(ms < 1040.0 && !let_go);
        }
        let flapping = measure(&readings, &engaged, &notes);
        assert!((flapping.note_seconds - 2.0).abs() < 1e-9);
        assert_eq!((flapping.drops, flapping.returns), (3, 3));
        assert_eq!((flapping.drops_unsure, flapping.drops_low, flapping.drops_sure), (2, 0, 1));
        assert_eq!(flapping.unsure_readings, 6);
    }
}
