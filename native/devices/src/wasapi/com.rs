use engine::driver::DriverError;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

/// COM on the current thread, multithreaded, for as long as this lives. The backend makes its
/// own threads for COM work, so Node's or Electron's apartment never matters.
pub struct Com;

impl Com {
    pub fn init() -> Result<Self, DriverError> {
        // SAFETY: paired with CoUninitialize in drop, on the same thread.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|error| DriverError::Backend(format!("COM: {error}")))?;
        Ok(Com)
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: this thread initialised COM in `init`.
        unsafe { CoUninitialize() };
    }
}
