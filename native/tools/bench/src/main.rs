//! `bench`: plays a session bundle through the mixer on a real clock (lib.rs).

use std::process::ExitCode;
use std::sync::Arc;

use bench::cli::{self, Clock, USAGE};
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

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let stats = Arc::new(Stats::new());
    let run = driver(&args.clock, Arc::clone(&stats))
        .and_then(|driver| bench::play::play(&args, driver, stats));
    match run {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("bench: {error}");
            ExitCode::FAILURE
        }
    }
}
