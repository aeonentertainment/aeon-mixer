//! A run's result as the table a PR quotes: each row beside the GStreamer baseline from Aeon
//! Karaoke's `docs/open-problems/auto-tune.md`. It reads only the result JSON, so a table can be
//! printed again from a saved run and nothing here can disagree with what `diff` compares.

use std::fmt::Write;

use serde_json::Value;

/// A number from the result, to `places` places; a dash when the row did not run.
fn number(value: &Value, places: usize) -> String {
    value.as_f64().map_or_else(|| "-".to_owned(), |n| format!("{n:.places$}"))
}

fn cell(table: &mut String, row: &str, bar: &str, baseline: &str, engine: &str) {
    writeln!(table, "| {row} | {bar} | {baseline} | {engine} |").expect("a string takes writes");
}

/// The baseline's numbers for one clip's intonation row: off, strength 0.6, full.
fn intonation_baseline(clip: &str) -> Option<[f64; 3]> {
    match clip {
        "vocadito_2" => Some([17.0, 8.0, 5.0]),
        "vocadito_6" => Some([30.0, 21.0, 19.0]),
        "vocadito_19" => Some([27.0, 19.0, 17.0]),
        "vocadito_10" => Some([27.0, 16.0, 19.0]),
        _ => None,
    }
}

/// The baseline's within-a-semitone percent on the three clips it read.
fn detector_baseline(clip: &str) -> Option<f64> {
    match clip {
        "vocadito_2" => Some(95.4),
        "vocadito_6" => Some(97.7),
        "vocadito_19" => Some(97.6),
        _ => None,
    }
}

