//! Opens an input and an output, pulls the input through a `capture` slot's own ring and drift
//! correction on every output block, and prints once a minute the ring's fill before a pull and the
//! wait the slot reports, input period included. It plays silence, so nothing feeds back. A person
//! runs it on real hardware, an hour with the mic on another clock than the output, to show the
//! delay holds (decision 0018).

use std::process::ExitCode;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use devices::{CaptureRequest, Request, Stats, inputs, outputs};
use engine::block::{BLOCK, RATE, Stereo};
use engine::ingest::{CaptureVoice, capture_voice};
use engine::slot::source::Source;

const USAGE: &str = "\
usage: measure-capture [--list] [--input ID] [--channel N] [--output ID] [--period FRAMES]
                       [--seconds N]

  --list        print the inputs and outputs and their ids, then exit
  --input ID    an input id from --list; the default input otherwise
  --channel N   the input's channel, from 0 (default 0)
  --output ID   an output id from --list; the default output otherwise
  --period N    the period to ask of both, in frames (default 128)
  --seconds N   how long to run (default 3600), printing a line a minute";

/// What the output's thread records of each block, swapped out once a minute.
struct Minute {
    lowest: AtomicU32,
    highest: AtomicU32,
    sum: AtomicU64,
    blocks: AtomicU64,
    wait: AtomicU32,
}

impl Minute {
    fn record(&self, fill: u32, wait: u32) {
        self.lowest.fetch_min(fill, Relaxed);
        self.highest.fetch_max(fill, Relaxed);
        self.sum.fetch_add(u64::from(fill), Relaxed);
        self.blocks.fetch_add(1, Relaxed);
        self.wait.store(wait, Relaxed);
    }

    /// Lowest, mean and highest fill before a pull since the last take, in frames, and the wait
    /// the slot reports now.
    fn take(&self) -> (u32, f64, u32, u32) {
        let blocks = self.blocks.swap(0, Relaxed).max(1);
        let mean = self.sum.swap(0, Relaxed) as f64 / blocks as f64;
        (
            self.lowest.swap(u32::MAX, Relaxed),
            mean,
            self.highest.swap(0, Relaxed),
            self.wait.load(Relaxed),
        )
    }
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if args.list {
        return list();
    }

