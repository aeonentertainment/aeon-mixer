use std::sync::Arc;

use engine::block::RATE;
use engine::driver::DriverError;
use objc2_core_audio::{
    AudioObjectID, kAudioDevicePropertyLatency, kAudioDevicePropertySafetyOffset,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeInput,
    kAudioStreamPropertyLatency, kAudioStreamPropertyVirtualFormat,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, kAudioFormatFlagIsFloat, kAudioFormatLinearPCM,
};

use super::capture_callback::CaptureCallback;
use super::capture_io::CaptureIo;
use super::driver::set_period;
use super::hal_rate::RATES;
use super::property::{get, string};
use crate::capture::{Capture, CaptureInfo, Capturing, Sink};
use crate::capture_feed::CaptureFeed;
use crate::capture_request::CaptureRequest;
use crate::stats::Stats;
use crate::stream_holding::stream_holding;

/// An IOProc on the input device, with no AudioUnit and none of the voice processing an app's
/// session would get. macOS asks the person for the microphone on first use and, refused, delivers
/// silence.
pub struct CoreAudioCapture {
    request: CaptureRequest,
    stats: Arc<Stats>,
}

impl CoreAudioCapture {
    pub fn new(request: CaptureRequest, stats: Arc<Stats>) -> Self {
        Self { request, stats }
    }
}

impl Capture for CoreAudioCapture {
    /// Sets the device up on the calling thread, which blocks while the HAL does, then starts IO.
    fn start(self: Box<Self>, sink: Sink) -> Result<Capturing, DriverError> {
        let Self { request, stats } = *self;
        let device = match &request.device {
            Some(uid) => super::find_input(uid)?,
            None => super::default_input()?,
        };
        let rate = RATES.hold(device)?;
        let period = set_period(device, request.period_frames)?;

        let streams = super::input_streams(device)?;
        let mut counts = Vec::with_capacity(streams.len());
        for stream in &streams {
            // SAFETY: a stream's virtual format is an AudioStreamBasicDescription.
            let format: AudioStreamBasicDescription = unsafe {
                get(*stream, kAudioStreamPropertyVirtualFormat, kAudioObjectPropertyScopeGlobal)?
            };
            if format.mFormatID != kAudioFormatLinearPCM
                || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
                || format.mBitsPerChannel != 32
            {
                return Err(DriverError::Backend("the device's input is not f32 PCM".into()));
            }
            counts.push(format.mChannelsPerFrame);
        }
        let channels: u32 = counts.iter().sum();
        let stream = stream_holding(&counts, u32::from(request.channel)).ok_or_else(|| {
            DriverError::Backend(format!(
                "the input has {channels} channels, so no channel {}",
                request.channel
            ))
        })?;
        let info = CaptureInfo {
            sample_rate: RATE,
            period_frames: period,
            channels: channels as u16,
            input_latency_frames: Some(input_latency(device, streams[stream])?),
        };
        stats.set_opened(format!(
            "CoreAudio {}, f32 x{channels}, channel {}, IO buffer {period}",
            string(device, kAudioObjectPropertyName).unwrap_or_default(),
            request.channel,
        ));
        stats.set_realtime(true);

        let feed = CaptureFeed::new(sink, Arc::clone(&stats));
        let callback = CaptureCallback::new(feed, usize::from(request.channel), stats);
        let io = CaptureIo::start(device, callback)?;
        Ok(Capturing::new(info, move || {
            drop(io);
            drop(rate);
        }))
    }
}

// What the HAL says lies between the hardware and an IO cycle: the device's latency, its safety
// offset and the stream's own latency, all in frames.
fn input_latency(device: AudioObjectID, stream: AudioObjectID) -> Result<u32, DriverError> {
    // SAFETY: each of these is a UInt32 frame count.
    unsafe {
        let device_latency: u32 =
            get(device, kAudioDevicePropertyLatency, kAudioObjectPropertyScopeInput)?;
        let safety: u32 =
            get(device, kAudioDevicePropertySafetyOffset, kAudioObjectPropertyScopeInput)?;
        let stream_latency: u32 =
            get(stream, kAudioStreamPropertyLatency, kAudioObjectPropertyScopeGlobal)?;
        Ok(device_latency + safety + stream_latency)
    }
}
