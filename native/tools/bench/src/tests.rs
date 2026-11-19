use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_no_alloc::assert_no_alloc;
use devices::{
    Capture, CaptureInfo, Captured, Capturing, Clock as TimerClock, Feed, Pacer, Sink, Stats,
};
use engine::audio_thread::AudioThread;
use engine::block::{BLOCK, RATE, Stereo};
use engine::command::Command;
use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};
use engine::graph::Graph;
use engine::ingest::capture_voice;
use engine::slot::Slot;
use engine::slot::source::Source;
use replay::bundle::report::{self, Options as Replay};
use replay::bundle::song::Song;
use scorecard::card::Scorecard;
use serde_json::{Value, json};

use measure_latency::physical::Physical;

use crate::cli::{Args, Clock, LoopbackArgs};
use crate::loopback::{Loopback, Measured, command, loopback};
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

/// What the fake output played reaches the fake input `delay` frames later, as down a cable. The
/// output is granted `period` frames at a callback, and the input delivers what has reached it
/// after each callback, as `input` says: an input on the output's clock with the output's period,
/// as a host opens one. It renders as fast as the machine allows, every callback under the
/// allocation guard.
struct FakeLoop {
    delay: usize,
    period: usize,
    input: Behaves,
    sink: Arc<Mutex<Option<Sink>>>,
    stats: Arc<Stats>,
}

/// Frames of the fake cable's ring: far past its length and the most a late wake holds back.
const TAPE: usize = 1 << 16;

/// How the fake input behaves, each time counted in frames the output plays from an event of the
/// input's own, so no run depends on when the control thread gets to it.
#[derive(Clone, Copy)]
struct Behaves {
    /// Frames from its opening to its first delivery; never, for one that delivers nothing.
    starts_after: Option<usize>,
    /// Frames from the first chirp reaching it to its last delivery, for one that stops.
    stops_after_a_chirp: Option<usize>,
    /// Frames from the first chirp reaching it to a wake that holds back its last few frames, and
    /// how many, handed over at the next wake, as the phase of an input on its own clock does now
    /// and then.
    late_wake_after_a_chirp: Option<(usize, usize)>,
    /// A quiet noise floor under what it hears, as a microphone has; without it the loop is
    /// lossless, an exact copy of the output, as S/PDIF, ADAT or a virtual cable is.
    hiss: bool,
    /// Delivers digital silence on time, as a microphone macOS has refused this terminal does.
    mute: bool,
}

/// An input that delivers from its opening on, losslessly.
const LOSSLESS: Behaves = Behaves {
    starts_after: Some(0),
    stops_after_a_chirp: None,
    late_wake_after_a_chirp: None,
    hiss: false,
    mute: false,
};

/// About -86 dBFS RMS of white noise: deterministic, so every run hears the same floor.
fn hiss(state: &mut u64) -> f32 {
    *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    ((*state >> 33) as f32 / (1u64 << 31) as f32 * 2.0 - 1.0) * 1e-4
}

struct FakeInput {
    sink: Arc<Mutex<Option<Sink>>>,
    period: u32,
}

impl Capture for FakeInput {
    fn start(self: Box<Self>, sink: Sink) -> Result<Capturing, DriverError> {
        *self.sink.lock().unwrap() = Some(sink);
        let info = CaptureInfo {
            sample_rate: RATE,
            period_frames: self.period,
            channels: 1,
            input_latency_frames: None,
        };
        Ok(Capturing::new(info, || {}))
    }
}

impl Driver for FakeLoop {
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let Self { delay, period, input, sink, stats } = *self;
        // The first lock may set the lock up, which allocates, so it is taken here
        drop(sink.lock());
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                let mut feed = Feed::new(render, stats);
                // What reached the cable, by the frame it left the output: a ring, since only the
                // cable's length and a wake or two are ever still to deliver
                let mut tape = vec![0.0f32; TAPE];
                let silence = vec![0.0f32; TAPE];
                let (mut played, mut delivered, mut noise) = (0usize, 0, 1u64);
                let (mut opened, mut chirped, mut woke_late) = (None, None, false);
                while !stop.load(Ordering::Acquire) {
                    assert_no_alloc(|| {
                        feed.take(period, |_, left, _| {
                            for sample in left {
                                chirped = chirped.or((*sample != 0.0).then_some(played));
                                let floor = if input.hiss { hiss(&mut noise) } else { 0.0 };
                                tape[played % TAPE] = sample + floor;
                                played += 1;
                            }
                        });
                        let reached = played.saturating_sub(delay);
                        let mut sink = sink.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        if opened.is_none() && sink.is_some() {
                            opened = Some(played);
                        }
                        let since = |at: usize| played.saturating_sub(at);
                        let started = opened
                            .zip(input.starts_after)
                            .is_some_and(|(opened, after)| since(opened) >= after);
                        let stopped = input
                            .stops_after_a_chirp
                            .zip(chirped)
                            .is_some_and(|(after, at)| since(at + delay) >= after);
                        let held = match input.late_wake_after_a_chirp.zip(chirped) {
                            Some(((after, frames), at))
                                if !woke_late && since(at + delay) >= after =>
                            {
                                woke_late = true;
                                frames
                            }
                            _ => 0,
                        };
                        // A late wake holds back its last frames; the next hands them over
                        let reached = reached.saturating_sub(held).max(delivered);
                        if let Some(sink) = sink.as_mut()
                            && started
                            && !stopped
                        {
                            let mut from = delivered;
                            while from < reached {
                                let start = from % TAPE;
                                let end = (start + reached - from).min(TAPE);
                                let samples = if input.mute { &silence } else { &tape };
                                sink(Captured {
                                    samples: &samples[start..end],
                                    at: Instant::now(),
                                });
                                from += end - start;
                            }
                        }
                        // Before it starts the input delivers nothing, and then only what follows
                        delivered = reached;
                    });
                }
            }
        });
        let info = StreamInfo {
            sample_rate: RATE,
            period_frames: period as u32,
            output_latency_frames: 0,
        };
        Ok(Running::new(info, move || {
            stop.store(true, Ordering::Release);
            thread.join().expect("the fake loop's thread");
        }))
    }
}

