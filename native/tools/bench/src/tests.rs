use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use assert_no_alloc::assert_no_alloc;
use devices::{Clock as TimerClock, Feed, Pacer, Stats};
use engine::audio_thread::AudioThread;
use engine::block::{BLOCK, RATE, Stereo};
use engine::command::Command;
use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};
use engine::graph::Graph;
use engine::slot::Slot;
use engine::slot::source::Source;
use replay::bundle::report::{self, Options as Replay};
use replay::bundle::song::Song;
use serde_json::{Value, json};

use crate::cli::{Args, Clock};
use crate::play::{block_budget, play, run};

/// The timer clock's time, made up: each wait returns at once at its deadline, and every
/// seventh oversleeps by three periods, so the pacer catches up as it does after a late wake.
struct Synthetic {
    now: Duration,
    waits: u32,
}

impl TimerClock for Synthetic {
    fn now(&self) -> Duration {
        self.now
    }

    fn sleep_until(&mut self, deadline: Duration) {
        self.waits += 1;
        let late = if self.waits.is_multiple_of(7) { 3 * BLOCK as u32 } else { 0 };
        let late = Duration::from_secs_f64(f64::from(late) / f64::from(RATE));
        self.now = self.now.max(deadline) + late;
    }
}

/// A fake output on the timer clock's own pacer and the synthetic clock: it renders as fast as
/// the machine allows, every block under the allocation guard when `guarded`, and keeps what it
/// played.
struct FakeOutput {
    stats: Arc<Stats>,
    heard: Arc<Mutex<Vec<f32>>>,
    guarded: bool,
}

impl Driver for FakeOutput {
    fn start(self: Box<Self>, mut render: Render) -> Result<Running, DriverError> {
        let Self { stats, heard, guarded } = *self;
        stats.set_opened("a fake output".into());
        // The first lock may set the lock up, which allocates, so it is taken here and not in render
        drop(heard.lock());
        let keep: Render = Box::new(move |out: Stereo<'_>| {
            let Stereo { left, right } = out;
            render(Stereo { left: &mut *left, right: &mut *right });
            let mut heard = heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            for (l, r) in left.iter().zip(right.iter()) {
                if heard.len() < heard.capacity() {
                    heard.extend_from_slice(&[*l, *r]);
                }
            }
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                let mut feed = Feed::new(keep, Arc::clone(&stats));
                let mut pacer = Pacer::new(Synthetic { now: Duration::ZERO, waits: 0 }, stats);
                while !stop.load(Ordering::Acquire) {
                    if guarded {
                        assert_no_alloc(|| pacer.tick(&mut feed));
                    } else {
                        pacer.tick(&mut feed);
                    }
                }
            }
        });
        let info =
            StreamInfo { sample_rate: RATE, period_frames: BLOCK as u32, output_latency_frames: 0 };
        Ok(Running::new(info, move || {
            stop.store(true, Ordering::Release);
            thread.join().expect("the fake output's thread");
        }))
    }
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../scorecard/fixtures")
}

fn with_stems() -> Replay {
    Replay { profile: None, backing: Some(fixtures().join("stems")) }
}

fn args(bundle: PathBuf, out: Option<PathBuf>) -> Args {
    Args { bundle, replay: with_stems(), out, baseline: None, write: None, clock: Clock::Timer }
}

/// A fresh directory under the system's temporary one, named for this test run.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("aeon-bench-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Plays `args` through a fake output under the allocation guard, and what it played.
fn played(args: &Args, frames: usize) -> Result<Vec<f32>, String> {
    let heard = Arc::new(Mutex::new(Vec::with_capacity(frames)));
    let stats = Arc::new(Stats::new());
    let output = FakeOutput { stats: Arc::clone(&stats), heard: Arc::clone(&heard), guarded: true };
    play(args, Box::new(output), stats)?;
    Ok(std::mem::take(&mut *heard.lock().unwrap()))
}

#[test]
fn a_run_renders_what_replay_renders_and_its_bundle_replays_to_the_same_samples() {
    let duet = fixtures().join("bundles/duet");
    let (_, replayed) = report::play(&duet, &with_stems()).unwrap();
    let out = scratch("duet");
    let heard = played(&args(duet, Some(out.clone())), replayed.mix.len()).unwrap();
    assert_eq!(heard.len(), replayed.mix.len());
    assert!(heard == replayed.mix, "the run played other samples than replay");
    let (_, again) = report::play(&out, &with_stems()).unwrap();
    assert!(again.mix == replayed.mix, "the run's bundle replays to other samples");
    std::fs::remove_dir_all(&out).unwrap();
}

