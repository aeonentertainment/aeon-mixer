//! A sung clip and what is known about it, from wherever it came.

use std::path::Path;

use engine::slot::guide::{Guide, Note};
use kernels::PitchClassMask;

use crate::audio::decode_mono;
use crate::chain::frames_of_ms;
use crate::music::{cents_off_class, median, midi_of};
use crate::truth::Truth;

/// One note the singer meant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeantNote {
    pub midi: u8,
    pub start_ms: f64,
    pub end_ms: f64,
}

pub struct SungClip {
    pub name: String,
    pub samples: Vec<f32>,
    /// The notes the guide follows: annotator 1 on vocadito, as the baseline did.
    pub notes: Vec<MeantNote>,
    /// A second reading of the same singing, where one exists: vocadito's annotator 2, who groups
    /// ornaments into longer notes.
    pub second_notes: Option<Vec<MeantNote>>,
    pub truth: Option<Truth>,
    pub key: String,
}

impl SungClip {
    /// A guide over these notes, pulling to `between` between them.
    pub fn guide(&self, between: PitchClassMask) -> Guide {
        Guide { notes: self.notes.iter().map(frame_note).collect(), key_mask: between }
    }

    pub fn seconds(&self) -> f64 {
        self.samples.len() as f64 / f64::from(engine::block::RATE)
    }
}

fn frame_note(note: &MeantNote) -> Note {
    Note {
        midi: note.midi,
        start_frame: frames_of_ms(note.start_ms),
        end_frame: frames_of_ms(note.end_ms),
    }
}

/// `start_seconds,hz,duration_seconds` lines, each note rounded to the note meant, on the
/// harness's rounded milliseconds.
pub fn parse_note_csv(text: &str) -> Result<Vec<MeantNote>, String> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let fields: Vec<f64> = line
                .split(',')
                .map(|field| field.trim().parse().map_err(|_| format!("a bad note line: {line}")))
                .collect::<Result<_, _>>()?;
            let [start, hz, duration] = fields[..] else {
                return Err(format!("a bad note line: {line}"));
            };
            Ok(MeantNote {
                midi: midi_of(hz).round() as u8,
                start_ms: (start * 1000.0).round(),
                end_ms: ((start + duration) * 1000.0).round(),
            })
        })
        .collect()
}

/// `seconds,hz` lines.
pub fn parse_f0_csv(text: &str) -> Result<Truth, String> {
    let rows = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (seconds, hz) = line.split_once(',').ok_or(format!("a bad f0 line: {line}"))?;
            let number = |field: &str| field.trim().parse().map_err(|_| format!("bad f0: {line}"));
            Ok((number(seconds)?, number(hz)?))
        })
        .collect::<Result<Vec<(f64, f64)>, String>>()?;
    Ok(Truth::new(rows))
}

