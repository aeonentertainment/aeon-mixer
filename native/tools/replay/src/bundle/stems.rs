//! A bundle names each stem by the SHA-256 of its file and never copies it (decision 0026), so
//! `--backing` names a directory and the stems are found in it by hash.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use engine::backing::StemFiles;
use sha2::{Digest, Sha256};

use super::manifest::{ManifestBacking, StemKind};

/// The files in `dir` whose hashes the manifest names.
pub fn find(dir: &Path, backing: &ManifestBacking) -> Result<StemFiles, String> {
    let read = |e: std::io::Error| format!("{}: {e}", dir.display());
    let mut by_hash = HashMap::new();
    for entry in std::fs::read_dir(dir).map_err(read)? {
        let path = entry.map_err(read)?.path();
        if path.is_file() {
            by_hash.insert(sha256(&path)?, path);
        }
    }
    let stem = |kind: StemKind| -> Result<Option<PathBuf>, String> {
        let Some(named) = backing.stems.iter().find(|stem| stem.kind == kind) else {
            return Ok(None);
        };
        // Two stems may be one file, so a hash can name both
        by_hash.get(&named.sha256).cloned().map(Some).ok_or_else(|| {
            format!("no file in {} is the {kind:?} stem, SHA-256 {}", dir.display(), named.sha256)
        })
    };
    let instrumental =
        stem(StemKind::Instrumental)?.ok_or("the manifest names no instrumental stem")?;
    Ok(StemFiles { instrumental, guide: stem(StemKind::GuideVocals)? })
}

pub fn sha256(path: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    File::open(path)
        .and_then(|mut file| std::io::copy(&mut file, &mut hasher))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}
