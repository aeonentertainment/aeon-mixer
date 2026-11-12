//! The mode a person runs on real hardware: chirps through a real slot and a real output, recorded
//! at a real input. Nothing else in this crate opens a device, and this only runs from the
//! `device` command with `--open-device` on its line, which `cli::parse` holds it to.

use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use devices::{CaptureRequest, Captured, Output, Request, Sink, Stats};
use engine::block::RATE;
use engine::driver::Driver;

use crate::capture_tap::CaptureTap;
use crate::cli::{Command, DeviceArgs, OutputMode};
use crate::ingest::Ingest;
use crate::offline;
use crate::opening::Opening;
use crate::programme::Programme;
use crate::reading::{self, Take};
use crate::report;
use crate::rig::{Delay, Rig};
use crate::tap::tap_render;

/// The path a device run plays through: raw PCM, so the signal reaches the output as it was sent.
/// The output's figure does not depend on the path, and the offline table has the other paths.
const THROUGH: Ingest = Ingest::L16 { frame: 240 };

/// Past the programme's end, for the last arrival and for the input to drain.
const TAIL: Duration = Duration::from_millis(200);

/// Frames an input may deliver past what the run should take, for a clock running fast.
const SPARE_FRAMES: usize = RATE as usize;

pub fn run(command: Command) -> Result<(), String> {
    match command {
        Command::List => list(),
        Command::Device(args) => measure(&args),
        Command::Offline { .. } => Err("the offline mode opens no device".into()),
    }
}

fn list() -> Result<(), String> {
    let describe = |output: &Output| {
        let default = if output.is_default { " (default)" } else { "" };
        format!("{}  {}{default}", output.id, output.name)
    };
    println!("outputs");
    for output in devices::outputs().map_err(|error| error.to_string())? {
        println!("  {}", describe(&output));
    }
    println!("inputs");
    for input in devices::inputs().map_err(|error| error.to_string())? {
        println!("  {}", describe(&input));
    }
    Ok(())
}

fn measure(args: &DeviceArgs) -> Result<(), String> {
    let programme = Programme::device(10f32.powf(args.level_db / 20.0));
    let signal = programme.signal();
    let rows = offline::measure_all(args.playout_frames)?;
    eprintln!(
        "measure-latency: playing chirps at {} dBFS for about {} s. Keep the room quiet.",
        args.level_db,
        programme.frames() * args.periods.len() * args.openings as usize / RATE as usize,
    );
    let mut openings = Vec::new();
    for period in &args.periods {
        for _ in 0..args.openings {
            openings.push(open_and_read(args, *period, &programme, &signal)?);
        }
    }
    print!("{}", report::device(args, &programme, &openings, &rows));
    Ok(())
}

/// Opens both streams, plays the programme, closes them, and reads what was recorded. A stream
/// that will not open stops the run; a recording with no number in it is reported as such.
fn open_and_read(
    args: &DeviceArgs,
    period: u32,
    programme: &Programme,
    signal: &[f32],
) -> Result<Opening, String> {
    let epoch = Instant::now();
    let rig = Rig::new(THROUGH, Delay::Pinned(args.playout_frames), signal, 0)
        .map_err(|error| error.to_string())?;
    let Rig { render, controls, .. } = rig;
    let frames = signal.len();
    let (render, mut rendered) = tap_render(render, frames, epoch);
    let (mut tap, mut captured) = CaptureTap::new(frames * 2 + SPARE_FRAMES, epoch);
    let sink: Sink =
        Box::new(move |delivery: Captured<'_>| tap.deliver(delivery.samples, delivery.at));

    let capture_stats = Arc::new(Stats::new());
    let output_stats = Arc::new(Stats::new());
    let request = CaptureRequest {
        device: args.input.clone(),
        period_frames: period,
        channel: args.input_channel,
    };
    let capture = devices::capture(request, Arc::clone(&capture_stats));
    let capturing = capture.start(sink).map_err(|error| format!("the input: {error}"))?;
    let input_info = capturing.info;

    let request = Request { device: args.output.clone(), period_frames: period };
    let output = driver(args.mode, request, Arc::clone(&output_stats))?;
    let running = output.start(render).map_err(|error| format!("the output: {error}"))?;
    let output_info = running.info;

    thread::sleep(Duration::from_secs_f64(frames as f64 / f64::from(RATE)) + TAIL);
    drop(running);
    drop(capturing);
    drop(controls);

    let (rendered, render_wakes) = rendered.take();
    let (captured, capture_wakes) = captured.take();
    let take = Take { rendered, render_wakes, captured, capture_wakes };
    let output_report = output_stats.report();
    let input_report = capture_stats.report();
    let reading = if input_report.underruns > 0 || input_report.failed || output_report.failed {
        Err(format!(
            "the input lost {} deliveries or a stream failed, so the frame counts no longer \
                 say the time. Rerun with nothing else using the input",
            input_report.underruns
        ))
    } else {
        reading::read(&take, programme)
    };
    Ok(Opening {
        asked: period,
        output_period: output_info.period_frames,
        output_latency_frames: output_info.output_latency_frames,
        input_period: input_info.period_frames,
        input_latency_frames: input_info.input_latency_frames,
        output_opened: output_stats.opened().unwrap_or("?").to_owned(),
        input_opened: capture_stats.opened().unwrap_or("?").to_owned(),
        underruns: output_report.underruns,
        realtime: output_report.realtime,
        reading,
    })
}

#[cfg(windows)]
fn driver(
    mode: OutputMode,
    request: Request,
    stats: Arc<Stats>,
) -> Result<Box<dyn Driver + Send>, String> {
    let mode = match mode {
        OutputMode::Auto => devices::WasapiMode::Auto,
        OutputMode::Shared => devices::WasapiMode::Shared,
        OutputMode::Exclusive => devices::WasapiMode::Exclusive,
    };
    Ok(Box::new(devices::WasapiDriver::new(request, mode, stats)))
}

#[cfg(not(windows))]
fn driver(
    mode: OutputMode,
    request: Request,
    stats: Arc<Stats>,
) -> Result<Box<dyn Driver + Send>, String> {
    match mode {
        OutputMode::Auto => Ok(devices::driver(request, stats)),
        _ => Err("--shared and --exclusive are for Windows".into()),
    }
}