/// A guide file the pipeline writes: `{ "key": "G", "notes": [{ "midi", "startMs", "endMs" }] }`.
pub fn parse_guide_json(text: &str) -> Result<(String, Vec<MeantNote>), String> {
    let guide: serde_json::Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let key = guide["key"].as_str().ok_or("a guide with no key")?.to_owned();
    let notes = guide["notes"]
        .as_array()
        .ok_or("a guide with no notes")?
        .iter()
        .map(|note| {
            let number =
                |field: &str| note[field].as_f64().ok_or(format!("a note with no {field}"));
            Ok(MeantNote {
                midi: number("midi")? as u8,
                start_ms: number("startMs")?,
                end_ms: number("endMs")?,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok((key, notes))
}

/// A person's take of a song they sang along to, carried by a pipeline guide. Nobody marked where
/// the take starts against the guide, so the offset is read from the singing: the one that puts
/// the sung pitches nearest the notes, within `search_ms`.
pub fn load_take(name: &str, wav: &Path, guide: &Path) -> Result<(SungClip, f64), String> {
    let samples = decode_mono(wav)?;
    let text = std::fs::read_to_string(guide).map_err(|e| format!("{}: {e}", guide.display()))?;
    let (key, notes) = parse_guide_json(&text)?;
    let offset = best_offset_ms(&samples, &notes, 3000.0);
    let notes = notes
        .into_iter()
        .map(|note| MeantNote {
            start_ms: note.start_ms + offset,
            end_ms: note.end_ms + offset,
            ..note
        })
        .collect();
    let clip =
        SungClip { name: name.to_owned(), samples, notes, second_notes: None, truth: None, key };
    Ok((clip, offset))
}

/// Offsets this close in median miss line up equally well.
const TIED_CENTS: f64 = 1.0;
/// The offsets tried for a take, this far apart.
const OFFSET_STEP_MS: f64 = 10.0;

/// Where in the take `notes` line up best, in 10 ms steps: the offset with the lowest median miss
/// over the notes that have singing under them, weighing notes covered so an empty stretch of the
/// take cannot win by having one lucky note.
pub fn best_offset_ms(samples: &[f32], notes: &[MeantNote], search_ms: f64) -> f64 {
    let points = crate::contour::read(samples, 0);
    let voiced: Vec<_> = points.iter().filter(|point| point.voiced()).collect();
    let steps = (search_ms / OFFSET_STEP_MS) as i64;
    let mut scores = Vec::with_capacity(2 * steps as usize + 1);
    for step in -steps..=steps {
        let offset = step as f64 * OFFSET_STEP_MS;
        let mut misses = Vec::with_capacity(voiced.len());
        let mut notes_at = notes.iter().peekable();
        for point in &voiced {
            let at = point.ms - offset;
            while notes_at.next_if(|note| note.end_ms <= at).is_some() {}
            if let Some(note) = notes_at.peek().filter(|note| note.start_ms <= at) {
                misses.push(cents_off_class(point.midi(), f64::from(note.midi)).abs());
            }
        }
        if misses.len() < voiced.len() / 4 {
            continue;
        }
        scores.push((offset, median(&misses)));
    }
    centre_of_lowest_run(&scores)
}

/// A long note in tune scores alike at every offset that keeps it under its note, so the lowest
/// score is a run, not a point: the middle of that run is where the notes line up. The run grows
/// from the lowest score both ways through unbroken offsets, so a near-tie elsewhere cannot take it
/// and it never spans an offset the coverage check turned away.
fn centre_of_lowest_run(scores: &[(f64, f64)]) -> f64 {
    let Some(best) = (0..scores.len()).min_by(|&a, &b| scores[a].1.total_cmp(&scores[b].1)) else {
        return 0.0;
    };
    let tied = |index: usize| scores[index].1 <= scores[best].1 + TIED_CENTS;
    let joined = |a: usize, b: usize| scores[b].0 - scores[a].0 <= OFFSET_STEP_MS;
    let mut low = best;
    while low > 0 && tied(low - 1) && joined(low - 1, low) {
        low -= 1;
    }
    let mut high = best;
    while high + 1 < scores.len() && tied(high + 1) && joined(high, high + 1) {
        high += 1;
    }
    (scores[low].0 + scores[high].0) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::harmonic_voice;

    // A take sung half a second into the recording is found there, not at the guide's own time
    #[test]
    fn a_takes_offset_comes_from_the_singing() {
        let notes = [
            MeantNote { midi: 57, start_ms: 200.0, end_ms: 700.0 },
            MeantNote { midi: 60, start_ms: 700.0, end_ms: 1200.0 },
            MeantNote { midi: 64, start_ms: 1200.0, end_ms: 1700.0 },
        ];
        let frames = 48_000 * 3;
        let samples = harmonic_voice(frames, |ms| {
            let sung = ms - 500.0;
            notes
                .iter()
                .find(|note| note.start_ms <= sung && sung < note.end_ms)
                .map_or(0.0, |note| 440.0 * 2f64.powf((f64::from(note.midi) - 69.0) / 12.0))
        });
        let offset = best_offset_ms(&samples, &notes, 1000.0);
        assert!((offset - 500.0).abs() <= 30.0, "found the take at {offset} ms");
    }

    // Tied offsets mean a held note fits anywhere along them: the middle of the run that holds the
    // lowest score is the answer, a run stops at an offset the coverage check left out, and a
    // near-tie away from the lowest does not take it
    #[test]
    fn the_offset_is_the_middle_of_the_lowest_unbroken_run() {
        let tied = [(-20.0, 9.0), (-10.0, 3.0), (0.0, 3.0), (10.0, 3.5), (20.0, 3.2), (30.0, 9.0)];
        assert_eq!(centre_of_lowest_run(&tied), 5.0);
        let apart = [(-30.0, 3.0), (-20.0, 3.0), (40.0, 3.0), (50.0, 3.0)];
        assert_eq!(centre_of_lowest_run(&apart), -25.0);
        // A chorus sung twice fits a wrong offset nearly as well: the true minimum's run wins
        let near_tie = [(-200.0, 10.6), (-190.0, 12.0), (50.0, 10.0), (60.0, 10.0), (70.0, 10.2)];
        assert_eq!(centre_of_lowest_run(&near_tie), 60.0);
    }

    #[test]
    fn notes_round_to_the_note_meant_and_truth_reads_at_or_after() {
        let notes = parse_note_csv("1.079727891,259.966,0.632743764\n").unwrap();
        assert_eq!(notes, [MeantNote { midi: 60, start_ms: 1080.0, end_ms: 1712.0 }]);
        let truth = parse_f0_csv("0.0,0.0\n0.01,220.0\n0.02,0.0\n").unwrap();
        assert_eq!(truth.hz_at(5.0), 220.0);
        assert_eq!(truth.hz_at(10.0), 220.0);
        assert_eq!(truth.hz_at(10.5), 0.0);
    }
}
