//! Runs the timer clock for a minute and prints its wake timing in measure-sink's form, with the
//! blocks it rendered against the blocks wall time reached. It opens no device and needs no
//! audio stack, so it runs anywhere the engine does: a container, CI, a box with no sound card.
//! Its output is the evidence in docs/decisions/0011-timer-clock.md.

use std::hint::black_box;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::thread;
use std::time::{Duration, Instant};

use devices::{Stats, Summary, TimerDriver};
use engine::block::{BLOCK, RATE, Stereo};
use engine::driver::{Driver, Render};

const USAGE: &str = "\
usage: measure-clock [--seconds N] [--work-us N]

  --seconds N   how long to run (default 60)
  --work-us N   busy work per block, in microseconds, to stand in for a mix (default 0)";

fn main() -> ExitCode {
    let (seconds, work) = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let stats = Arc::new(Stats::new());
    let rendered = Arc::new(AtomicU64::new(0));
    let render = busy(work, Arc::clone(&rendered));
    let running = match Box::new(TimerDriver::new(Arc::clone(&stats))).start(render) {
        Ok(running) => running,
        Err(error) => {
            eprintln!("measure-clock: could not start the timer: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("measure-clock: running for {seconds} s");
    let started = Instant::now();
    thread::sleep(Duration::from_secs(seconds));
    let ran = started.elapsed();
    let report = stats.report();
    let info = running.info;
    drop(running);
    let blocks = rendered.load(Relaxed);

    println!("measure-clock, {} {}, {seconds} s", std::env::consts::OS, std::env::consts::ARCH);
    println!("opened     {}", stats.opened().unwrap_or("?"));
    println!("work       {} us a block", work.as_micros());
    println!(
        "blocks     {blocks} rendered, {:.0} expected",
        ran.as_secs_f64() * f64::from(RATE) / BLOCK as f64
    );
    println!("{}", Summary { report: &report, period_frames: info.period_frames, ran });
    if report.failed { ExitCode::FAILURE } else { ExitCode::SUCCESS }
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<(u64, Duration), String> {
    let (mut seconds, mut work) = (60, Duration::ZERO);
    while let Some(arg) = args.next() {
        let value = args.next().ok_or_else(|| format!("{arg} needs a value"))?;
        let number =
            value.parse::<u64>().map_err(|_| format!("{arg} takes a whole number, not {value}"))?;
        match arg.as_str() {
            "--seconds" => seconds = number,
            "--work-us" => work = Duration::from_micros(number),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok((seconds, work))
}

// Spins for `work` a block, so the wake timing is read with a render's cost behind it
fn busy(work: Duration, rendered: Arc<AtomicU64>) -> Render {
    Box::new(move |mut out: Stereo<'_>| {
        let started = Instant::now();
        while started.elapsed() < work {
            black_box(&out);
        }
        out.silence();
        rendered.fetch_add(1, Relaxed);
    })
}
