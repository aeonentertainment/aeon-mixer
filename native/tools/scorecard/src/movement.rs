//! A number that differs between a scorecard and its baseline, and whether the baseline's
//! tolerance lets it.

use std::collections::{BTreeMap, BTreeSet};

use crate::card::{Entry, Scorecard};

#[derive(Debug, PartialEq)]
pub struct Movement {
    pub bundle: String,
    pub number: String,
    pub change: Change,
}

#[derive(Debug, PartialEq)]
pub enum Change {
    Moved {
        baseline: f64,
        now: f64,
        tolerance: f64,
    },
    /// In the baseline and not scored now: a bundle not given, or a number with nothing to read.
    Gone {
        baseline: f64,
    },
    /// Scored now and not in the baseline, which so has no tolerance for it.
    New {
        now: f64,
    },
}

impl Movement {
    /// Every number that differs between `now` and `baseline`, by bundle and then number.
    pub fn between(now: &Scorecard, baseline: &Scorecard) -> Vec<Self> {
        let bundles: BTreeSet<&String> =
            now.bundles.keys().chain(baseline.bundles.keys()).collect();
        let mut movements = Vec::new();
        for bundle in bundles {
            let (scored, base) = (now.bundles.get(bundle), baseline.bundles.get(bundle));
            let numbers: BTreeSet<&String> =
                scored.into_iter().chain(base).flat_map(|entries| entries.keys()).collect();
            for number in numbers {
                let entry = |card: Option<&BTreeMap<String, Entry>>| {
                    card.and_then(|entries| entries.get(number)).copied()
                };
                let change = match (entry(scored), entry(base)) {
                    (Some(now), Some(base)) if now.value != base.value => Change::Moved {
                        baseline: base.value,
                        now: now.value,
                        tolerance: base.tolerance,
                    },
                    (Some(_), Some(_)) | (None, None) => continue,
                    (None, Some(base)) => Change::Gone { baseline: base.value },
                    (Some(now), None) => Change::New { now: now.value },
                };
                movements.push(Self { bundle: bundle.clone(), number: number.clone(), change });
            }
        }
        movements
    }

    /// Whether this movement fails the run: a move past its tolerance, or a number gone.
    pub fn fails(&self) -> bool {
        match self.change {
            Change::Moved { baseline, now, tolerance } => {
                hundredths(now - baseline).abs() > tolerance
            }
            Change::Gone { .. } => true,
            Change::New { .. } => false,
        }
    }
}

/// Numbers are kept to a hundredth, so their difference is too, and a move of exactly the
/// tolerance is within it.
fn hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
