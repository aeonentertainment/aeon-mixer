//! `bench play`: a bundle's song on a real clock, as `replay` plays it on the offline driver
//! (docs/design.md 8.9). The render hands each call and packet to the engine in the block
//! `replay` would, so the run renders what `replay` renders; what it measures is whether the
//! clock got every block in time.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devices::Stats;
use engine::block::{BLOCK, RATE};
use engine::command::Event;
use engine::driver::{Driver, Render};
use replay::bundle::report;
use replay::bundle::song::{Song, SongSlot};
use scorecard::card::Scorecard;
use scorecard::movement::{Change, Movement};
use serde_json::{Value, json};

use crate::cli::{Args, Clock};
use crate::render_times::RenderTimes;
use crate::run_bundle::{self, Run};

/// Plays the bundle `args` names on `driver`, whose stream records into `stats`, and prints what
/// it played, on what, and what it measured. Ok(false) when the run fell behind, or moved past
/// what the baseline allows.
pub fn play(
    args: &Args,
    driver: Box<dyn Driver + Send>,
    stats: Arc<Stats>,
) -> Result<bool, String> {
    let (bundle, profile, stems) = report::load(&args.bundle, &args.replay)?;
    let baseline = args.baseline.as_deref().map(Scorecard::read).transpose()?;
    if let Some(out) = args.out.as_deref().filter(|out| out.exists()) {
        return Err(format!("{} already exists; --out writes a new bundle", out.display()));
    }
    let stems_found = stems.is_some();
    let Song { mut audio, mut player, mut events, mut retired, frames, slots, blocks, notes } =
        Song::prepare(&bundle, &profile, stems)?;
    if blocks == 0 {
        return Err("the bundle holds no song to play".into());
    }
    let kinds: Vec<&str> = bundle.manifest.slots.iter().map(|slot| slot.kind.as_str()).collect();
    let seconds = (blocks * BLOCK) as f64 / f64::from(RATE);
    println!(
        "played  {}: {} slots ({}), {seconds:.1} s",
        args.bundle.display(),
        slots.len(),
        kinds.join(", ")
    );
    if let (true, Some(dir)) = (stems_found, &args.replay.backing) {
        println!("        with its stems from {}", dir.display());
    }
    for note in &notes {
        println!("        note: {note}");
    }

    let times = Arc::new(RenderTimes::default());
    let jammed = Arc::new(AtomicBool::new(false));
    let (finished, done) = mpsc::sync_channel(1);
    let render: Render = Box::new({
        let (times, jammed) = (Arc::clone(&times), Arc::clone(&jammed));
        let mut rendered = 0;
        move |out| {
            let began = Instant::now();
            // A full command ring wakes the control thread at once, which ends the run
            if !player.before_block() && !jammed.swap(true, Relaxed) {
                let _ = finished.try_send(());
            }
            audio.render(out);
            if rendered < blocks {
                times.record(began.elapsed());
                rendered += 1;
                // Once, after the song's last block: nothing is left on the clock to make late
                if rendered == blocks {
                    let _ = finished.try_send(());
                }
            }
        }
    });
    let us = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |t| t.as_micros() as i64);
    let running = driver.start(render).map_err(|e| format!("the output would not start: {e}"))?;
    let info = running.info;
    let opened = stats.opened().unwrap_or("an output").to_owned();
    let rate = f64::from(info.sample_rate);
    let latency_ms = f64::from(info.output_latency_frames) * 1000.0 / rate;
    println!(
        "on      {opened}: {rate} Hz, a {}-frame period, {latency_ms:.2} ms of output latency reported",
        info.period_frames
    );
    if args.clock == Clock::Timer {
        println!(
            "        the timer clock: nothing is heard, and --open-device plays it through an output"
        );
    }
    println!();

    let mut stats_lines = Vec::new();
    let mut second = 0;
    let stopped = loop {
        let waited = done.recv_timeout(Duration::from_secs(1));
        while retired.pop().is_some() {}
        let mut faulted = false;
        while let Some(event) = events.pop() {
            faulted |= event == Event::Fault;
        }
        let report = stats.report();
        match waited {
            _ if jammed.load(Relaxed) => {
                break Some(
                    "the engine's command ring was full, so the song from there on would not have been \
                 the bundle's",
                );
            }
            Ok(()) => break None,
            _ if faulted => break Some("the engine faulted"),
            _ if report.lost => break Some("the output went away"),
            _ if report.failed => break Some("the output failed"),
            Err(RecvTimeoutError::Disconnected) => break Some("the output stopped"),
            Err(RecvTimeoutError::Timeout) => {}
        }
        second += 1;
        println!("{second:>5} s  {}  underruns {}", slot_line(&slots), report.underruns);
        stats_lines.push((frames.load(Relaxed), stats_line(&slots)));
    };
    drop(running);
    if let Some(why) = stopped {
        return Err(format!("refused {second} s into the song: {why}. The run measured nothing"));
    }

    let underruns = stats.report().underruns;
    let period = Duration::from_secs_f64(f64::from(info.period_frames) / rate);
    let (p99, longest) = (times.percentile(0.99), times.longest());
    let kept_up = underruns == 0 && longest < period;
    println!("\nmeasured");
    println!("  underruns    {underruns:>8}     periods the output was not handed in time");
    println!("  render p99   {:>8.2} ms  of the {:.2} ms period", ms(p99), ms(period));
    println!("  render max   {:>8.2} ms", ms(longest));
    let spare = 100.0 * (1.0 - longest.as_secs_f64() / period.as_secs_f64());
    let verdict = match () {
        _ if kept_up => format!("kept up: every block rendered in time, the slowest with {spare:.0}% of its period to spare"),
        _ if underruns > 0 => format!("fell behind: the output went without {underruns} periods"),
        _ => "fell behind: a render took longer than its period, which a real output hears as a dropout".into(),
    };
    println!("verdict  {verdict}");

    let name = args.bundle.file_name().map_or("bundle".into(), |n| n.to_string_lossy().to_string());
    let mut numbers = std::collections::BTreeMap::new();
    numbers.insert("output.underruns".into(), underruns as f64);
    numbers.insert("output.renderP99Ms".into(), hundredths(ms(p99)));
    numbers.insert("output.renderMaxMs".into(), hundredths(ms(longest)));
    let mut card = Scorecard::new();
    card.add(&name, numbers, baseline.as_ref());
    if let Some(path) = &args.write {
        card.write(path)?;
        println!("wrote    this result to {}", path.display());
    }
    let held = baseline.is_none_or(|baseline| compare(&card, &baseline));
    if let Some(out) = &args.out {
        let profile_file = args.replay.profile.clone().unwrap_or(args.bundle.join("profile.json"));
        let profile_text = std::fs::read_to_string(&profile_file)
            .map_err(|e| format!("{}: {e}", profile_file.display()))?;
        let output = json!({
            "latencyMs": latency_ms,
            "name": opened,
            "periodFrames": info.period_frames,
            "sampleRate": info.sample_rate,
        });
        let run = Run { us, output, profile: &profile_text, stats: &stats_lines };
        run_bundle::write(out, &args.bundle, &bundle, &run)?;
        println!("wrote    this run's bundle to {}, which `scorecard` scores", out.display());
    }
    Ok(kept_up && held)
}

