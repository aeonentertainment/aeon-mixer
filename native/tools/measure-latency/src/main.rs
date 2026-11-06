//! The mic path's latency. See `USAGE`, and docs/decisions/0006-measuring-latency.md for what the
//! figures can and cannot say. A person runs the `device` mode on real hardware; everything else,
//! tests included, opens nothing.

use std::process::ExitCode;

use measure_latency::cli::{self, Command, USAGE};
use measure_latency::{offline, report};

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("measure-latency: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Offline { playout_frames } => {
            offline::measure_all(playout_frames).map(|rows| print!("{}", report::offline(&rows)))
        }
        Command::Device(_) | Command::List => hardware(command),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("measure-latency: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(feature = "hardware")]
fn hardware(command: Command) -> Result<(), String> {
    measure_latency::device::run(command)
}

#[cfg(not(feature = "hardware"))]
fn hardware(_command: Command) -> Result<(), String> {
    Err("this build opens no device. Build with --features hardware".into())
}
