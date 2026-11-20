//! A bench run's numbers as a scorecard file (docs/design.md 8.9): written with `--write`, and
//! compared with `--baseline` under the scorecard's own rule.

use std::collections::BTreeMap;
use std::path::Path;

use scorecard::card::Scorecard;
use scorecard::movement;

/// Files `numbers` under `name`, each rounded to a hundredth, writes them to `write` when given,
/// and prints each one that moved against `baseline`. Ok(false) when one moved past what the
/// baseline allows.
pub fn settle(
    name: &str,
    numbers: impl IntoIterator<Item = (&'static str, f64)>,
    baseline: Option<&Scorecard>,
    write: Option<&Path>,
) -> Result<bool, String> {
    let numbers: BTreeMap<String, f64> = numbers
        .into_iter()
        .map(|(number, value)| (number.to_owned(), (value * 100.0).round() / 100.0))
        .collect();
    let mut card = Scorecard::new();
    card.add(name, numbers, baseline);
    if let Some(path) = write {
        card.write(path)?;
        println!("wrote    this result to {}", path.display());
    }
    Ok(baseline.is_none_or(|baseline| {
        println!("\nagainst the baseline");
        movement::report(&card, baseline)
    }))
}

/// The baseline `--baseline` names, read before a run so a wrong file costs nothing.
pub fn baseline(path: Option<&Path>) -> Result<Option<Scorecard>, String> {
    path.map(|path| {
        Scorecard::read(path).map_err(|e| format!("--baseline is not a scorecard bench wrote: {e}"))
    })
    .transpose()
}

/// Refuses, before a run, a `--write` path the result could not be written to, or one that holds
/// anything but a scorecard, which writing would destroy. A scorecard there is replaced, as a
/// baseline is updated.
pub fn writable(path: Option<&Path>) -> Result<(), String> {
    let Some(path) = path else { return Ok(()) };
    let name = path.display();
    if path.is_dir() {
        return Err(format!("--write {name} is a folder: name a file for the result"));
    }
    if path.exists() {
        Scorecard::read(path).map_err(|e| {
            format!("--write {name} would overwrite a file that is not a scorecard ({e})")
        })?;
    }
    let folder = path.parent().filter(|folder| !folder.as_os_str().is_empty());
    if folder.is_some_and(|folder| !folder.is_dir()) {
        return Err(format!("--write {name}: its folder does not exist"));
    }
    Ok(())
}
