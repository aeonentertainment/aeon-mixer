use std::sync::Arc;

use engine::block::RATE;
use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};
use objc2_core_audio::{
    AudioObjectID, kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyBufferFrameSizeRange,
    kAudioDevicePropertyLatency, kAudioDevicePropertySafetyOffset, kAudioObjectPropertyName,
    kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput, kAudioStreamPropertyLatency,
    kAudioStreamPropertyVirtualFormat,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, AudioValueRange, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved, kAudioFormatLinearPCM,
};

use super::callback::Callback;
use super::hal_rate::RATES;
use super::io::Io;
use super::property::{get, set, string};
use super::steps::Steps;
use crate::feed::Feed;
use crate::request::Request;
use crate::stats::Stats;

/// The CoreAudio HAL with no AudioUnit between: an IOProc on the device, with the device's IO
/// buffer set to the asked period. Other apps keep playing; the device runs at the smallest
/// buffer any client asks for. The HAL's IO thread is already a realtime thread.
pub struct CoreAudioDriver {
    request: Request,
    stats: Arc<Stats>,
}

impl CoreAudioDriver {
    pub fn new(request: Request, stats: Arc<Stats>) -> Self {
        Self { request, stats }
    }
}

impl Driver for CoreAudioDriver {
    /// Sets the device up on the calling thread, which blocks while the HAL does, then starts IO.
    /// A failure says how far into the start it came and when each step before it finished.
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let mut steps = Steps::new();
        let Self { request, stats } = *self;
        open(&request, stats, render, &mut steps).map_err(|error| steps.failed(error))
    }
}

fn open(
    request: &Request,
    stats: Arc<Stats>,
    render: Render,
    steps: &mut Steps,
) -> Result<Running, DriverError> {
    let device = match &request.device {
        Some(uid) => super::find(uid)?,
        None => super::default_device()?,
    };
    // The engine never resamples, so the device itself moves to 48 kHz if it is elsewhere,
    // until the stream stops or fails to start here
    let rate = RATES.hold(device)?;
    steps.done("48 kHz held");
    let period = set_period(device, request.period_frames)?;
    steps.done("IO buffer set");

    let stream = *super::output_streams(device)?.first().ok_or(DriverError::NoDevice)?;
    // SAFETY: a stream's virtual format is an AudioStreamBasicDescription.
    let format: AudioStreamBasicDescription =
        unsafe { get(stream, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal)? };
    if format.mFormatID != kAudioFormatLinearPCM
        || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
        || format.mBitsPerChannel != 32
    {
        return Err(DriverError::Backend("the device's output stream is not f32 PCM".into()));
    }
    let layout = match format.mFormatFlags & kAudioFormatFlagIsNonInterleaved {
        0 => "interleaved",
        _ => "a buffer a channel",
    };

    let info = StreamInfo {
        sample_rate: RATE,
        period_frames: period,
        output_latency_frames: period + output_latency(device, stream)?,
    };
    stats.set_opened(format!(
        "CoreAudio {}, f32 x{} {layout}, IO buffer {period}",
        string(device, kAudioObjectPropertyName).unwrap_or_default(),
        format.mChannelsPerFrame,
    ));
    stats.set_realtime(true);

    steps.done("format read");

    let feed = Feed::new(render, Arc::clone(&stats));
    let io = Io::start(device, Callback::new(feed, stats))?;
    Ok(Running::new(info, move || {
        drop(io);
        drop(rate);
    }))
}

// The IO buffer is the period: the frames each IOProc call renders.
pub(super) fn set_period(device: AudioObjectID, asked: u32) -> Result<u32, DriverError> {
    // SAFETY: the buffer size range is an AudioValueRange and the buffer size a UInt32.
    unsafe {
        let range: AudioValueRange =
            get(device, kAudioDevicePropertyBufferFrameSizeRange, kAudioObjectPropertyScopeGlobal)?;
        let frames = f64::from(asked).clamp(range.mMinimum, range.mMaximum) as u32;
        set(device, kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal, &frames)?;
        get(device, kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal)
    }
}

// What the HAL says lies between the IO buffer and the speaker: the device's latency, its safety
// offset, and the stream's own latency, all in frames.
fn output_latency(device: AudioObjectID, stream: AudioObjectID) -> Result<u32, DriverError> {
    // SAFETY: each of these is a UInt32 frame count.
    unsafe {
        let device_latency: u32 =
            get(device, kAudioDevicePropertyLatency, kAudioObjectPropertyScopeOutput)?;
        let safety: u32 =
            get(device, kAudioDevicePropertySafetyOffset, kAudioObjectPropertyScopeOutput)?;
        let stream_latency: u32 =
            get(stream, kAudioStreamPropertyLatency, kAudioObjectPropertyScopeGlobal)?;
        Ok(device_latency + safety + stream_latency)
    }
}
