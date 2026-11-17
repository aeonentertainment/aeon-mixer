//! The bundles a scorecard names: each path is a bundle's directory, one holding a
//! `manifest.json`, or a directory of them.

use std::path::{Path, PathBuf};

/// Each bundle under `paths` with its directory's name, in the order given and by name within
/// a directory. Two bundles of one name would share a scorecard entry, so they are refused.
pub fn find(paths: &[PathBuf]) -> Result<Vec<(String, PathBuf)>, String> {
    let mut found = Vec::new();
    for path in paths {
        if is_bundle(path) {
            found.push(path.clone());
            continue;
        }
        let read = |e: std::io::Error| format!("{}: {e}", path.display());
        let mut inside: Vec<PathBuf> = std::fs::read_dir(path)
            .map_err(read)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<_, _>>()
            .map_err(read)?;
        inside.retain(|dir| is_bundle(dir));
        if inside.is_empty() {
            return Err(format!("{}: neither a bundle nor a directory of them", path.display()));
        }
        inside.sort();
        found.extend(inside);
    }
    let mut named = Vec::new();
    for dir in found {
        let name = dir
            .canonicalize()
            .ok()
            .and_then(|dir| dir.file_name().map(|name| name.to_string_lossy().into_owned()))
            .ok_or_else(|| format!("{}: a bundle needs a directory name", dir.display()))?;
        if named.iter().any(|(other, _)| *other == name) {
            return Err(format!("two bundles are named {name}"));
        }
        named.push((name, dir));
    }
    Ok(named)
}

fn is_bundle(dir: &Path) -> bool {
    dir.join("manifest.json").is_file()
}
