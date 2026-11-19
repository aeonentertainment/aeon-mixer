//! A paired take's reference mic, as a bundle holds it under `reference/` (docs/design.md 8.11):
//! `<n>.wav` for slot `n`, mono, 48 kHz, 32-bit float, and `<n>.json` naming the mic.

use std::path::Path;

use serde::Deserialize;
use serde_json::Value;

/// The one format of `<n>.json` this tool reads.
const FORMAT: u64 = 1;

pub struct Reference {
    pub slot: usize,
    pub mic: String,
    pub distance_cm: f64,
    pub samples: Vec<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct About {
    mic: String,
    distance_cm: f64,
}

impl Reference {
    /// Every reference in the bundle at `dir`, by slot, and each other file in its `reference/`
    /// with why it was skipped; neither when it has no `reference/`. A `.wav` named other than
    /// by its slot's plain number is refused, so no file is read as another's slot.
    pub fn find(dir: &Path) -> Result<(Vec<Self>, Vec<String>), String> {
        let folder = dir.join("reference");
        if !folder.is_dir() {
            return Ok((Vec::new(), Vec::new()));
        }
        let read = |e: std::io::Error| format!("reference/: {e}");
        let mut paths = std::fs::read_dir(&folder)
            .map_err(read)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(read)?;
        paths.sort();
        let wav = |path: &Path| extension(path).as_deref() == Some("wav");
        let (mut found, mut skipped) = (Vec::<Self>::new(), Vec::new());
        for path in &paths {
            let file = format!("reference/{}", path.file_name().unwrap_or_default().display());
            let stem = path.file_stem().unwrap_or_default().to_string_lossy();
            if wav(path) {
                let slot =
                    stem.parse::<usize>().ok().filter(|slot| slot.to_string() == stem).ok_or_else(
                        || format!("{file}: a reference is named by its slot, as 0.wav"),
                    )?;
                if found.iter().any(|reference| reference.slot == slot) {
                    return Err(format!("{file}: a second reference for slot {slot}"));
                }
                found.push(Self::read(path, &file, slot)?);
            } else if extension(path).as_deref() != Some("json") {
                skipped.push(format!("{file} was skipped: a reference is <slot>.wav"));
            } else if !paths.iter().any(|other| wav(other) && other.file_stem() == path.file_stem())
            {
                skipped.push(format!("{file} was skipped: it has no {stem}.wav beside it"));
            }
        }
        Ok((found, skipped))
    }

    /// The reference at `path`, which a person names `file`, and the `.json` beside it.
    fn read(path: &Path, file: &str, slot: usize) -> Result<Self, String> {
        let about = path.with_extension("json");
        let name = format!("reference/{}", about.file_name().unwrap_or_default().display());
        let text = std::fs::read_to_string(&about).map_err(|e| format!("{name}: {e}"))?;
        let mut value: Value = serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
        match value
            .as_object_mut()
            .and_then(|about| about.remove("format"))
            .and_then(|f| f.as_u64())
        {
            Some(FORMAT) => {}
            Some(format) => {
                return Err(format!("{name}: format {format}: this tool reads {FORMAT}"));
            }
            None => return Err(format!("{name}: a reference needs a format")),
        }
        let about: About = serde_json::from_value(value).map_err(|e| format!("{name}: {e}"))?;
        let bytes = std::fs::read(path).map_err(|e| format!("{file}: {e}"))?;
        let samples = float_wav(&bytes).map_err(|e| format!("{file}: {e}"))?;
        Ok(Self { slot, mic: about.mic, distance_cm: about.distance_cm, samples })
    }
}

/// A file's extension in lower case: a recorder may write `.WAV`.
fn extension(path: &Path) -> Option<String> {
    path.extension().map(|extension| extension.to_string_lossy().to_lowercase())
}

/// The samples of a mono, 48 kHz, 32-bit float WAV, plain or extensible. Anything else is refused
/// by what it is, so a person knows what to export instead.
fn float_wav(bytes: &[u8]) -> Result<Vec<f32>, String> {
    let u16_at = |b: &[u8], at: usize| b.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |b: &[u8], at: usize| {
        b.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    if bytes.get(0..4) != Some(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("not a WAV file".into());
    }
    let mut format = None;
    let mut at = 12;
    while let (Some(id), Some(len)) = (bytes.get(at..at + 4), u32_at(bytes, at + 4)) {
        let len = len as usize;
        let body = &bytes[(at + 8).min(bytes.len())..(at + 8 + len).min(bytes.len())];
        if id == b"fmt " {
            format = Some(body);
        } else if id == b"data" {
            let fmt = format.ok_or("a WAV file's data before its format")?;
            let tag = match u16_at(fmt, 0) {
                Some(0xFFFE) => u16_at(fmt, 24),
                tag => tag,
            };
            let (channels, rate, bits) = (u16_at(fmt, 2), u32_at(fmt, 4), u16_at(fmt, 14));
            if (tag, channels, rate, bits) != (Some(3), Some(1), Some(48_000), Some(32)) {
                return Err(format!(
                    "a reference is mono, 48 kHz, 32-bit float, and this is {} channels at {} Hz, \
                     {}-bit {}",
                    channels.unwrap_or(0),
                    rate.unwrap_or(0),
                    bits.unwrap_or(0),
                    if tag == Some(3) { "float" } else { "integer" }
                ));
            }
            return Ok(body.as_chunks::<4>().0.iter().map(|&b| f32::from_le_bytes(b)).collect());
        }
        at += 8 + len + len % 2;
    }
    Err("a WAV file with no data".into())
}