    let minute = Arc::new(Minute {
        lowest: AtomicU32::new(u32::MAX),
        highest: AtomicU32::new(0),
        sum: AtomicU64::new(0),
        blocks: AtomicU64::new(0),
        wait: AtomicU32::new(0),
    });
    // The voice needs the output's period, which only a running output reports, so the output
    // opens first and the voice reaches its render through a bounded channel of one, whose
    // `try_recv` neither locks nor allocates
    let (hand_over, handed) = mpsc::sync_channel::<CaptureVoice>(1);
    let recorder = Arc::clone(&minute);
    let mut voice = None;
    let mut mic = [0.0; BLOCK];
    let render = Box::new(move |out: Stereo<'_>| {
        if voice.is_none() {
            voice = handed.try_recv().ok();
        }
        if let Some(voice) = voice.as_mut() {
            voice.pull(&mut mic);
            recorder.record(voice.ring_frames() as u32, voice.wait_frames() as u32);
        }
        out.left.fill(0.0);
        out.right.fill(0.0);
    });
    let output_stats = Arc::new(Stats::new());
    let running =
        match devices::driver(args.output.clone(), Arc::clone(&output_stats)).start(render) {
            Ok(running) => running,
            Err(error) => {
                eprintln!("measure-capture: could not open the output: {error}");
                return ExitCode::FAILURE;
            }
        };

    let (mut producer, opened) = capture_voice();
    let input_stats = Arc::new(Stats::new());
    let capturing = match devices::capture(args.input.clone(), Arc::clone(&input_stats)).start(
        Box::new(move |captured| {
            producer.push(captured.samples);
        }),
    ) {
        Ok(capturing) => capturing,
        Err(error) => {
            eprintln!("measure-capture: could not open the input: {error}");
            return ExitCode::FAILURE;
        }
    };
    let info = capturing.info;
    let hardware = info.input_latency_frames.unwrap_or(0);
    let opened = match opened.for_input(info.period_frames, hardware, running.info.period_frames) {
        Ok(opened) => opened,
        Err(reason) => {
            eprintln!("measure-capture: {reason}");
            return ExitCode::FAILURE;
        }
    };
    let counters = opened.shared_stats();
    if hand_over.try_send(opened).is_err() {
        eprintln!("measure-capture: the output never took the input");
        return ExitCode::FAILURE;
    }

    let ms = |frames: f64| frames * 1000.0 / f64::from(RATE);
    println!(
        "measure-capture, {} {}, {} s",
        std::env::consts::OS,
        std::env::consts::ARCH,
        args.seconds
    );
    println!("input      {}", input_stats.opened().unwrap_or("?"));
    println!(
        "           period {} frames, {:.2} ms; the OS reports {} more",
        info.period_frames,
        ms(f64::from(info.period_frames)),
        info.input_latency_frames
            .map_or("nothing".into(), |f| format!("{:.2} ms", ms(f64::from(f)))),
    );
    println!("output     {}", output_stats.opened().unwrap_or("?"));
    println!(
        "           period {} frames, the OS reports {:.2} ms",
        running.info.period_frames,
        ms(f64::from(running.info.output_latency_frames)),
    );
    println!("minute  fill ms: low  mean  high    wait   gaps  moves  input underruns");
    let started = Instant::now();
    let asked = Duration::from_secs(args.seconds);
    for elapsed in 1..=args.seconds.div_ceil(60) {
        // A minute a line, and the last line whatever is left of the time asked for
        thread::sleep(Duration::from_secs(60).min(asked.saturating_sub(started.elapsed())));
        let (low, mean, high, wait) = minute.take();
        let stats = counters.read();
        println!(
            "{elapsed:>6}  {:>13.2} {:>5.2} {:>5.2} {:>7.2} {:>6} {:>6} {:>16}",
            ms(f64::from(low)),
            ms(mean),
            ms(f64::from(high)),
            ms(f64::from(wait)),
            stats.concealed,
            stats.resyncs,
            input_stats.report().underruns,
        );
    }
    drop(running);
    drop(capturing);
    let ran = started.elapsed().as_secs_f64();
    let failed = input_stats.report().failed || output_stats.report().failed;
    println!("ran        {ran:.0} s; failed {}", if failed { "yes" } else { "no" });
    if failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

struct Args {
    list: bool,
    input: CaptureRequest,
    output: Request,
    seconds: u64,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        list: false,
        input: CaptureRequest::default(),
        output: Request::default(),
        seconds: 3600,
    };
    let number = |flag: &str, text: String| {
        text.parse::<u32>().map_err(|_| format!("{flag} takes a whole number, not {text}"))
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--list" => parsed.list = true,
            "--input" => parsed.input.device = Some(value()?),
            "--channel" => {
                parsed.input.channel = u16::try_from(number(&arg, value()?)?)
                    .map_err(|_| "--channel is at most 65535".to_string())?;
            }
            "--output" => parsed.output.device = Some(value()?),
            "--period" => {
                let period = number(&arg, value()?)?;
                parsed.input.period_frames = period;
                parsed.output.period_frames = period;
            }
            "--seconds" => parsed.seconds = number(&arg, value()?)?.into(),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(parsed)
}

fn list() -> ExitCode {
    for (heading, listed) in [("inputs", inputs()), ("outputs", outputs())] {
        match listed {
            Ok(devices) => {
                println!("{heading}");
                for device in devices {
                    let default = if device.is_default { " (default)" } else { "" };
                    println!("  {}  {}{default}", device.id, device.name);
                }
            }
            Err(error) => {
                eprintln!("measure-capture: could not list {heading}: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
