//! `bench loopback`: a house mic's mouth to speaker on this machine, with no mouth (docs/design.md
//! 8.9). Decision 0006's chirps leave the output's left channel from the render at frames it
//! knows, and a `capture` slot (decision 0018) reads them back from the input the rig reaches.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::{Acquire, Release};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

use devices::{Capture, Stats};
use engine::audio_thread::AudioThread;
use engine::block::{RATE, Stereo};
use engine::command::{Command, Event};
use engine::driver::{Driver, Render};
use engine::graph::Graph;
use engine::ingest::{capture_voice, input_period_for};
use engine::slot::Slot;
use measure_latency::physical::Physical;
use measure_latency::programme::Programme;
use rtrb::RingBuffer;

use crate::cli::LoopbackArgs;
use crate::result;
use crate::round_trip;

/// Decision 0008's budget for the mic path, mouth to speaker, in ms.
const BUDGET_MS: (f64, f64) = (20.0, 30.0);

/// The chirps decision 0006's device mode plays, after a second of the slot playing the input,
/// whose second half the floor is read from.
pub fn programme(level_db: f32) -> Programme {
    Programme { preroll: RATE as usize, ..Programme::device(10f32.powf(level_db / 20.0)) }
}

/// Why the render woke the control thread.
enum Finished {
    /// The programme's last frame has played, with the stops the capture slot had judged its
    /// input to make by then, as it publishes them (design.md 8.4).
    Programme { stops: u32 },
    /// The slot had not played its input `MOST_WAITED` frames after it joined the graph.
    NeverPlayed,
}

/// How long the slot may take to play its input once it has joined the graph: 5 s, far past its
/// priming and settling, 0.34 s, and any input's first delivery.
const MOST_WAITED: usize = RATE as usize * 5;

/// What a loopback run plays, and how it is heard.
pub struct Loopback {
    pub rig: Physical,
    /// The chirps' peak in dBFS.
    pub level_db: f32,
}

/// What a loopback run measured, in ms and dB.
pub struct Measured {
    /// The median chirp's round trip, the air taken off.
    pub round_trip_ms: f64,
    /// How far the chirps' round trips disagree.
    pub spread_ms: f64,
    /// The chirp as heard against as played.
    pub gain_db: f64,
    /// The weakest chirp over what the slot heard before the first.
    pub snr_db: f64,
    /// The capture slot's wait as it reports it: the middle of a span one input period wide.
    pub slot_ms: f64,
    /// The input's period, how far apart two openings can read.
    pub period_ms: f64,
}

