//! A `capture` slot's input (docs/design.md 8.4): this OS's capture stream on one device, whose
//! sink pushes each delivery into the slot's ring on the device's own thread.

use std::sync::Arc;

use devices::{CaptureRequest, Capturing, Stats};
use engine::ingest::{CaptureVoice, capture_voice};
use napi::bindgen_prelude::*;

/// Starts `device`, or the default input, at `period_frames`, the output's period, and returns the
/// running stream with the voice that reads it under that output. Dropping the `Capturing` stops
/// the stream. Blocks while the OS sets the device up, as opening the output does.
pub fn open(
    device: Option<String>,
    channel: u16,
    period_frames: u32,
) -> Result<(Capturing, CaptureVoice)> {
    let (mut producer, voice) = capture_voice();
    let named = device.clone().unwrap_or_else(|| "the default input".into());
    let request = CaptureRequest { device, period_frames, channel };
    let capturing = devices::capture(request, Arc::new(Stats::new()))
        .start(Box::new(move |captured| {
            producer.push(captured.samples);
        }))
        .map_err(|error| Error::from_reason(format!("could not open {named}: {error}")))?;
    let info = capturing.info;
    let hardware = info.input_latency_frames.unwrap_or(0);
    let voice = voice
        .for_input(info.period_frames, hardware, period_frames)
        .map_err(|reason| Error::from_reason(format!("cannot capture {named}: {reason}")))?;
    Ok((capturing, voice))
}
