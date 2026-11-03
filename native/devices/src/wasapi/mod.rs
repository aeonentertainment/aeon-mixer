mod com;
mod driver;
mod event;
mod format;
mod mode;
mod stream;

use std::thread;

use engine::driver::DriverError;
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    DEVICE_STATE_ACTIVE, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::core::PCWSTR;

pub use driver::WasapiDriver;
pub use mode::WasapiMode;

use crate::output::Output;
use com::Com;

/// The active render endpoints, by MMDevice id, with the default console output marked. Runs on
/// a thread of its own so the caller's COM apartment never matters.
pub fn outputs() -> Result<Vec<Output>, DriverError> {
    thread::spawn(|| {
        let _com = Com::init()?;
        let enumerator = enumerator()?;
        // SAFETY: COM is initialised on this thread.
        unsafe {
            let default = enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .ok()
                .and_then(|device| id(&device).ok());
            let devices = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
                .map_err(failed("list the outputs"))?;
            let count = devices.GetCount().map_err(failed("count the outputs"))?;
            (0..count)
                .map(|index| {
                    let device = devices.Item(index).map_err(failed("read an output"))?;
                    let id = id(&device)?;
                    Ok(Output {
                        is_default: default.as_ref() == Some(&id),
                        name: name(&device)?,
                        id,
                    })
                })
                .collect()
        }
    })
    .join()
    .map_err(|_| DriverError::Backend("listing the outputs panicked".into()))?
}

/// The output with this id, or the default console output.
fn device(id: Option<&str>) -> Result<IMMDevice, DriverError> {
    let enumerator = enumerator()?;
    // SAFETY: COM is initialised on this thread; `wide` is nul-terminated and outlives the call.
    unsafe {
        match id {
            Some(id) => {
                let wide: Vec<u16> = id.encode_utf16().chain([0]).collect();
                enumerator.GetDevice(PCWSTR(wide.as_ptr())).map_err(|_| DriverError::NoDevice)
            }
            None => enumerator
                .GetDefaultAudioEndpoint(eRender, eConsole)
                .map_err(|_| DriverError::NoDevice),
        }
    }
}

fn enumerator() -> Result<IMMDeviceEnumerator, DriverError> {
    // SAFETY: COM is initialised on the calling thread.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(failed("list devices"))
}

fn id(device: &IMMDevice) -> Result<String, DriverError> {
    // SAFETY: GetId returns a COM-allocated string, freed here after it is copied.
    unsafe {
        let raw = device.GetId().map_err(failed("read a device id"))?;
        let id = raw.to_string();
        CoTaskMemFree(Some(raw.0.cast_const().cast()));
        id.map_err(|_| DriverError::Backend("a device id is not UTF-16".into()))
    }
}

fn name(device: &IMMDevice) -> Result<String, DriverError> {
    // SAFETY: COM is initialised on the calling thread.
    unsafe {
        let store =
            device.OpenPropertyStore(STGM_READ).map_err(failed("open a device's properties"))?;
        let name =
            store.GetValue(&PKEY_Device_FriendlyName).map_err(failed("read a device name"))?;
        Ok(name.to_string())
    }
}

fn failed(doing: &'static str) -> impl FnOnce(windows::core::Error) -> DriverError {
    move |error| DriverError::Backend(format!("WASAPI could not {doing}: {error}"))
}
