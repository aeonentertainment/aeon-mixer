use std::sync::Arc;
use std::thread;
use std::time::Duration;

use engine::block::RATE;
use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};
use objc2_core_audio::{
    AudioObjectID, kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyBufferFrameSizeRange,
    kAudioDevicePropertyLatency, kAudioDevicePropertyNominalSampleRate,
    kAudioDevicePropertySafetyOffset, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeOutput, kAudioStreamPropertyLatency,
    kAudioStreamPropertyVirtualFormat,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, AudioValueRange, kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsNonInterleaved, kAudioFormatLinearPCM,
};

use super::callback::Callback;
use super::io::Io;
use super::property::{get, set, string};
use crate::feed::Feed;
use crate::request::Request;
use crate::stats::Stats;

/// How long a sample rate change may take to land; the HAL applies it asynchronously.
const RATE_CHANGE: Duration = Duration::from_secs(1);

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
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let Self { request, stats } = *self;
        let device = match &request.device {
            Some(uid) => super::find(uid)?,
            None => super::default_device()?,
        };
        set_rate(device)?;
        let period = set_period(device, request.period_frames)?;

        let stream = *super::output_streams(device)?.first().ok_or(DriverError::NoDevice)?;
        // SAFETY: a stream's virtual format is an AudioStreamBasicDescription.
        let format: AudioStreamBasicDescription = unsafe {
            get(stream, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal)?
        };
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

        let feed = Feed::new(render, Arc::clone(&stats));
        let io = Io::start(device, Callback::new(feed, stats))?;
        Ok(Running::new(info, move || drop(io)))
    }
}

// The engine never resamples, so the device itself moves to 48 kHz if it is elsewhere.
fn set_rate(device: AudioObjectID) -> Result<(), DriverError> {
    let read = || -> Result<f64, DriverError> {
        // SAFETY: the nominal sample rate is a Float64.
        unsafe {
            get(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal)
        }
    };
    let rate = f64::from(RATE);
    if read()? == rate {
        return Ok(());
    }
    // SAFETY: the nominal sample rate is a Float64.
    unsafe {
        set(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal, &rate)?
    };
    let step = Duration::from_millis(10);
    for _ in 0..RATE_CHANGE.as_millis() / step.as_millis() {
        if read()? == rate {
            return Ok(());
        }
        thread::sleep(step);
    }
    Err(DriverError::Backend(format!("the device did not move to {RATE} Hz")))
}

// The IO buffer is the period: the frames each IOProc call renders.
fn set_period(device: AudioObjectID, asked: u32) -> Result<u32, DriverError> {
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
