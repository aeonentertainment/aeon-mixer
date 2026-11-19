//! A scorecard file: each bundle's numbers, each with the tolerance a comparison allows it. The
//! tool writes every tolerance as 0 or as the baseline had it; a person sets them, so which
//! movement fails a run is the file's to say and never the tool's.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::numbers::Numbers;

/// The one format this tool reads and writes.
pub const FORMAT: u64 = 1;

#[derive(Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Scorecard {
    pub format: u64,
    /// Each bundle by its directory's name.
    pub bundles: BTreeMap<String, BTreeMap<String, Entry>>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub value: f64,
    /// How far `value` may move against this file as a baseline before the run fails.
    pub tolerance: f64,
}

impl Scorecard {
    pub fn new() -> Self {
        Self { format: FORMAT, bundles: BTreeMap::new() }
    }

    /// Adds `bundle`'s numbers, each with the tolerance `baseline` gives it, or 0.
    pub fn add(&mut self, bundle: &str, numbers: Numbers, baseline: Option<&Scorecard>) {
        let tolerances = baseline.and_then(|card| card.bundles.get(bundle));
        let entries = numbers
            .into_iter()
            .map(|(number, value)| {
                let tolerance =
                    tolerances.and_then(|t| t.get(&number)).map_or(0.0, |entry| entry.tolerance);
                (number, Entry { value, tolerance })
            })
            .collect();
        self.bundles.insert(bundle.to_owned(), entries);
    }

    /// The scorecard in `path`. A format it does not know is refused by its number, and an
    /// unknown field by name, so a misspelt tolerance never passes as one that held.
    pub fn read(path: &Path) -> Result<Self, String> {
        let name = path.display();
        let text = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
        let value: Value = serde_json::from_str(&text).map_err(|e| format!("{name}: {e}"))?;
        match value.get("format").and_then(Value::as_u64) {
            Some(FORMAT) => {}
            Some(format) => {
                return Err(format!("{name}: scorecard format {format}: this tool reads {FORMAT}"));
            }
            None => return Err(format!("{name}: a scorecard needs a format")),
        }
        serde_json::from_value(value).map_err(|e| format!("{name}: {e}"))
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        let mut text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        text.push('\n');
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}
