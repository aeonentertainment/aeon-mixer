//! A sung clip and what is known about it, from wherever it came.

use std::path::Path;

use engine::slot::guide::{Guide, Note};
use kernels::PitchClassMask;

use crate::audio::decode_mono;
use crate::chain::frames_of_ms;
use crate::music::{cents_off_class, median, midi_of};

/// One note the singer meant.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeantNote {
    pub midi: u8,
    pub start_ms: f64,
    pub end_ms: f64,
}

/// Human-checked f0, for scoring a detector.
pub struct Truth {
    rows: Vec<(f64, f64)>,
}

impl Truth {
    pub fn new(rows: Vec<(f64, f64)>) -> Self {
        Self { rows }
    }

    /// The annotation row at or after `ms`, as the harness's scorer picks it; 0 where unvoiced.
    pub fn hz_at(&self, ms: f64) -> f64 {
        let index = self.rows.partition_point(|(seconds, _)| seconds * 1000.0 < ms);
        self.rows.get(index).or(self.rows.last()).map_or(0.0, |(_, hz)| *hz)
    }
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

/// Where in the take `notes` line up best, in 10 ms steps: the offset with the lowest median miss
/// over the notes that have singing under them, weighing notes covered so an empty stretch of the
/// take cannot win by having one lucky note.
pub fn best_offset_ms(samples: &[f32], notes: &[MeantNote], search_ms: f64) -> f64 {
    let points = crate::contour::read(samples, 0);
    let voiced: Vec<_> = points.iter().filter(|point| point.voiced()).collect();
    let steps = (search_ms / 10.0) as i64;
    let mut best = (f64::INFINITY, 0.0);
    for step in -steps..=steps {
        let offset = step as f64 * 10.0;
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
        let score = median(&misses);
        if score < best.0 {
            best = (score, offset);
        }
    }
    best.1
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
