//! vocadito (CC BY 4.0, zenodo.org/records/5578807), fetched by a person to a cache outside the
//! repo: `Audio/vocadito_N.wav`, `Annotations/F0/vocadito_N_f0.csv` and
//! `Annotations/Notes/vocadito_N_notesA{1,2}.csv`.

use std::path::Path;

use crate::audio::decode_mono;
use crate::sung_clip::{SungClip, parse_f0_csv, parse_note_csv};

pub const ATTRIBUTION: &str = "Vocal recordings from vocadito: A dataset of solo vocals with f0, \
note, and lyric annotations, by Bittner, Pasalo, Bosch, Meseguer-Brocal and Rubinstein, CC BY 4.0, \
https://zenodo.org/records/5578807. These clips are changed: they are resampled to 48 kHz and run \
through Aeon Mixer's pitch corrector in the ways the answer key lists.";

/// The keys #184's blind-listen clips used, which `MelodyGuide` reads between notes in key mode.
pub fn key_of(clip: u32) -> &'static str {
    match clip {
        2 => "C",
        6 => "A#m",
        10 => "F#",
        19 => "F#m",
        _ => "chromatic",
    }
}

pub fn load(dir: &Path, clip: u32) -> Result<SungClip, String> {
    let read = |relative: String| {
        std::fs::read_to_string(dir.join(&relative)).map_err(|e| format!("{relative}: {e}"))
    };
    Ok(SungClip {
        name: format!("vocadito_{clip}"),
        samples: decode_mono(&dir.join(format!("Audio/vocadito_{clip}.wav")))?,
        notes: parse_note_csv(&read(format!("Annotations/Notes/vocadito_{clip}_notesA1.csv"))?)?,
        second_notes: Some(parse_note_csv(&read(format!(
            "Annotations/Notes/vocadito_{clip}_notesA2.csv"
        ))?)?),
        truth: Some(parse_f0_csv(&read(format!("Annotations/F0/vocadito_{clip}_f0.csv"))?)?),
        key: key_of(clip).to_owned(),
    })
}