pub fn table(result: &Value) -> String {
    let mut table = String::new();
    table.push_str("| Row | Bar | GStreamer baseline | This engine |\n|---|---|---|---|\n");

    let latency = &result["latency"];
    cell(
        &mut table,
        "Added latency, off",
        "under 15 ms",
        "0 ms",
        &format!("{} ms", number(&latency["off_ms"], 2)),
    );
    cell(
        &mut table,
        "Added latency, correcting at rest",
        "under 15 ms",
        "1.2 ms",
        &format!(
            "{} ms; whole chain with reverb {} ms",
            number(&latency["at_rest_ms"], 2),
            number(&latency["whole_chain_at_rest_ms"], 2)
        ),
    );
    let golden = &latency["golden_vocal"];
    let span =
        |stat: &Value| format!("{} to {}", number(&stat["lowest"], 1), number(&stat["highest"], 1));
    let golden_row = |row: &Value| {
        format!(
            "mean {}, p95 {}, max {} ms",
            span(&row["mean_ms"]),
            span(&row["p95_ms"]),
            span(&row["max_ms"])
        )
    };
    cell(
        &mut table,
        "Added latency, correcting the golden vocal, over four start offsets",
        "under 15 ms",
        "mean 4.9, p95 8.3, max 10.1 ms",
        &if golden.is_null() {
            "needs --golden".to_owned()
        } else {
            format!(
                "key of C as the harness asked: {}; the song's own G: {}; chromatic: {}",
                golden_row(&golden["key_c"]),
                golden_row(&golden["song_key_g"]),
                golden_row(&golden["chromatic"])
            )
        },
    );
    let at = |hz: &str| {
        let row = &latency["range"][hz];
        format!(
            "{} ms, lands {} cents",
            number(&row["max_delay_ms"], 1),
            number(&row["landed_cents"], 1)
        )
    };
    cell(
        &mut table,
        "Delay bound on a held tone",
        "under 15 ms",
        "11.4 ms at 110 Hz, 14.9 ms at 82 Hz",
        &format!("110 Hz: {}; 82.5 Hz: {}", at("110hz"), at("82.5hz")),
    );

    let cpu = &result["cpu"];
    if !cpu.is_null() {
        let spent = |row: &str| {
            format!("{} (best {})", number(&cpu[row]["median"], 2), number(&cpu[row]["min"], 2))
        };
        cell(
            &mut table,
            "CPU, the core alone",
            "under 5 % of a core",
            "0.28 %",
            &format!("{} %", spent("core_correcting")),
        );
        cell(
            &mut table,
            "CPU, a slot correcting",
            "under 5 % of a core",
            "element 0.61 %, slot tail 1.10 %",
            &format!(
                "{} %, bypassed {} %, four slots {} %",
                spent("slot_correcting"),
                spent("slot_bypassed"),
                spent("four_slots_correcting")
            ),
        );
    }

    let sync = &result["sync"];
    cell(
        &mut table,
        "Melody sync, target change",
        "within one 10 ms frame",
        "every change within 0.021 ms of its edge",
        &format!(
            "{} of {} target changes reach the output, the latest {} frames after its edge ({} ms)",
            number(&sync["target_changes_seen"], 0),
            number(&sync["target_changes"], 0),
            number(&sync["latest_frames"], 0),
            number(&sync["latest_ms"], 3)
        ),
    );
    cell(
        &mut table,
        "Melody sync, output crosses halfway",
        "within one 10 ms frame",
        "within 6 ms",
        &format!("within {} ms", number(&sync["halfway_worst_ms"], 1)),
    );

    let Some(clips) = result["clips"].as_object() else { return table };
    for (clip, conditions) in clips {
        let detector = &conditions["direct"]["detector"];
        if detector.is_null() {
            continue;
        }
        let baseline = detector_baseline(clip)
            .map_or("not measured".to_owned(), |percent| format!("{percent} %"));
        cell(
            &mut table,
            &format!("Detector, {clip}"),
            "over 95 % within a semitone",
            &format!("{baseline}, median 2 to 5 cents"),
            &format!(
                "{} %, median {} cents; corrector engages on {} % of voiced frames",
                number(&detector["within_50_cents_pct"], 1),
                number(&detector["median_cents"], 1),
                number(&detector["engaged_pct"], 1)
            ),
        );
    }
    for (clip, conditions) in clips {
        let Some([off, natural, full]) = intonation_baseline(clip) else { continue };
        let variants = &conditions["direct"]["variants"];
        let read = |variant: &str| {
            let row = &variants[variant];
            format!(
                "{} ({} to {})",
                number(&row["median_cents"], 1),
                number(&row["start_offset_lowest_cents"], 1),
                number(&row["start_offset_highest_cents"], 1)
            )
        };
        cell(
            &mut table,
            &format!("Intonation, {clip}: off, 0.6, 1.0"),
            "none, it informs the listen",
            &format!("{off:.0}, {natural:.0}, {full:.0} cents"),
            &format!("{}, {}, {} cents", read("off"), read("melody-0.6"), read("melody-1.0")),
        );
    }
    for (clip, conditions) in clips {
        let variants = &conditions["opus-lan"]["variants"];
        if variants.is_null() || clip == "phone" {
            continue;
        }
        let read = |variant: &str| number(&variants[variant]["median_cents"], 1);
        let no_dtx = &conditions["opus-lan-no-dtx"]["variants"];
        let wifi = &conditions["opus-wifi"]["variants"];
        cell(
            &mut table,
            &format!("Intonation through Opus, {clip} (not a phone clip): off, 0.6, 1.0"),
            "none",
            "not measured",
            &format!(
                "LAN {}, {}, {}; LAN without DTX {}, {}, {}; Wi-Fi {}, {}, {} cents",
                read("off"),
                read("melody-0.6"),
                read("melody-1.0"),
                number(&no_dtx["off"]["median_cents"], 1),
                number(&no_dtx["melody-0.6"]["median_cents"], 1),
                number(&no_dtx["melody-1.0"]["median_cents"], 1),
                number(&wifi["off"]["median_cents"], 1),
                number(&wifi["melody-0.6"]["median_cents"], 1),
                number(&wifi["melody-1.0"]["median_cents"], 1)
            ),
        );
    }
    let phone = clips.get("phone");
    cell(
        &mut table,
        "Intonation on a phone clip: off, 0.6, 1.0",
        "none",
        "not measured",
        &phone.map_or("pending: needs a person's recording".to_owned(), |conditions| {
            let variants = &conditions["opus-lan"]["variants"];
            let read = |variant: &str| number(&variants[variant]["median_cents"], 1);
            format!("{}, {}, {} cents", read("off"), read("melody-0.6"), read("melody-1.0"))
        }),
    );
    cell(
        &mut table,
        "Artefacts: blind listen",
        "melody preferred or equal to off by all three; no warble",
        "not run",
        "pending: needs three listeners",
    );
    table
}

