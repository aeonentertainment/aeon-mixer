//! Opens an input and an output, pulls the input through a `capture` slot's own ring and drift
//! correction on every output block, and prints once a minute the ring's fill before a pull, the
//! wait the slot reports, input period included, and whether the fill stayed within 8.4's bound: the
//! target and the headroom. It plays silence, so nothing feeds back. A person runs it on real
//! hardware, an hour with the mic on another clock than the output, to show the delay holds
//! (decision 0018).

use std::process::ExitCode;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicU32, AtomicU64};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use devices::{CaptureRequest, Request, Stats, inputs, outputs};
use engine::block::{BLOCK, RATE, Stereo};
use engine::ingest::{CaptureVoice, capture_voice, input_period_for};
use engine::slot::source::Source;

const USAGE: &str = "\
usage: measure-capture [--list] [--input ID] [--channel N] [--output ID] [--period FRAMES]
                       [--seconds N]

  --list        print the inputs and outputs and their ids, then exit
  --input ID    an input id from --list; the default input otherwise
  --channel N   the input's channel, from 0 (default 0)
  --output ID   an output id from --list; the default output otherwise
  --period N    the period to ask of the output, in frames (default 128); the input is asked
                what a capture slot asks under the output's granted period
  --seconds N   how long to run (default 3600), printing a line a minute";

/// What the output's thread records of each pull, swapped out once a minute. Only the pulls that
/// played the mic count toward the fill: priming and settling hold it over the target and the
/// headroom on purpose.
struct Minute {
    lowest: AtomicU32,
    highest: AtomicU32,
    sum: AtomicU64,
    played: AtomicU64,
    /// Every pull since the start, and the first that played, `u64::MAX` until one has.
    pulls: AtomicU64,
    first_played: AtomicU64,
    wait: AtomicU32,
    target: AtomicU32,
}

/// One minute's line: the fill before the pulls that played, lowest, mean and highest, in frames,
/// how many played, and the wait and target the slot holds now.
struct Taken {
    low: u32,
    mean: f64,
    high: u32,
    played: u64,
    wait: u32,
    target: u32,
}

impl Minute {
    fn new() -> Self {
        Self {
            lowest: AtomicU32::new(u32::MAX),
            highest: AtomicU32::new(0),
            sum: AtomicU64::new(0),
            played: AtomicU64::new(0),
            pulls: AtomicU64::new(0),
            first_played: AtomicU64::new(u64::MAX),
            wait: AtomicU32::new(0),
            target: AtomicU32::new(0),
        }
    }

    /// One pull of `voice`, just after it ran, on the output's thread.
    fn record(&self, voice: &CaptureVoice) {
        let pull = self.pulls.fetch_add(1, Relaxed);
        self.wait.store(voice.wait_frames() as u32, Relaxed);
        self.target.store(voice.target_frames() as u32, Relaxed);
        if !voice.played() {
            return;
        }
        let _ = self.first_played.compare_exchange(u64::MAX, pull, Relaxed, Relaxed);
        let fill = voice.ring_frames() as u32;
        self.lowest.fetch_min(fill, Relaxed);
        self.highest.fetch_max(fill, Relaxed);
        self.sum.fetch_add(u64::from(fill), Relaxed);
        self.played.fetch_add(1, Relaxed);
    }

    fn take(&self) -> Taken {
        let played = self.played.swap(0, Relaxed);
        let low = self.lowest.swap(u32::MAX, Relaxed);
        Taken {
            low: if played == 0 { 0 } else { low },
            mean: self.sum.swap(0, Relaxed) as f64 / played.max(1) as f64,
            high: self.highest.swap(0, Relaxed),
            played,
            wait: self.wait.load(Relaxed),
            target: self.target.load(Relaxed),
        }
    }

