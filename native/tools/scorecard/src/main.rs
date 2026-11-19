//! scorecard: replays session bundles on the offline driver and turns each into numbers, then
//! reports every number that moved against a baseline scorecard (docs/design.md 8.9). It opens
//! no device and no socket, and the same bundle scores to the same numbers every time.

mod bundles;
mod card;
mod movement;
mod numbers;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::process::ExitCode;

use replay::bundle::report::{self, Options as Replay};

use crate::card::Scorecard;
use crate::movement::{Change, Movement};

const USAGE: &str = "\
usage: scorecard BUNDLE... [--profile FILE] [--backing STEMS] [--baseline FILE] [--write FILE]

  BUNDLE           a session bundle's directory, or a directory of them
  --profile FILE   replay under this settings profile, not each bundle's own
  --backing STEMS  a directory holding the songs' stems, found by hash; without it each mic path
                   plays alone
  --baseline FILE  an earlier scorecard: report each number that moved, and fail on any it does
                   not allow
  --write FILE     write this scorecard, with the baseline's tolerances where it has them";

struct Options {
    bundles: Vec<PathBuf>,
    replay: Replay,
    baseline: Option<PathBuf>,
    write: Option<PathBuf>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options =
        Options { bundles: Vec::new(), replay: Replay::default(), baseline: None, write: None };
    while let Some(arg) = args.next() {
        let mut value = || args.next().map(PathBuf::from).ok_or(format!("{arg} needs a value"));
        match arg.as_str() {
            "--profile" => options.replay.profile = Some(value()?),
            "--backing" => options.replay.backing = Some(value()?),
            "--baseline" => options.baseline = Some(value()?),
            "--write" => options.write = Some(value()?),
            flag if flag.starts_with("--") => return Err(format!("no flag {flag}")),
            path => options.bundles.push(PathBuf::from(path)),
        }
    }
    if options.bundles.is_empty() {
        return Err("name a bundle".into());
    }
    Ok(options)
}

/// Scores every bundle `options` names, against the baseline when given.
fn score(options: &Options, baseline: Option<&Scorecard>) -> Result<Scorecard, String> {
    let mut card = Scorecard::new();
    for (name, dir) in bundles::find(&options.bundles)? {
        let (_, played) =
            report::play(&dir, &options.replay).map_err(|e| format!("{name}: {e}"))?;
        for note in &played.notes {
            println!("{name}: note: {note}");
        }
        card.add(&name, numbers::of(&played), baseline);
    }
    Ok(card)
}

/// Prints the scorecard, or what moved against the baseline, and whether the run passes.
fn run(options: &Options) -> Result<bool, String> {
    let baseline = options.baseline.as_deref().map(Scorecard::read).transpose()?;
    let card = score(options, baseline.as_ref())?;
    if let Some(path) = &options.write {
        card.write(path)?;
    }
    let Some(baseline) = baseline else {
        for (bundle, entries) in &card.bundles {
            println!("\n{bundle}");
            for (number, entry) in entries {
                println!("  {number:<30} {:>10.2}", entry.value);
            }
        }
        return Ok(true);
    };
    let movements = Movement::between(&card, &baseline);
    if movements.is_empty() {
        println!("no number moved");
        return Ok(true);
    }
    println!("{:<42} {:>10} {:>10} {:>10} {:>10}", "", "baseline", "now", "change", "tolerance");
    for movement in &movements {
        let name = format!("{} {}", movement.bundle, movement.number);
        let verdict = if movement.fails() { "  beyond" } else { "" };
        match movement.change {
            Change::Moved { baseline, now, tolerance } => println!(
                "{name:<42} {baseline:>10.2} {now:>10.2} {:>+10.2} {tolerance:>10.2}{verdict}",
                now - baseline
            ),
            Change::Gone { baseline } => {
                println!("{name:<42} {baseline:>10.2} {:>10}{:>22}{verdict}", "gone", "")
            }
            Change::New { now } => println!("{name:<42} {:>10} {now:>10.2}", "new"),
        }
    }
    let failing = movements.iter().filter(|movement| movement.fails()).count();
    println!("\n{} moved, {failing} beyond what the baseline allows", movements.len());
    Ok(failing == 0)
}

fn main() -> ExitCode {
    match parse(std::env::args().skip(1)).and_then(|options| run(&options)) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

// Tests replay every block in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
