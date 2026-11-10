//! The melody takes of the corrector a change replaces, so one blind set can put today's corrector
//! beside the change. The build before the change writes them with `listen --export`; the build
//! with it reads them back with `listen --before` as the `before` take.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::audio::{read_wav, write_wav};

const MEASURES: &str = "measures.json";

/// One source's take as its build rendered it, before level matching, and what it measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Rendered {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub measures: Value,
}

pub struct BeforeTakes {
    dir: PathBuf,
    takes: BTreeMap<String, Rendered>,
}

impl BeforeTakes {
    /// Writes each `(source, take)` as `<source>.wav` under `dir`, with their measures beside them.
    pub fn export<'a>(
        takes: impl IntoIterator<Item = (&'a str, Rendered)>,
        dir: &Path,
    ) -> Result<(), String> {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut measures = Map::new();
        for (source, take) in takes {
            write_wav(&wav_of(dir, source), &take.left, &take.right)?;
            measures.insert(source.to_owned(), take.measures);
        }
        let text = serde_json::to_string_pretty(&measures).map_err(|e| e.to_string())?;
        let path = dir.join(MEASURES);
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn load(dir: &Path) -> Result<Self, String> {
        let path = dir.join(MEASURES);
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let measures: Map<String, Value> =
            serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut takes = BTreeMap::new();
        for (source, measures) in measures {
            let (left, right) = read_wav(&wav_of(dir, &source))?;
            takes.insert(source, Rendered { left, right, measures });
        }
        Ok(Self { dir: dir.to_owned(), takes })
    }

    pub fn take(&self, source: &str) -> Result<Rendered, String> {
        self.takes.get(source).cloned().ok_or(format!("no before take of {source}"))
    }

    /// Where an export keeps the training pair its build rendered.
    pub fn training(dir: &Path) -> PathBuf {
        dir.join("training")
    }

    /// The exported training pair, copied as it was into `to`.
    pub fn copy_training(&self, to: &Path) -> Result<(), String> {
        std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
        let from = Self::training(&self.dir);
        let entries = std::fs::read_dir(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            let name = path.file_name().ok_or(format!("{} has no name", path.display()))?;
            std::fs::copy(&path, to.join(name)).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        Ok(())
    }
}

fn wav_of(dir: &Path, source: &str) -> PathBuf {
    dir.join(format!("{source}.wav"))
}
