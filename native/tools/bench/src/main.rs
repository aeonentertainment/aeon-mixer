//! `bench`: the mixer alone on a real clock (lib.rs).

use std::process::ExitCode;
use std::sync::Arc;

use bench::cli::{self, Clock, Command, LoopbackArgs, USAGE};
use devices::{Stats, TimerDriver};
use engine::driver::Driver;

/// The clock `clock` names, recording into `stats`. A real output exists only in a build with
/// `hardware`, and only when the command line asked for it by name.
fn driver(clock: &Clock, stats: Arc<Stats>) -> Result<Box<dyn Driver + Send>, String> {
    match clock {
        Clock::Timer => Ok(Box::new(TimerDriver::new(stats))),
        #[cfg(feature = "hardware")]
        Clock::Device { output, period } => {
            let request = devices::Request { device: output.clone(), period_frames: *period };
            Ok(devices::driver(request, stats))
        }
        #[cfg(not(feature = "hardware"))]
        Clock::Device { .. } => Err("--open-device needs a build with --features hardware".into()),
    }
}

/// `bench loopback` on the output and input `args` name, which only a build with `hardware` has.
#[cfg(feature = "hardware")]
fn loopback(args: &LoopbackArgs) -> Result<bool, String> {
    let (output_stats, input_stats) = (Arc::new(Stats::new()), Arc::new(Stats::new()));
    let clock = Clock::Device { output: args.output.clone(), period: args.period };
    let output = driver(&clock, Arc::clone(&output_stats))?;
    // The loopback opens the input once it knows the callback the output was granted
    let input = |period_frames| {
        let request = devices::CaptureRequest {
            device: args.input.clone(),
            period_frames,
            channel: args.input_channel,
        };
        devices::capture(request, Arc::clone(&input_stats))
    };
    bench::loopback::command(args, output, output_stats, input, Arc::clone(&input_stats))
}

#[cfg(not(feature = "hardware"))]
fn loopback(_: &LoopbackArgs) -> Result<bool, String> {
    Err("loopback needs a build with --features hardware".into())
}

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let run = match &command {
        Command::Play(args) => {
            let stats = Arc::new(Stats::new());
            driver(&args.clock, Arc::clone(&stats))
                .and_then(|driver| bench::play::play(args, driver, stats))
        }
        Command::Loopback(args) => loopback(args),
        Command::Help => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
    };
    match run {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("bench: {error}");
            ExitCode::FAILURE
        }
    }
}