/// A loopback through a cable `delay` frames long under an output granted `period` frames, into
/// an input that behaves as `input` says.
fn looped_into(delay: usize, period: usize, input: Behaves) -> Result<Measured, String> {
    let sink = Arc::new(Mutex::new(None));
    let (output_stats, input_stats) = (Arc::new(Stats::new()), Arc::new(Stats::new()));
    let stats = Arc::clone(&output_stats);
    let output = FakeLoop { delay, period, input, sink: Arc::clone(&sink), stats };
    let args = Loopback { rig: Physical::Cable, level_db: -12.0 };
    let opens = |period| -> Box<dyn Capture + Send> { Box::new(FakeInput { sink, period }) };
    loopback(&args, Box::new(output), output_stats, opens, input_stats)
}

/// The same, through a lossless loop that delivers from its opening on.
fn looped(delay: usize, period: usize) -> Measured {
    looped_into(delay, period, LOSSLESS).unwrap()
}

#[test]
fn a_lossless_loop_reads_back_as_its_delay_and_the_capture_slots_target_to_the_frame() {
    let frame_ms = 1000.0 / f64::from(RATE);
    // The input is an exact copy of the output, zeros until the first chirp, as S/PDIF or a virtual
    // cable is. Under a block's callback, an input that delivers each block as it arrives leaves a sample in
    // the ring for the slot's target fill and nothing else. The slot reports the middle of a span
    // one input period wide, a sample waiting up to a period on the hardware first, and this
    // input's samples wait none of it: they come back at the span's bottom
    let target = capture_voice().1.target_frames() as f64;
    for delay in [300, 300 + BLOCK / 2, 300 + 7 * BLOCK] {
        let run = looped(delay, BLOCK);
        let added = run.round_trip_ms / frame_ms - delay as f64;
        assert!((added - target).abs() <= 1.0, "{delay}: added {added:.2} frames");
        let bottom = run.slot_ms / frame_ms - BLOCK as f64 / 2.0;
        assert!((bottom - target).abs() <= 1.0, "{delay}: reported span's bottom {bottom:.2}");
    }
}

#[test]
fn under_a_longer_callback_than_asked_for_the_slot_is_set_up_for_the_one_granted() {
    let frame_ms = 1000.0 / f64::from(RATE);
    // WASAPI shared grants 480 frames whatever is asked. A slot set up for 480 waits 16 ms by
    // decision 0018, its target and headroom, and a sample comes back within half an input
    // period of that either side
    for delay in [300, 300 + 64] {
        let run = looped(delay, 480);
        assert!((run.slot_ms - 16.0).abs() < 0.01, "{delay}: the slot reports {}", run.slot_ms);
        assert!((run.period_ms - 10.0).abs() < 0.01, "{delay}: an input of {}", run.period_ms);
        let added = run.round_trip_ms - delay as f64 * frame_ms;
        assert!((added - run.slot_ms).abs() <= 5.0 + frame_ms, "{delay}: added {added:.3} ms");
    }
}

