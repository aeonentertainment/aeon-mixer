//! Two saved runs side by side: what a later PR quotes as before and after. Every row but CPU is
//! deterministic on one machine, so any change here is the code's.

use std::collections::BTreeMap;

use serde_json::Value;

/// Every number, boolean-free, under a dotted path.
pub fn flatten(value: &Value) -> BTreeMap<String, f64> {
    let mut leaves = BTreeMap::new();
    walk(value, String::new(), &mut leaves);
    leaves
}

fn walk(value: &Value, path: String, leaves: &mut BTreeMap<String, f64>) {
    match value {
        Value::Number(number) => {
            leaves.insert(path, number.as_f64().unwrap_or(f64::NAN));
        }
        Value::Object(fields) => {
            for (key, field) in fields {
                let path = if path.is_empty() { key.clone() } else { format!("{path}.{key}") };
                walk(field, path, leaves);
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk(item, format!("{path}.{index}"), leaves);
            }
        }
        _ => {}
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    pub before: Option<f64>,
    pub after: Option<f64>,
}

/// The numbers that moved by more than `tolerance`, and those only one run has. CPU comes last,
/// since it says something about the machine as well as the code.
pub fn compare(before: &Value, after: &Value, tolerance: f64) -> Vec<Change> {
    let (before, after) = (flatten(before), flatten(after));
    let mut paths: Vec<&String> = before.keys().chain(after.keys()).collect();
    paths.sort_by_key(|path| (path.starts_with("cpu."), path.as_str().to_owned()));
    paths.dedup();
    paths
        .into_iter()
        .filter_map(|path| {
            let (was, now) = (before.get(path).copied(), after.get(path).copied());
            let moved = match (was, now) {
                (Some(was), Some(now)) => (was - now).abs() > tolerance,
                _ => true,
            };
            moved.then(|| Change { path: path.clone(), before: was, after: now })
        })
        .collect()
}

pub fn print(changes: &[Change]) -> String {
    if changes.is_empty() {
        return "no row moved\n".to_owned();
    }
    let side = |value: Option<f64>| value.map_or("-".to_owned(), |number| format!("{number:.3}"));
    let mut table = String::from("| Row | Before | After |\n|---|---|---|\n");
    for change in changes {
        table.push_str(&format!(
            "| {} | {} | {} |\n",
            change.path,
            side(change.before),
            side(change.after)
        ));
    }
    table
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn only_what_moved_is_reported_and_cpu_comes_last() {
        let before =
            json!({ "cpu": { "a": 1.0 }, "row": { "same": 2.0, "moved": 3.0, "gone": 4.0 } });
        let after =
            json!({ "cpu": { "a": 1.5 }, "row": { "same": 2.0, "moved": 3.5, "new": 5.0 } });
        let changes = compare(&before, &after, 0.0);
        let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
        assert_eq!(paths, ["row.gone", "row.moved", "row.new", "cpu.a"]);
        assert!(
            compare(&before, &after, 1.0)
                .iter()
                .all(|c| c.path.contains("gone") || c.path.contains("new"))
        );
    }
}
