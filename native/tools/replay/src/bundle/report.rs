//! `replay BUNDLE_DIR`: plays a bundle and prints what the replay's own slots counted and
//! metered, as text for a person or as JSON for a scorer (docs/design.md 8.11).

use std::path::{Path, PathBuf};

use engine::block::RATE;
use serde_json::json;

use super::Bundle;
use super::profile::Profile;
use super::song::{self, Played};
use super::stems;
use crate::levels::Levels;
use crate::wav;

#[derive(Default, PartialEq)]
pub struct Options {
    pub profile: Option<PathBuf>,
    pub backing: Option<PathBuf>,
    pub json: bool,
}

/// Replays the bundle in `dir`, writing its mix into `wav` when given.
pub fn run(dir: &Path, options: &Options, wav: Option<&Path>) -> Result<(), String> {
    let bundle = Bundle::read(dir)?;
    let profile = match &options.profile {
        Some(path) => {
            let name = path.display().to_string();
            let text = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
            Profile::parse(&name, &text)?
        }
        None => bundle.profile.clone(),
    };
    let stems = match (&options.backing, &bundle.manifest.backing) {
        (Some(stems_dir), Some(backing)) => Some(stems::find(stems_dir, backing)?),
        _ => None,
    };
    let played = song::play(&bundle, &profile, stems)?;
    if let Some(out) = wav {
        wav::write(&out.join("mix.wav"), &played.mix, 2)?;
    }
    if options.json {
        println!("{}", as_json(&played));
    } else {
        print_text(dir, &bundle, &played);
        if let Some(out) = wav {
            println!("\nwrote mix.wav in {}", out.display());
        }
    }
    Ok(())
}

fn as_json(played: &Played) -> serde_json::Value {
    let levels = |levels: Levels| {
        json!({
            "topTenthDb": levels.top_tenth_db,
            "medianDb": levels.median_db,
            "loudestDb": levels.loudest_db,
            "readings": levels.readings,
        })
    };
    let slots: Vec<_> = played
        .slots
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            let s = slot.stats;
            json!({
                "index": index,
                "kind": slot.kind,
                "stats": {
                    "pushed": s.pushed,
                    "lost": s.lost,
                    "late": s.late,
                    "lateAtOpening": s.late_at_opening,
                    "concealed": s.concealed,
                    "resyncs": s.resyncs,
                    "jitterMs": s.jitter_ms,
                    "playoutDelayMs": s.playout_delay_ms,
                },
                "ringFull": slot.ring_full,
                "level": levels(Levels::of_readings(&slot.meter_db)),
            })
        })
        .collect();
    json!({
        "seconds": played.mix.len() as f64 / 2.0 / f64::from(RATE),
        "slots": slots,
        "master": levels(Levels::of_readings(&played.master_db)),
        "notes": played.notes,
    })
}

fn print_text(dir: &Path, bundle: &Bundle, played: &Played) {
    println!(
        "{}: a song from package {}, {} slots, {:.1} s",
        dir.display(),
        bundle.manifest.package,
        played.slots.len(),
        played.mix.len() as f64 / 2.0 / f64::from(RATE),
    );
    for note in &played.notes {
        println!("note: {note}");
    }
    for (index, slot) in played.slots.iter().enumerate() {
        let s = slot.stats;
        println!(
            "slot {index} {}: pushed {} lost {} late {} concealed {} resyncs {} jitter {:.2} ms, \
             delay {:.2} ms, ring full {}",
            slot.kind,
            s.pushed,
            s.lost,
            s.late,
            s.concealed,
            s.resyncs,
            s.jitter_ms,
            s.playout_delay_ms,
            slot.ring_full
        );
    }
    println!("\n100 ms RMS, dBFS     top tenth   median   loudest   readings");
    let meters = played
        .slots
        .iter()
        .enumerate()
        .map(|(index, slot)| (format!("slot {index} meter"), &slot.meter_db))
        .chain(std::iter::once(("master meter".to_owned(), &played.master_db)));
    for (name, readings) in meters {
        let levels = Levels::of_readings(readings);
        println!(
            "{name:<18} {:>9.1} {:>8.1} {:>9.1} {:>10}",
            levels.top_tenth_db, levels.median_db, levels.loudest_db, levels.readings
        );
    }
}
