use std::thread;
use std::time::Duration;

use engine::driver::DriverError;
use objc2_core_audio::{kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal};

use super::property::{get, set};
use crate::nominal_rate::NominalRate;
use crate::rate_holds::RateHolds;

/// Every device this process holds at 48 kHz, its output streams and its inputs alike.
pub static RATES: RateHolds<HalRate> = RateHolds::new(HalRate);

/// How long a sample rate change may take to land; the HAL applies it asynchronously.
const RATE_CHANGE: Duration = Duration::from_secs(1);

/// The HAL's nominal sample rate of a device, by AudioObjectID.
pub struct HalRate;

impl NominalRate for HalRate {
    fn rate(&self, device: u32) -> Result<f64, DriverError> {
        // SAFETY: the nominal sample rate is a Float64.
        unsafe {
            get(device, kAudioDevicePropertyNominalSampleRate, kAudioObjectPropertyScopeGlobal)
        }
    }

    fn set_rate(&self, device: u32, rate: f64) -> Result<(), DriverError> {
        // SAFETY: the nominal sample rate is a Float64.
        unsafe {
            set(
                device,
                kAudioDevicePropertyNominalSampleRate,
                kAudioObjectPropertyScopeGlobal,
                &rate,
            )?
        };
        let step = Duration::from_millis(10);
        for _ in 0..RATE_CHANGE.as_millis() / step.as_millis() {
            if self.rate(device)? == rate {
                return Ok(());
            }
            thread::sleep(step);
        }
        Err(DriverError::Backend(format!("the device did not move to {rate} Hz")))
    }
}
