//! The answer-key rows and the artefacts every take added, as the tables a PR quotes. Like
//! `report`, it reads only the result JSON.

use std::fmt::Write;

use serde_json::Value;

pub const UNCALIBRATED: &str = "These rows inform and gate nothing: none has yet been checked against \
the blind listen, and none says how anything sounds.";

fn number(value: &Value, places: usize) -> String {
    value.as_f64().map_or_else(|| "-".to_owned(), |n| format!("{n:.places$}"))
}

/// A row's number, or that it is under what the degrader alone costs, which no row may claim to
/// beat.
fn above(value: &Value, floor: &Value, places: usize) -> String {
    match (value.as_f64(), floor.as_f64()) {
        (Some(value), Some(floor)) if value <= floor => format!("under floor {floor:.places$}"),
        _ => number(value, places),
    }
}

fn artefacts(row: &Value) -> String {
    format!(
        "{}, {}, {} c, {} % (in {} %)",
        number(&row["clicks_added_per_min"], 1),
        number(&row["octave_jumps_added_per_min"], 1),
        number(&row["fast_warble_added_p95_cents"], 1),
        number(&row["still_notes_pct"], 0),
        number(&row["still_notes_pct_in"], 0),
    )
}

pub fn table(result: &Value) -> String {
    let mut table = format!("{UNCALIBRATED}\n\n");
    let Some(keys) = result["answer_key"].as_object() else { return table };
    table.push_str(
        "| Answer key | Key moved notes | Degradation asked, applied | Floor: pitch median, p95 | \
         Floor: LSD, MCD |\n|---|---|---|---|---|\n",
    );
    for (clip, key) in keys {
        let floor = &key["floor"];
        writeln!(
            table,
            "| {clip} | {} c | {}, {} c | {}, {} c | {}, {} dB |",
            number(&key["key_moved_median_cents"], 1),
            number(&key["degradation"]["asked_median_cents"], 1),
            number(&key["degradation"]["applied_median_cents"], 1),
            number(&floor["pitch_median_cents"], 1),
            number(&floor["pitch_p95_cents"], 1),
            number(&floor["lsd_db"], 2),
            number(&floor["mcd_db"], 2),
        )
        .expect("a string takes writes");
    }

    table.push_str(
        "\n| Left alone: clip, variant | Moved median, p95 | Vibrato depth kept, rate moved | \
         LSD, MCD | Added clicks/min, octave jumps/min, fast warble p95, still notes |\n\
         |---|---|---|---|---|\n",
    );
    for (clip, key) in keys {
        let Some(variants) = key["variants"].as_object() else { continue };
        for (variant, row) in variants {
            let left = &row["left_alone"];
            writeln!(
                table,
                "| {clip} {variant} | {}, {} c | {}, {} Hz | {}, {} dB | {} |",
                number(&left["moved_median_cents"], 1),
                number(&left["moved_p95_cents"], 1),
                number(&left["vibrato_depth_kept"], 2),
                number(&left["vibrato_rate_moved_hz"], 2),
                number(&left["lsd_db"], 2),
                number(&left["mcd_db"], 2),
                artefacts(&left["artefacts"]),
            )
            .expect("a string takes writes");
        }
    }

    table.push_str(
        "\n| Restored: clip, variant | Error median, p95 | Held, onset, wobble | LSD, MCD | \
         Added clicks/min, octave jumps/min, fast warble p95, still notes |\n|---|---|---|---|---|\n",
    );
    for (clip, key) in keys {
        let floor = &key["floor"];
        let Some(variants) = key["variants"].as_object() else { continue };
        for (variant, row) in variants {
            let restored = &row["restored"];
            writeln!(
                table,
                "| {clip} {variant} | {}, {} c | {}, {}, {} c | {}, {} dB | {} |",
                above(&restored["median_cents"], &floor["pitch_median_cents"], 1),
                above(&restored["p95_cents"], &floor["pitch_p95_cents"], 1),
                above(&restored["held_median_cents"], &floor["pitch_median_cents"], 1),
                above(&restored["onset_median_cents"], &floor["pitch_median_cents"], 1),
                above(&restored["wobble_median_cents"], &floor["pitch_median_cents"], 1),
                above(&restored["lsd_db"], &floor["lsd_db"], 2),
                above(&restored["mcd_db"], &floor["mcd_db"], 2),
                artefacts(&restored["artefacts"]),
            )
            .expect("a string takes writes");
        }
    }
    table
}

/// What each take added that needs no answer key, per clip, condition and variant. A phone take's
/// rows are here; through Opus, `off` is what the codec path adds, and a corrector's share is its
/// row less `off`'s.
pub fn artefacts_table(result: &Value) -> String {
    let mut table = String::from(
        "| Clip, condition, variant | Added clicks/min, octave jumps/min, fast warble p95, still notes |\n\
         |---|---|\n",
    );
    let Some(clips) = result["clips"].as_object() else { return table };
    for (clip, conditions) in clips {
        let Some(conditions) = conditions.as_object() else { continue };
        for (condition, entry) in conditions {
            let Some(variants) = entry["variants"].as_object() else { continue };
            for (variant, row) in variants {
                writeln!(
                    table,
                    "| {clip} {condition} {variant} | {} |",
                    artefacts(&row["artefacts"])
                )
                .expect("a string takes writes");
            }
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    // No restored row prints a number under what the degrader alone costs
    #[test]
    fn a_number_under_the_floor_says_so() {
        assert_eq!(above(&json!(1.5), &json!(2.0), 1), "under floor 2.0");
        assert_eq!(above(&json!(2.5), &json!(2.0), 1), "2.5");
        assert_eq!(above(&Value::Null, &json!(2.0), 1), "-");
    }
}