/// What correction did to the voice's movement, per clip. No baseline exists for these.
pub fn expression_table(result: &Value) -> String {
    let mut table = String::new();
    table.push_str(
        "| Clip, condition, variant | Median miss | In tune | Motion kept | Added motion p95 | \
         Vibrato kept | Added level p95 |\n|---|---|---|---|---|---|---|\n",
    );
    let Some(clips) = result["clips"].as_object() else { return table };
    for (clip, conditions) in clips {
        let Some(conditions) = conditions.as_object() else { continue };
        for (condition, entry) in conditions {
            let Some(variants) = entry["variants"].as_object() else { continue };
            for (variant, row) in variants {
                writeln!(
                    table,
                    "| {clip} {condition} {variant} | {} c | {} % | {} | {} c | {} ({} notes) | {} dB |",
                    number(&row["median_cents"], 1),
                    number(&row["in_tune_pct"], 0),
                    number(&row["motion_kept"], 2),
                    number(&row["motion_added_p95_cents"], 1),
                    number(&row["vibrato_kept"], 2),
                    number(&row["vibrato_notes"], 0),
                    number(&row["envelope_added_p95_db"], 2),
                )
                .expect("a string takes writes");
            }
        }
    }
    table
}

/// What a known voice does: the rows #45 and #46 are judged on before there is a listen.
pub fn known_voices_table(result: &Value) -> String {
    let mut table = String::from("| Known voice, setting | Reads |\n|---|---|\n");
    let voices = &result["known_voices"];
    if let Some(rows) = voices["vibrato"].as_object() {
        for (row, value) in rows {
            writeln!(
                table,
                "| 30 cents flat, 50 cents of vibrato, {row} | mean {} cents from the note, depth kept {} |",
                number(&value["mean_cents_from_target"], 1),
                number(&value["depth_kept"], 2),
            )
            .expect("a string takes writes");
        }
    }
    if let Some(rows) = voices["scoop"].as_object() {
        for (row, value) in rows {
            writeln!(
                table,
                "| a 100 cent scoop over 150 ms, {row} | within 10 cents {} ms after it ends |",
                number(&value["settle_ms"], 0),
            )
            .expect("a string takes writes");
        }
    }
    if let Some(rows) = voices["on_the_fence"].as_object() {
        for (row, value) in rows {
            writeln!(
                table,
                "| vibrato across the midpoint of two notes, {row} | motion {} cents in, {} out |",
                number(&value["motion_in_cents"], 1),
                number(&value["motion_out_cents"], 1),
            )
            .expect("a string takes writes");
        }
    }
    if let Some(clips) = result["detector_windows"].as_object() {
        for (clip, windows) in clips {
            let read = |window: &str, field: &str| number(&windows[window][field], 1);
            writeln!(
                table,
                "| The detector alone on {clip}, windows 1024 and 2048 | within a semitone {} and {} %, median {} and {} cents, engaged {} and {} % |",
                read("1024", "within_50_cents_pct"),
                read("2048", "within_50_cents_pct"),
                read("1024", "median_cents"),
                read("2048", "median_cents"),
                read("1024", "engaged_pct"),
                read("2048", "engaged_pct"),
            )
            .expect("a string takes writes");
        }
    }
    table
}
