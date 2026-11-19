use std::fmt;

use engine::driver::DriverError;
use objc2_core_audio::{
    kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyBufferFrameSizeRange,
    kAudioDevicePropertyDeviceIsAlive, kAudioDevicePropertyDeviceIsRunning,
    kAudioDevicePropertyDeviceIsRunningSomewhere, kAudioDevicePropertyHogMode,
    kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal,
    kAudioStreamPropertyIsActive, kAudioStreamPropertyPhysicalFormat,
    kAudioStreamPropertyVirtualFormat,
};
use objc2_core_audio_types::{
    AudioStreamBasicDescription, AudioValueRange, kAudioFormatFlagIsFloat,
};

use super::property::get;

/// What the HAL says of an output device and its first output stream, read without starting it:
/// every property the driver sets, and the ones that tell a wedged device from a healthy one.
/// measure-sink prints it around an open for a person chasing #232. A value the HAL will not give
/// shows as `?`.
pub struct DeviceState {
    buffer: Option<u32>,
    range: Option<AudioValueRange>,
    rate: Option<f64>,
    hog: Option<i32>,
    alive: Option<u32>,
    running_here: Option<u32>,
    running_somewhere: Option<u32>,
    stream_active: Option<u32>,
    virtual_format: Option<AudioStreamBasicDescription>,
    physical_format: Option<AudioStreamBasicDescription>,
}

impl DeviceState {
    /// The output `uid` names, or the default output.
    pub fn read(uid: Option<&str>) -> Result<Self, DriverError> {
        let device = match uid {
            Some(uid) => super::find(uid)?,
            None => super::default_device()?,
        };
        let stream =
            super::output_streams(device).ok().and_then(|streams| streams.first().copied());
        let global = kAudioObjectPropertyScopeGlobal;
        // SAFETY: the buffer size, the is-alive and is-running flags and the stream's is-active
        // flag are UInt32s, the range an AudioValueRange, the rate a Float64, hog mode a pid_t and
        // both formats AudioStreamBasicDescriptions.
        unsafe {
            Ok(Self {
                buffer: get(device, kAudioDevicePropertyBufferFrameSize, global).ok(),
                range: get(device, kAudioDevicePropertyBufferFrameSizeRange, global).ok(),
                rate: get(device, kAudioDevicePropertyNominalSampleRate, global).ok(),
                hog: get(device, kAudioDevicePropertyHogMode, global).ok(),
                alive: get(device, kAudioDevicePropertyDeviceIsAlive, global).ok(),
                running_here: get(device, kAudioDevicePropertyDeviceIsRunning, global).ok(),
                running_somewhere: get(
                    device,
                    kAudioDevicePropertyDeviceIsRunningSomewhere,
                    global,
                )
                .ok(),
                stream_active: stream
                    .and_then(|stream| get(stream, kAudioStreamPropertyIsActive, global).ok()),
                virtual_format: stream
                    .and_then(|stream| get(stream, kAudioStreamPropertyVirtualFormat, global).ok()),
                physical_format: stream.and_then(|stream| {
                    get(stream, kAudioStreamPropertyPhysicalFormat, global).ok()
                }),
            })
        }
    }
}

impl fmt::Display for DeviceState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let range = self.range.map(|range| format!("{} to {}", range.mMinimum, range.mMaximum));
        write!(
            f,
            "IO buffer {} ({}), {} Hz, hog {}, alive {}, running here {}, somewhere {}, \
             stream active {}, virtual {}, physical {}",
            shown(self.buffer),
            shown(range),
            shown(self.rate),
            shown(self.hog),
            shown(self.alive),
            shown(self.running_here),
            shown(self.running_somewhere),
            shown(self.stream_active),
            shown(self.virtual_format.map(format)),
            shown(self.physical_format.map(format)),
        )
    }
}

fn shown(value: Option<impl fmt::Display>) -> String {
    value.map_or_else(|| "?".into(), |value| value.to_string())
}

fn format(format: AudioStreamBasicDescription) -> String {
    let kind = if format.mFormatFlags & kAudioFormatFlagIsFloat == 0 { "int" } else { "float" };
    format!(
        "{}-bit {kind} x{} {} Hz",
        format.mBitsPerChannel, format.mChannelsPerFrame, format.mSampleRate
    )
}
