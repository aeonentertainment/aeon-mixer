use std::thread;
use std::time::Duration;

use engine::driver::DriverError;
use objc2_core_audio::{
    kAudioDevicePropertyBufferFrameSize, kAudioDevicePropertyBufferFrameSizeRange,
    kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal,
};
use objc2_core_audio_types::AudioValueRange;

use super::property::{get, set};
use crate::device_holds::DeviceHolds;
use crate::device_properties::DeviceProperties;

/// Every device this process holds at 48 kHz and its period, its output streams and its inputs
/// alike.
pub static HOLDS: DeviceHolds<HalProperties> = DeviceHolds::new(HalProperties);

/// How long a sample rate change may take to land; the HAL applies it asynchronously.
const RATE_CHANGE: Duration = Duration::from_secs(1);

/// The HAL's nominal sample rate and IO buffer size of a device, by AudioObjectID.
pub struct HalProperties;

impl DeviceProperties for HalProperties {
    fn rate(&self, device: u32) -> Result<f64, DriverError> {
        // SAFETY: the nominal sample rate is a Float64.
        unsafe {
            get(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal)
        }
    }

    fn wait_for(&self, device: u32, rate: f64) -> Result<(), DriverError> {
        let step = Duration::from_millis(10);
        for _ in 0..RATE_CHANGE.as_millis() / step.as_millis() {
            if self.rate(device)? == rate {
                return Ok(());
            }
            thread::sleep(step);
        }
        Err(DriverError::Backend(format!("the device did not move to {rate} Hz")))
    }

    fn put_rate(&self, device: u32, rate: f64) -> Result<(), DriverError> {
        // SAFETY: the nominal sample rate is a Float64.
        unsafe {
            set(
                device,
                kAudioDevicePropertyNominalSampleRate,
                kAudioObjectPropertyScopeGlobal,
                &rate,
            )
        }
    }

    fn frames(&self, device: u32) -> Result<u32, DriverError> {
        // SAFETY: the buffer size is a UInt32.
        unsafe { get(device, kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal) }
    }

    // Clamped to the device's range, read now, after the rate has moved
    fn put_frames(&self, device: u32, frames: u32) -> Result<(), DriverError> {
        // SAFETY: the buffer size range is an AudioValueRange, and the buffer size a UInt32.
        unsafe {
            let range: AudioValueRange = get(
                device,
                kAudioDevicePropertyBufferFrameSizeRange,
                kAudioObjectPropertyScopeGlobal,
            )?;
            let frames = f64::from(frames).clamp(range.mMinimum, range.mMaximum) as u32;
            set(
                device,
                kAudioDevicePropertyBufferFrameSize,
                kAudioObjectPropertyScopeGlobal,
                &frames,
            )
        }
    }
}
