//! Opens an output, plays for a minute, and prints what the OS gave: the period, the latency it
//! reports, the spacing of the device callbacks and the underruns. A person runs it on real
//! hardware, and its output is the evidence in docs/decisions/0003-device-backends.md.

use std::process::ExitCode;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use devices::{Output, Request, Stats, outputs};
#[cfg(windows)]
use devices::{WasapiDriver, WasapiMode};
use engine::block::{RATE, Stereo};
use engine::driver::{Driver, Render};

const USAGE: &str = "\
usage: measure-sink [--list] [--device ID] [--period FRAMES] [--seconds N] [--tone]
                    [--shared | --exclusive]   (Windows only)

  --list        print the outputs and their ids, then exit
  --device ID   an id from --list; the default output otherwise
  --period N    the hardware period to ask for, in frames (default 128)
  --seconds N   how long to play (default 60)
  --tone        play a quiet 440 Hz tone instead of silence
  --shared      WASAPI: IAudioClient3 shared, at the nearest period it allows
  --exclusive   WASAPI: exclusive (default: shared, exclusive only if shared cannot open)";

struct Args {
    list: bool,
    request: Request,
    seconds: u64,
    tone: bool,
    #[cfg(windows)]
    mode: WasapiMode,
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let outputs = match outputs() {
        Ok(outputs) => outputs,
        Err(error) => {
            eprintln!("measure-sink: could not list outputs: {error}");
            return ExitCode::FAILURE;
        }
    };
    if args.list {
        for output in &outputs {
            println!("{}", describe(output));
        }
        return ExitCode::SUCCESS;
    }

    let stats = Arc::new(Stats::new());
    let running = match driver(&args, Arc::clone(&stats)).start(render(args.tone)) {
        Ok(running) => running,
        Err(error) => {
            eprintln!("measure-sink: could not open the output: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("measure-sink: playing for {} s", args.seconds);
    let started = Instant::now();
    thread::sleep(Duration::from_secs(args.seconds));
    let ran = started.elapsed();
    let report = stats.report();
    let info = running.info;
    drop(running);

    let frames = |frames: u32| {
        format!(
            "{frames} frames, {} ms",
            ms(Duration::from_secs_f64(f64::from(frames) / f64::from(RATE)))
        )
    };
    let period = Duration::from_secs_f64(f64::from(info.period_frames) / f64::from(RATE));
    let late = period.mul_f64(1.5);
    let chosen = outputs.iter().find(|output| match &args.request.device {
        Some(id) => &output.id == id,
        None => output.is_default,
    });
    let interval =
        |fraction: f64| report.percentile(fraction).map(ms).unwrap_or_else(|| "-".into());
    let yes = |flag: bool| if flag { "yes" } else { "no" };

    println!(
        "measure-sink, {} {}, {} s",
        std::env::consts::OS,
        std::env::consts::ARCH,
        args.seconds
    );
    println!("output     {}", chosen.map(describe).unwrap_or_else(|| "?".into()));
    println!("opened     {}", stats.opened().unwrap_or("?"));
    println!("rate       {} Hz", info.sample_rate);
    println!("period     {} (asked {})", frames(info.period_frames), args.request.period_frames);
    println!("latency    {}, as the OS reports it", frames(info.output_latency_frames));
    println!("realtime   {}", yes(report.realtime));
    println!(
        "callbacks  {} in {:.1} s, {:.0} expected",
        report.callbacks,
        ran.as_secs_f64(),
        ran.as_secs_f64() / period.as_secs_f64(),
    );
    println!(
        "interval   min {}  p50 {}  p90 {}  p99 {}  p99.9 {}  max {} ms",
        report.shortest.map(ms).unwrap_or_else(|| "-".into()),
        interval(0.5),
        interval(0.9),
        interval(0.99),
        interval(0.999),
        report.longest.map(ms).unwrap_or_else(|| "-".into()),
    );
    println!("late       {} over 1.5 periods ({} ms)", report.at_least(late), ms(late));
    println!("underruns  {}", report.underruns);
    println!("failed     {}", yes(report.failed));
    if report.failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut parsed = Args {
        list: false,
        request: Request::default(),
        seconds: 60,
        tone: false,
        #[cfg(windows)]
        mode: WasapiMode::Auto,
    };
    let number = |flag: &str, text: String| {
        text.parse::<u32>().map_err(|_| format!("{flag} takes a whole number, not {text}"))
    };
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--list" => parsed.list = true,
            "--device" => parsed.request.device = Some(value()?),
            "--period" => parsed.request.period_frames = number(&arg, value()?)?,
            "--seconds" => parsed.seconds = number(&arg, value()?)?.into(),
            "--tone" => parsed.tone = true,
            #[cfg(windows)]
            "--shared" => parsed.mode = WasapiMode::Shared,
            #[cfg(windows)]
            "--exclusive" => parsed.mode = WasapiMode::Exclusive,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(parsed)
}

#[cfg(windows)]
fn driver(args: &Args, stats: Arc<Stats>) -> Box<dyn Driver> {
    Box::new(WasapiDriver::new(args.request.clone(), args.mode, stats))
}

#[cfg(not(windows))]
fn driver(args: &Args, stats: Arc<Stats>) -> Box<dyn Driver> {
    devices::driver(args.request.clone(), stats)
}

// Silence by default: the timing is the same and the room stays quiet.
fn render(tone: bool) -> Render {
    let gain = if tone { 0.1 } else { 0.0 };
    let step = std::f32::consts::TAU * 440.0 / RATE as f32;
    let mut phase = 0.0f32;
    Box::new(move |out: Stereo<'_>| {
        for (left, right) in out.left.iter_mut().zip(out.right.iter_mut()) {
            *left = gain * phase.sin();
            *right = *left;
            phase = (phase + step) % std::f32::consts::TAU;
        }
    })
}

fn describe(output: &Output) -> String {
    let default = if output.is_default { " (default)" } else { "" };
    format!("{}  {}{default}", output.id, output.name)
}

fn ms(duration: Duration) -> String {
    format!("{:.2}", duration.as_secs_f64() * 1000.0)
}
