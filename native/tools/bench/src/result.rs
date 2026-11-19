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
