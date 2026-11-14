mod callback;
mod capture;
mod capture_callback;
mod capture_io;
mod driver;
mod hal_rate;
mod io;
mod property;
mod steps;
mod watch;

use engine::driver::DriverError;
use objc2_core_audio::{
    AudioObjectID, kAudioDevicePropertyDeviceUID, kAudioDevicePropertyStreams,
    kAudioHardwarePropertyDefaultInputDevice, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioHardwarePropertyDevices, kAudioObjectPropertyName, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeInput, kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};

pub use capture::CoreAudioCapture;
pub use driver::CoreAudioDriver;
pub use watch::Watch;

use crate::output::Output;
use property::{get, get_list, string};

const SYSTEM: AudioObjectID = kAudioObjectSystemObject as AudioObjectID;

/// Every device with an output stream, by UID, with the system's default output marked. A device
/// whose UID or name cannot be read, as one being unplugged, is left out rather than failing the
/// list, so the follower still sees the outputs that remain.
pub fn outputs() -> Result<Vec<Output>, DriverError> {
    let default = default_device().ok();
    Ok(output_devices()?
        .into_iter()
        .filter_map(|device| {
            Some(Output {
                id: string(device, kAudioDevicePropertyDeviceUID).ok()?,
                name: string(device, kAudioObjectPropertyName).ok()?,
                is_default: Some(device) == default,
            })
        })
        .collect())
}

/// Every device with an input stream, by UID, with the system's default input marked.
pub fn inputs() -> Result<Vec<crate::Input>, DriverError> {
    let default = default_input().ok();
    input_devices()?
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

fn input_devices() -> Result<Vec<AudioObjectID>, DriverError> {
    // SAFETY: the device list is an array of AudioObjectIDs.
    let devices: Vec<AudioObjectID> = unsafe {
        get_list(SYSTEM, kAudioHardwarePropertyDevices, kAudioObjectPropertyScopeGlobal)?
    };
    Ok(devices
        .into_iter()
        .filter(|device| !input_streams(*device).unwrap_or_default().is_empty())
        .collect())
}

fn input_streams(device: AudioObjectID) -> Result<Vec<AudioObjectID>, DriverError> {
    // SAFETY: a device's streams are an array of AudioObjectIDs.
    unsafe { get_list(device, kAudioDevicePropertyStreams, kAudioObjectPropertyScopeInput) }
}

fn default_input() -> Result<AudioObjectID, DriverError> {
    // SAFETY: the default input is an AudioObjectID.
    let device: AudioObjectID = unsafe {
        get(SYSTEM, kAudioHardwarePropertyDefaultInputDevice, kAudioObjectPropertyScopeGlobal)?
    };
    // kAudioObjectUnknown
    if device == 0 { Err(DriverError::NoDevice) } else { Ok(device) }
}

fn find_input(uid: &str) -> Result<AudioObjectID, DriverError> {
    input_devices()?
        .into_iter()
        .find(|device| string(*device, kAudioDevicePropertyDeviceUID).is_ok_and(|id| id == uid))
        .ok_or(DriverError::NoDevice)
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
