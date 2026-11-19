//! A bundle's `manifest.json`, as `src/bundle/bundle-manifest.ts` writes format 1. Only what a
//! replay uses is read; the rest of the file is the host's and a person's.

use serde::Deserialize;
use serde_json::Value;

/// The one format this replay reads.
pub const FORMAT: u64 = 1;

#[derive(Deserialize)]
pub struct Manifest {
    pub package: String,
    /// The bundle's clock as the mixer was created: the replay's first block.
    pub frame: f64,
    pub us: f64,
    pub slots: Vec<ManifestSlot>,
    pub backing: Option<ManifestBacking>,
}

#[derive(Deserialize)]
pub struct ManifestSlot {
    pub index: usize,
    pub kind: String,
    pub recorded: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestBacking {
    pub stems: Vec<ManifestStem>,
    pub key: i8,
    pub speed: f32,
    /// What the host named; a gain it left out is the profile's.
    pub stem_gains: StemGainChoices,
    pub stop_at_ms: Option<f64>,
}

#[derive(Deserialize)]
pub struct ManifestStem {
    pub kind: StemKind,
    pub sha256: String,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum StemKind {
    Instrumental,
    GuideVocals,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StemGainChoices {
    pub instrumental: Option<f32>,
    pub guide_vocals: Option<f32>,
}

impl Manifest {
    /// The manifest in `text`. The format is checked before anything else is read, so a bundle
    /// from another format is refused by its number and never misread as this one.
    pub fn parse(text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| format!("manifest.json: {e}"))?;
        match value.get("format").and_then(Value::as_u64) {
            Some(FORMAT) => {}
            Some(format) => {
                return Err(format!("bundle format {format}: this replay reads format {FORMAT}"));
            }
            None => return Err("manifest.json names no format".into()),
        }
        serde_json::from_value(value).map_err(|e| format!("manifest.json: {e}"))
    }
}
