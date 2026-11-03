mod callback;
mod driver;
mod io;
mod property;

use engine::driver::DriverError;
use objc2_core_audio::{
    AudioObjectID, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyScopeOutput,
    kAudioObjectSystemObject,
};

pub use driver::CoreAudioDriver;

use crate::output::Output;
use property::{get, get_list, string};

const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

/// Every device with an output stream, by UID, with the system's default output marked.
pub fn outputs() -> Result<Vec<Output>, DriverError> {
    let default = default_device().ok();
    output_devices()?
        .into_iter()
        .map(|device| {
            Ok(Output {
                id: string(device, kAudioDevicePropertyDeviceUID)?,
                name: string(device, kAudioObjectPropertyName)?,
                is_default: Some(device) == default,
            })
        })
        .collect()
}

fn output_devices() -> Result<Vec<AudioObjectID>, DriverError> {
    // SAFETY: the device list is an array of AudioObjectIDs.
    let devices: Vec<AudioObjectID> = unsafe {
        get_list(SYSTEM, kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal)?
    };
    Ok(devices
        .into_iter()
        .filter(|device| !output_streams(*device).unwrap_or_default().is_empty())
        .collect())
}

fn output_streams(device: AudioObjectID) -> Result<Vec<AudioObjectID>, DriverError> {
    // SAFETY: a device's streams are an array of AudioObjectIDs.
    unsafe { get_list(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeOutput) }
}

fn default_device() -> Result<AudioObjectID, DriverError> {
    // SAFETY: the default output is an AudioObjectID.
    let device: AudioObjectID = unsafe {
        get(SYSTEM, kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal)?
    };
    // kAudioObjectUnknown
    if device == 0 { Err(DriverError::NoDevice) } else { Ok(device) }
}

fn find(uid: &str) -> Result<AudioObjectID, DriverError> {
    output_devices()?
        .into_iter()
        .find(|device| string(*device, kAudioDevicePropertyDeviceUID).is_ok_and(|id| id == uid))
        .ok_or(DriverError::NoDevice)
}