    /// How long the slot primed and settled before it first played, once it has.
    fn opened_after(&self) -> Option<Duration> {
        let pulls = self.first_played.load(Relaxed);
        (pulls != u64::MAX)
            .then(|| Duration::from_secs_f64((pulls * BLOCK as u64) as f64 / f64::from(RATE)))
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

    let minute = Arc::new(Minute::new());
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
            recorder.record(voice);
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
    let input = CaptureRequest {
        period_frames: input_period_for(running.info.period_frames),
        ..args.input.clone()
    };
    let capturing =
        match devices::capture(input, Arc::clone(&input_stats)).start(Box::new(move |captured| {
            producer.push(captured.samples);
        })) {
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
    let headroom = opened.headroom_frames() as u32;
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
    println!(
        "headroom   {headroom} frames, {:.2} ms: the input's period and the blocks a callback pulls \
         after its first",
        ms(f64::from(headroom)),
    );
    println!(
        "           a healthy fill before a pull stays at or under the target and the headroom"
    );
    println!(
        "minute  fill ms: low  mean  high  target  bound  within   wait   gaps  moves  underruns"
    );
    let started = Instant::now();
    let asked = Duration::from_secs(args.seconds);
    let (mut minutes, mut over, mut opened) = (0, 0, false);
    for elapsed in 1..=args.seconds.div_ceil(60) {
        // A minute a line, and the last line whatever is left of the time asked for
        thread::sleep(Duration::from_secs(60).min(asked.saturating_sub(started.elapsed())));
        let taken = minute.take();
        if let Some(after) = minute.opened_after().filter(|_| !opened) {
            opened = true;
            println!("played     from {} ms in, once it had primed and settled", ms_of(after));
        }
        let bound = taken.target + headroom;
        // A minute in which nothing played, all priming and settling, has no fill to judge
        let within = (taken.played > 0).then_some(taken.high <= bound);
        minutes += u32::from(within.is_some());
        over += u32::from(within == Some(false));
        let stats = counters.read();
        println!(
            "{elapsed:>6}  {:>13.2} {:>5.2} {:>5.2} {:>7.2} {:>6.2} {:>7} {:>6.2} {:>6} {:>6} {:>10}",
            ms(f64::from(taken.low)),
            ms(taken.mean),
            ms(f64::from(taken.high)),
            ms(f64::from(taken.target)),
            ms(f64::from(bound)),
            match within {
                Some(true) => "yes",
                Some(false) => "NO",
                None => "-",
            },
            ms(f64::from(taken.wait)),
            stats.concealed,
            stats.resyncs,
            input_stats.report().underruns,
        );
    }
    println!(
        "within     {} of {minutes} minutes kept the fill at or under its bound",
        minutes - over
    );
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
                parsed.output.period_frames = period;
            }
            "--seconds" => parsed.seconds = number(&arg, value()?)?.into(),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(parsed)
}

fn ms_of(duration: Duration) -> String {
    format!("{:.0}", duration.as_secs_f64() * 1000.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A healthy opening on a fake input of 128 frames, on the output's clock, under a block-a-wake
    /// output and a 480-frame callback. Priming and settling hold the fill over the target and the
    /// headroom on purpose; judged only over the pulls that played, the first minute stays within
    /// the bound, and the tool says the slot played from about a third of a second in.
    #[test]
    fn a_healthy_opening_reads_within_its_bound_in_the_first_minute() {
        for callback in [128u32, 480] {
            let (mut producer, voice) = capture_voice();
            let mut voice = voice.for_input(128, 0, callback).expect("a period it takes");
            let minute = Minute::new();
            let (mut mic, delivery) = ([0.0; BLOCK], [0.1; 128]);
            let (mut delivered, mut pulled) = (0u64, 0u64);
            while pulled < 60 * u64::from(RATE) {
                // A wake hears what the input delivered by when it starts, then pulls its blocks
                while delivered + 128 <= pulled {
                    producer.push(&delivery);
                    delivered += 128;
                }
                let wake = pulled + u64::from(callback);
                while pulled < wake {
                    voice.pull(&mut mic);
                    minute.record(&voice);
                    pulled += BLOCK as u64;
                }
            }
            let taken = minute.take();
            let bound = taken.target + voice.headroom_frames() as u32;
            assert!(taken.played > 0, "{callback}");
            assert!(taken.high <= bound, "{callback}: high {} over {bound}", taken.high);
            let after = minute.opened_after().expect("it played");
            let opened = (0.3..0.5).contains(&after.as_secs_f64());
            assert!(opened, "{callback}: played from {after:?}");
        }
    }
}