/// Plays the chirps on `output` and reads them back from the input `input` opens for the period
/// it is given, each stream recording into its stats, and prints what it played, on what, and
/// what it measured. A run that cannot stand for the path is refused with its reason.
pub fn loopback(
    args: &Loopback,
    output: Box<dyn Driver + Send>,
    output_stats: Arc<Stats>,
    input: impl FnOnce(u32) -> Box<dyn Capture + Send>,
    input_stats: Arc<Stats>,
) -> Result<Measured, String> {
    let programme = programme(args.level_db);
    let played = programme.signal();
    let frames = played.len();
    println!(
        "plays   {} chirps, 0.5 to 7.5 kHz, at {} dBFS from the output's left channel, {:.1} s",
        programme.repeats,
        args.level_db,
        frames as f64 / f64::from(RATE)
    );
    println!("rig     {}", args.rig.describe());

    let (mut audio, mut controls) = AudioThread::new();
    // Hard right at unit gain, so the master's right channel is the slot's output, and the output
    // is handed only the chirps on its left: the room never hears the slot, and the loop never
    // feeds itself
    controls.params.update(|params| params.slots[0].pan = 1.0);
    let (mut heard, mut heard_out) = RingBuffer::new(frames);
    let (finished, done) = mpsc::sync_channel(1);
    // Set once the slot is on its way into the graph: the wait for it to play counts from there,
    // not from the output's opening, which the control thread may reach late on a busy machine
    let joined = Arc::new(AtomicBool::new(false));
    let (mut producer, voice) = capture_voice();
    let slot_stats = voice.shared_stats();
    let render: Render = Box::new({
        let (signal, joined, slot) = (played.clone(), Arc::clone(&joined), Arc::clone(&slot_stats));
        // The programme's count, held at 0 until the capture slot says it has played its input:
        // until then it plays silence while it opens, primes and settles. So however late the
        // input opens, no chirp falls in that silence, and the floor is read from what the input
        // delivers, its noise or a lossless loop's exact zeros alike. The slot decides it, and the
        // loopback only reads it
        let (mut waited, mut at) = (0, None);
        move |out: Stereo<'_>| {
            if at.is_none() && slot.played() {
                at = Some(0);
            }
            let Stereo { left, right } = out;
            audio.render(Stereo { left: &mut *left, right: &mut *right });
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let Some(frame) = at.as_mut() else {
                    (*l, *r) = (0.0, 0.0);
                    continue;
                };
                if *frame < frames {
                    let _ = heard.push(*r);
                }
                (*l, *r) = (signal.get(*frame).copied().unwrap_or(0.0), 0.0);
                *frame += 1;
                // Once, after the programme's last frame: nothing is left on the clock to make late.
                // A phase short the slot rode out is no stop, and the spread check judges what it
                // moved
                if *frame == frames {
                    let _ = finished.try_send(Finished::Programme { stops: slot.stops() });
                }
            }
            if at.is_none() && joined.load(Acquire) {
                waited += left.len();
                if waited >= MOST_WAITED {
                    let _ = finished.try_send(Finished::NeverPlayed);
                }
            }
        }
    });

    // The output opens first, as a host's engine is open before a mixer makes a capture slot, so
    // the input and the slot are set up for the callback the output was granted, not the one asked
    // for. The programme waits for the slot, and its opening drops what came before it
    let running = output.start(render).map_err(|e| {
        format!(
            "the output would not start ({e}): check --output against the ids `measure-latency \
             list` prints, and that nothing else holds that output on its own"
        )
    })?;
    let info = running.info;
    let capturing = input(input_period_for(info.period_frames))
        .start(Box::new(move |captured| {
            producer.push(captured.samples);
        }))
        .map_err(|e| {
            format!(
                "the input would not start ({e}): check --input against the ids `measure-latency \
                 list` prints, and on macOS that this terminal may use the microphone"
            )
        })?;
    let input_info = capturing.info;
    let hardware = input_info.input_latency_frames.unwrap_or(0);
    let voice = voice.for_input(input_info.period_frames, hardware, info.period_frames).map_err(
        |reason| {
            format!(
                "the input cannot play through a house mic's slot ({reason}): try another --period"
            )
        },
    )?;
    let graph = Box::new(Graph::new(vec![Slot::new(Box::new(voice))]));
    if controls.commands.push(Command::Swap { id: 1, graph }).is_err() {
        return Err(
            "a new command ring had no room for the slot, which is a bug in the bench".into()
        );
    }
    joined.store(true, Release);

    let latency_ms = f64::from(info.output_latency_frames) * 1000.0 / f64::from(info.sample_rate);
    let opened = output_stats.opened().unwrap_or("an output");
    println!(
        "out     {opened}: {} Hz, a {}-frame period, {latency_ms:.2} ms of output latency reported",
        info.sample_rate, info.period_frames
    );
    let input_latency =
        input_info.input_latency_frames.map_or("no input latency reported".into(), |frames| {
            let ms = f64::from(frames) * 1000.0 / f64::from(input_info.sample_rate);
            format!("{ms:.2} ms of input latency reported")
        });
    println!(
        "in      {}: a {}-frame period, {input_latency}\n",
        input_stats.opened().unwrap_or("an input"),
        input_info.period_frames
    );

    let (mut second, mut faulted) = (0, false);
    let stopped = loop {
        let waited = done.recv_timeout(Duration::from_secs(1));
        while controls.retired.pop().is_some() {}
        while let Some(event) = controls.events.pop() {
            faulted |= event == Event::Fault;
        }
        let (out, into) = (output_stats.report(), input_stats.report());
        // Whatever went wrong before the programme's end refuses the run, whichever wake sees it
        match waited {
            _ if faulted => {
                break Some(
                    "the engine faulted, which is a bug in the mixer: rerun, and report it if it \
                     happens again"
                        .to_owned(),
                );
            }
            _ if out.lost || into.lost => {
                break Some(
                    "a device went away: plug it back in, or pick another with --output or --input"
                        .to_owned(),
                );
            }
            _ if out.failed || into.failed => {
                break Some(
                    "a stream failed: rerun, and pick another device if it fails again".to_owned(),
                );
            }
            Ok(Finished::NeverPlayed) => {
                break Some(format!(
                    "the input delivered no sound in the {} s after the house mic's slot opened: \
                     check --input and --input-channel, and on macOS that this terminal may use \
                     the microphone",
                    MOST_WAITED / RATE as usize
                ));
            }
            Ok(Finished::Programme { stops: 0 }) => break None,
            Ok(Finished::Programme { stops }) => {
                break Some(format!(
                    "the input stopped delivering sound {stops} times while the chirps played: \
                     check the cable or the mic's connection, and that nothing else is using the \
                     input"
                ));
            }
            Err(RecvTimeoutError::Disconnected) => {
                break Some(
                    "the output stopped: rerun, and pick another --output if it stops again"
                        .to_owned(),
                );
            }
            Err(RecvTimeoutError::Timeout) => second += 1,
        }
    };
    drop(running);
    drop(capturing);
    if let Some(why) = stopped {
        return Err(format!("refused {second} s into the run: {why}. The run measured nothing"));
    }
    let lost = input_stats.report().underruns;
    if lost > 0 {
        return Err(format!(
            "refused: the input lost {lost} deliveries, so its frames no longer count the time. \
             Rerun with nothing else using the input. The run measured nothing"
        ));
    }
    let heard: Vec<f32> = std::iter::from_fn(|| heard_out.pop().ok()).collect();
    let reading = round_trip::read(&played, &heard, &programme, args.rig.air_ms())
        .map_err(|why| format!("refused: {why}. The run measured nothing"))?;

    let slot_ms = f64::from(slot_stats.read().playout_delay_ms);
    let (round_trip, spread) = (reading.median_ms(), reading.spread_ms());
    let period_ms =
        f64::from(input_info.period_frames) * 1000.0 / f64::from(input_info.sample_rate);
    println!("measured");
    println!("  round trip   {round_trip:>8.2} ms  median of {} chirps", reading.gaps_ms.len());
    println!(
        "               another opening can read up to one input period away, {period_ms:.2} ms \
         here: where the input's\n               deliveries fall against the output's wakes picks \
         the point, so compare two runs with a\n               tolerance of one period, 2.67 ms at \
         the default period and 1.33 ms at --period 64"
    );
    println!("  spread       {spread:>8.2} ms  how far the chirps disagree");
    println!(
        "  slot waits   {slot_ms:>8.2} ms  as the capture slot reports it: the middle of that \
         {period_ms:.2} ms span"
    );
    println!("  gain         {:>8.2} dB  the chirp as heard against as played", reading.gain_db());
    println!("  over noise   {:>8.2} dB  the weakest chirp", reading.snr_db);
    let path = match args.rig {
        Physical::Cable => "to the output's jack; a powered speaker adds its own",
        Physical::Mic { .. } => "to what the room hears",
    };
    let (low, high) = BUDGET_MS;
    let against = match round_trip {
        ms if ms < low => format!("under decision 0008's {low} to {high} ms budget"),
        ms if ms <= high => format!("inside decision 0008's {low} to {high} ms budget"),
        ms => format!("over decision 0008's {low} to {high} ms budget by {:.2} ms", ms - high),
    };
    println!(
        "verdict  a house mic on this machine reaches the room {round_trip:.2} ms after it hears \
         the singer, {path}: {against}"
    );
    Ok(Measured {
        round_trip_ms: round_trip,
        spread_ms: spread,
        gain_db: reading.gain_db(),
        snr_db: reading.snr_db,
        slot_ms,
        period_ms,
    })
}

/// `bench loopback`: reads the baseline `args` names, runs the loopback on `output` and the input
/// `input` opens, and files its numbers as a scorecard under `loopback`. Ok(false) when a number
/// moved past what the baseline allows; a refusal is its reason.
pub fn command(
    args: &LoopbackArgs,
    output: Box<dyn Driver + Send>,
    output_stats: Arc<Stats>,
    input: impl FnOnce(u32) -> Box<dyn Capture + Send>,
    input_stats: Arc<Stats>,
) -> Result<bool, String> {
    let baseline = args.baseline.as_deref().map(scorecard::card::Scorecard::read).transpose()?;
    let measured = loopback(&args.loopback, output, output_stats, input, input_stats)?;
    let numbers = [
        ("loopback.roundTripMs", measured.round_trip_ms),
        ("loopback.spreadMs", measured.spread_ms),
        ("loopback.gainDb", measured.gain_db),
        ("loopback.snrDb", measured.snr_db),
    ];
    result::settle("loopback", numbers, baseline.as_ref(), args.write.as_deref())
}