#[test]
fn every_way_a_loopback_can_begin_reads_the_same_round_trip_or_is_refused_with_the_reason() {
    let frame_ms = 1000.0 / f64::from(RATE);
    let lossless = looped(300, BLOCK);
    // Over a microphone's floor: the same round trip, the floor read from that noise
    let noisy = looped_into(300, BLOCK, Behaves { hiss: true, ..LOSSLESS }).unwrap();
    assert!(
        (noisy.round_trip_ms - lossless.round_trip_ms).abs() <= frame_ms,
        "{}",
        noisy.round_trip_ms
    );
    assert!((30.0..120.0).contains(&noisy.snr_db), "a floor of noise: {}", noisy.snr_db);
    assert!(lossless.snr_db > noisy.snr_db, "a lossless loop's floor is its exact zeros");
    // 1.2 s late, past the preroll: the programme waits for the slot, so no chirp falls in the
    // silence it plays while it opens
    let late = Behaves { starts_after: Some(RATE as usize * 6 / 5), ..LOSSLESS };
    let late = looped_into(300, BLOCK, late).unwrap();
    assert!(
        (late.round_trip_ms - lossless.round_trip_ms).abs() <= frame_ms,
        "{}",
        late.round_trip_ms
    );
    // Nothing ever delivered: refused for that, once the slot has waited 5 s in the graph
    let never = looped_into(300, BLOCK, Behaves { starts_after: None, ..LOSSLESS });
    let error = never.err().expect("an input that never delivers is refused");
    assert!(error.contains("the input delivered no sound"), "{error}");
    // A delivery a second after the first chirp that misses its pull by 16 frames, as the phase
    // of an input on its own clock can: the slot rides it out as a phase short, no stop, and it
    // measures, the delay's move under the spread check's bound
    let phase = Behaves { late_wake_after_a_chirp: Some((RATE as usize, 16)), ..LOSSLESS };
    let phase = looped_into(300, BLOCK, phase).unwrap();
    assert!(
        (phase.round_trip_ms - lossless.round_trip_ms).abs() <= frame_ms,
        "{}",
        phase.round_trip_ms
    );
    assert!(phase.spread_ms > 0.0, "the phase short moved the later chirps");
    // A whole wake late: still no stop, but the slot raises its delay a block, and the spread check
    // refuses in its own words
    let wake = Behaves { late_wake_after_a_chirp: Some((RATE as usize, BLOCK)), ..LOSSLESS };
    let error = looped_into(300, BLOCK, wake).err().expect("a block's move is past the bound");
    assert!(error.contains("the chirps disagree by 2.67 ms"), "{error}");
    // Stopped a second after the first chirp reached it: the later chirps are never heard, and the
    // run says the input stopped, not that the rig is wrong
    let stops = Behaves { stops_after_a_chirp: Some(RATE as usize), ..LOSSLESS };
    let error = looped_into(300, BLOCK, stops).err().expect("an input that stops is refused");
    assert!(error.contains("the input stopped delivering sound 1 times"), "{error}");
}

/// `bench loopback` as a person runs it, through a fake cable `delay` frames long under an output
/// granted `period` frames into an input that behaves as `input` says, writing its result to
/// `write`.
fn commanded(delay: usize, period: usize, input: Behaves, write: &Path) -> Result<bool, String> {
    let sink = Arc::new(Mutex::new(None));
    let (output_stats, input_stats) = (Arc::new(Stats::new()), Arc::new(Stats::new()));
    let stats = Arc::clone(&output_stats);
    let output = FakeLoop { delay, period, input, sink: Arc::clone(&sink), stats };
    let args = LoopbackArgs {
        loopback: Loopback { rig: Physical::Cable, level_db: -12.0 },
        output: None,
        input: None,
        input_channel: 0,
        period: period as u32,
        baseline: None,
        write: Some(write.to_owned()),
    };
    let opens = |period| -> Box<dyn Capture + Send> { Box::new(FakeInput { sink, period }) };
    command(&args, Box::new(output), output_stats, opens, input_stats)
}

#[test]
fn the_command_files_every_working_rig_and_refuses_each_broken_one_with_what_to_check() {
    let file =
        std::env::temp_dir().join(format!("aeon-bench-{}-loopback.json", std::process::id()));
    let second = RATE as usize;
    let works = [
        ("a lossless loop", BLOCK, LOSSLESS),
        ("a noisy loop", BLOCK, Behaves { hiss: true, ..LOSSLESS }),
        (
            "an input that opens late",
            BLOCK,
            Behaves { starts_after: Some(second * 6 / 5), ..LOSSLESS },
        ),
        (
            "a phase short",
            BLOCK,
            Behaves { late_wake_after_a_chirp: Some((second, 16)), ..LOSSLESS },
        ),
        ("a 480-frame grant", 480, LOSSLESS),
    ];
    for (rig, period, input) in works {
        let _ = std::fs::remove_file(&file);
        assert_eq!(commanded(300, period, input, &file), Ok(true), "{rig} works");
        let card = Scorecard::read(&file).unwrap();
        assert_eq!(card.bundles["loopback"].len(), 4, "{rig}: its four numbers are filed");
    }
    let broken = [
        (Behaves { starts_after: None, ..LOSSLESS }, "check --input and --input-channel"),
        (Behaves { mute: true, ..LOSSLESS }, "let this terminal use the microphone"),
        (Behaves { stops_after_a_chirp: Some(second), ..LOSSLESS }, "check the cable or the mic's"),
        (
            Behaves { late_wake_after_a_chirp: Some((second, BLOCK)), ..LOSSLESS },
            "rerun with nothing else playing or recording",
        ),
    ];
    for (input, says) in broken {
        let error = commanded(300, BLOCK, input, &file).expect_err("a broken rig is refused");
        assert!(error.contains(says) && error.ends_with("The run measured nothing"), "{error}");
    }
    let _ = std::fs::remove_file(&file);
}
