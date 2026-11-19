//! `bench play`: a bundle's song on a real clock, as `replay` plays it on the offline driver
//! (docs/design.md 8.9). The render hands each call and packet to the engine in the block
//! `replay` would, so the run renders what `replay` renders; what it measures is whether the
//! clock got every block in time.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devices::Stats;
use engine::block::{BLOCK, RATE};
use engine::command::Event;
use engine::driver::{Driver, Render};
use replay::bundle::song::{Song, SongSlot};
use replay::bundle::{Bundle, report};
use rtrb::RingBuffer;
use scorecard::card::Scorecard;
use serde_json::{Value, json};

use crate::cli::{Args, Clock};
use crate::render_times::RenderTimes;
use crate::result;
use crate::run_bundle::{self, Run};

/// Plays the bundle `args` names on `driver`, whose stream records into `stats`, and prints what
/// it played, on what, and what it measured. Ok(false) when the run fell behind, or moved past
/// what the baseline allows.
pub fn play(
    args: &Args,
    driver: Box<dyn Driver + Send>,
    stats: Arc<Stats>,
) -> Result<bool, String> {
    if !args.bundle.join("manifest.json").is_file() {
        return Err(format!(
            "{} is not a session bundle bench can play: it holds no manifest.json",
            args.bundle.display()
        ));
    }
    // Each error past here names the file it could not read: the bundle's, the stems', the profile
    let (bundle, profile, stems) = report::load(&args.bundle, &args.replay)?;
    let baseline = result::baseline(args.baseline.as_deref())?;
    result::writable(args.write.as_deref())?;
    if let Some(out) = args.out.as_deref().filter(|out| out.exists()) {
        return Err(format!("{} already exists; --out writes a new bundle", out.display()));
    }
    let out_folder =
        args.out.as_deref().and_then(Path::parent).filter(|f| !f.as_os_str().is_empty());
    if let Some(folder) = out_folder.filter(|folder| !folder.is_dir()) {
        return Err(format!("--out: the folder {} does not exist", folder.display()));
    }
    let stems_found = stems.is_some();
    let song = Song::prepare(&bundle, &profile, stems)?;
    let blocks = song.blocks;
    if blocks == 0 {
        return Err("the bundle holds no song to play".into());
    }
    let kinds: Vec<&str> = bundle.manifest.slots.iter().map(|slot| slot.kind.as_str()).collect();
    let seconds = (blocks * BLOCK) as f64 / f64::from(RATE);
    println!(
        "played  {}: {} slots ({}), {seconds:.1} s",
        args.bundle.display(),
        song.slots.len(),
        kinds.join(", ")
    );
    if let (true, Some(dir)) = (stems_found, &args.replay.backing) {
        println!("        with its stems from {}", dir.display());
    }
    for note in &song.notes {
        println!("        note: {note}");
    }

    run(args, &bundle, song, baseline, driver, stats)
}

/// Why the render woke the control thread.
enum Finished {
    /// The song's last block has rendered.
    Song,
    /// A command could not reach the engine in its block, so the song from there on is not the
    /// bundle's.
    Diverged,
}

/// Retired graphs and guides the bench holds for the control thread to free: a song's worth and
/// more, where the engine's own ring holds 16 and holds commands back once it is full.
const GRAVE: usize = 4096;

/// Plays `song`, prepared from `bundle`, on `driver`. The part of `play` a test can hand a song of
/// its own making.
pub fn run(
    args: &Args,
    bundle: &Bundle,
    song: Song,
    baseline: Option<Scorecard>,
    driver: Box<dyn Driver + Send>,
    stats: Arc<Stats>,
) -> Result<bool, String> {
    let Song { mut audio, mut player, mut events, mut retired, frames, slots, blocks, .. } = song;
    let times = Arc::new(RenderTimes::default());
    let (finished, done) = mpsc::sync_channel(1);
    let (mut grave, mut graves) = RingBuffer::new(GRAVE);
    let render: Render = Box::new({
        let times = Arc::clone(&times);
        let (mut rendered, mut stuck) = (0, None);
        move |out| {
            let began = Instant::now();
            if !player.before_block() {
                let _ = finished.try_send(Finished::Diverged);
            }
            audio.render(out);
            // Each block, as `replay` empties the engine's retired ring each block, so the engine
            // never holds a command back for want of room in it, however seldom the control thread
            // wakes. What finds no room is kept, never freed here, and ends the run
            while stuck.is_none()
                && let Some(old) = retired.pop()
            {
                if let Err(rtrb::PushError::Full(old)) = grave.push(old) {
                    stuck = Some(old);
                    let _ = finished.try_send(Finished::Diverged);
                }
            }
            if rendered < blocks {
                times.record(began.elapsed());
                rendered += 1;
                // Once, after the song's last block: nothing is left on the clock to make late
                if rendered == blocks {
                    let _ = finished.try_send(Finished::Song);
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
    let (mut second, mut faulted) = (0, false);
    let stopped = loop {
        let woke = done.recv_timeout(Duration::from_secs(1));
        while graves.pop().is_ok() {}
        while let Some(event) = events.pop() {
            faulted |= event == Event::Fault;
        }
        let report = stats.report();
        // Whatever went wrong before the song's end refuses the run, whichever wake sees it
        match woke {
            _ if faulted => break Some("the engine faulted"),
            _ if report.lost => break Some("the output went away"),
            _ if report.failed => break Some("the output failed"),
            Ok(Finished::Diverged) => {
                break Some(
                    "a command could not reach the engine in its block, so the song from there on \
                 would not have been the bundle's",
                );
            }
            Ok(Finished::Song) => break None,
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
    let budget = block_budget(info.period_frames);
    let (p99, longest) = (times.percentile(0.99), times.longest());
    let kept_up = underruns == 0 && longest < budget;
    println!("\nmeasured");
    println!("  underruns    {underruns:>8}     periods the output was not handed in time");
    println!("  render p99   {:>8.2} ms  of the {:.2} ms a block has", ms(p99), ms(budget));
    println!("  render max   {:>8.2} ms", ms(longest));
    let spare = 100.0 * (1.0 - longest.as_secs_f64() / budget.as_secs_f64());
    let verdict = match () {
        _ if kept_up => format!("kept up: every block rendered in time, the slowest with {spare:.0}% of its budget to spare"),
        _ if underruns > 0 => format!("fell behind: the output went without {underruns} periods"),
        _ => "fell behind: a block's render took longer than its budget, which a real output hears as a dropout".into(),
    };
    println!("verdict  {verdict}");

    let name = args.bundle.file_name().map_or("bundle".into(), |n| n.to_string_lossy().to_string());
    let numbers = [
        ("output.underruns", underruns as f64),
        ("output.renderP99Ms", ms(p99)),
        ("output.renderMaxMs", ms(longest)),
    ];
    let held = result::settle(&name, numbers, baseline.as_ref(), args.write.as_deref())?;
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
        run_bundle::write(out, &args.bundle, bundle, &run)?;
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

/// The time one block's render has under an output granted `period_frames` at a callback: the
/// period itself when it is shorter than a block, since a block must render inside one callback,
/// and a block's own length when it is longer, since a callback then renders several back to
/// back.
pub fn block_budget(period_frames: u32) -> Duration {
    let frames = period_frames.min(BLOCK as u32);
    Duration::from_secs_f64(f64::from(frames) / f64::from(RATE))
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}