/// Each slot's counters as `stats()` gives them, for the line printed each second.
fn slot_line(slots: &[SongSlot]) -> String {
    let parts: Vec<String> = slots
        .iter()
        .enumerate()
        .filter_map(|(index, slot)| {
            let s = slot.stats.as_ref()?.read();
            Some(format!(
                "slot {index}: late {} concealed {} delay {:.1} ms",
                s.late, s.concealed, s.playout_delay_ms
            ))
        })
        .collect();
    parts.join("  ")
}

/// A `stats.jsonl` line's body: each slot's counters as `stats()` names them, null for a slot
/// with nothing to play.
fn stats_line(slots: &[SongSlot]) -> Value {
    let slots: Vec<Value> = slots
        .iter()
        .map(|slot| {
            slot.stats.as_ref().map_or(Value::Null, |stats| {
                let s = stats.read();
                json!({
                    "pushed": s.pushed, "lost": s.lost, "late": s.late,
                    "lateAtOpening": s.late_at_opening, "concealed": s.concealed,
                    "resyncs": s.resyncs, "jitterMs": s.jitter_ms,
                    "playoutDelayMs": s.playout_delay_ms,
                })
            })
        })
        .collect();
    json!({ "positionMs": null, "slots": slots })
}

/// Prints each number that moved against `baseline`, and whether every one held.
fn compare(card: &Scorecard, baseline: &Scorecard) -> bool {
    let movements = Movement::between(card, baseline);
    println!(
        "\nagainst the baseline{}",
        if movements.is_empty() { ", no number moved" } else { "" }
    );
    for movement in &movements {
        let held = if movement.fails() { "beyond its tolerance" } else { "within its tolerance" };
        let number = &movement.number;
        match movement.change {
            Change::Moved { baseline, now, tolerance } => {
                println!("  {number:<22} {now:.2}, was {baseline:.2}: {held} of {tolerance:.2}")
            }
            Change::Gone { baseline } => println!("  {number:<22} gone, was {baseline:.2}"),
            Change::New { now } => println!("  {number:<22} {now:.2}, new"),
        }
    }
    movements.iter().all(|movement| !movement.fails())
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}