#[test]
fn a_song_that_replaces_its_guide_twenty_times_in_a_second_plays_as_replay_plays_it() {
    // The duet, with twenty more guides for its first slot 50 ms apart, each a minor third from
    // the last, so the corrector pulls its voice somewhere else at each: each retires the one
    // before it, more than the engine's retired ring holds
    let bundle = scratch("guides");
    let duet = fixtures().join("bundles/duet");
    std::fs::create_dir_all(bundle.join("slots")).unwrap();
    for file in ["manifest.json", "profile.json", "stats.jsonl", "slots/0.pcap", "slots/1.pcap"] {
        std::fs::copy(duet.join(file), bundle.join(file)).unwrap();
    }
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(duet.join("manifest.json")).unwrap())
            .unwrap();
    let (frame, us) = (manifest["frame"].as_f64().unwrap(), manifest["us"].as_f64().unwrap());
    let text = std::fs::read_to_string(duet.join("events.jsonl")).unwrap();
    let mut lines: Vec<Value> =
        text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    let guide = lines.iter().find(|line| line["type"] == "setSlotGuide").unwrap().clone();
    for k in 0..20 {
        let at = frame + f64::from(RATE) + f64::from(k * 2400);
        let mut line = guide.clone();
        for note in line["guide"]["notes"].as_array_mut().unwrap() {
            note["midi"] = json!(54 + 3 * (k % 2));
        }
        line["frame"] = json!(at);
        line["us"] = json!((us + (at - frame) * 1e6 / f64::from(RATE)).round());
        lines.push(line);
    }
    lines.sort_by(|a, b| a["frame"].as_f64().unwrap().total_cmp(&b["frame"].as_f64().unwrap()));
    let events: String = lines.iter().map(|line| format!("{line}\n")).collect();
    std::fs::write(bundle.join("events.jsonl"), events).unwrap();

    let (_, replayed) = report::play(&bundle, &with_stems()).unwrap();
    let heard = played(&args(bundle.clone(), None), replayed.mix.len()).unwrap();
    assert!(heard == replayed.mix, "the run played other samples than replay");
    std::fs::remove_dir_all(&bundle).unwrap();
}

/// A source that panics on its `at`th pull, as a kernel meeting an edge case would.
struct PanicsAt {
    pulls: usize,
    at: usize,
}

impl Source for PanicsAt {
    fn pull(&mut self, out: &mut [f32]) {
        self.pulls += 1;
        assert!(self.pulls < self.at, "a kernel's edge case");
        out.fill(0.0);
    }
}

#[test]
fn an_engine_that_faults_in_the_songs_last_second_refuses_the_run() {
    let duet = fixtures().join("bundles/duet");
    let (bundle, profile, stems) = report::load(&duet, &with_stems()).unwrap();
    let mut song = Song::prepare(&bundle, &profile, stems).unwrap();
    // The song's engine, swapped for one whose only slot panics 100 blocks before the end
    let (audio, mut controls) = AudioThread::new();
    let at = song.blocks - 100;
    let graph = Box::new(Graph::new(vec![Slot::new(Box::new(PanicsAt { pulls: 0, at }))]));
    assert!(controls.commands.push(Command::Swap { id: 1, graph }).is_ok());
    (song.audio, song.events, song.retired) = (audio, controls.events, controls.retired);

    // Not under the guard: the panic allocates its payload
    let stats = Arc::new(Stats::new());
    let heard = Arc::new(Mutex::new(Vec::new()));
    let output = FakeOutput { stats: Arc::clone(&stats), heard, guarded: false };
    let error = run(&args(duet, None), &bundle, song, None, Box::new(output), stats)
        .expect_err("a run whose engine faulted is refused");
    assert!(error.contains("the engine faulted"), "{error}");
}

#[test]
fn a_block_has_a_blocks_time_under_a_long_callback_and_the_callbacks_under_a_short_one() {
    let ms = |frames: u32| block_budget(frames).as_secs_f64() * 1000.0;
    // WASAPI shared grants 480 frames: each callback renders nearly four blocks back to back
    assert!((ms(480) - 2.667).abs() < 0.001, "{}", ms(480));
    assert!((ms(128) - 2.667).abs() < 0.001, "{}", ms(128));
    assert!((ms(64) - 1.333).abs() < 0.001, "{}", ms(64));
}
