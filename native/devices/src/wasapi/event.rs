use engine::driver::DriverError;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::CreateEventW;

/// The auto-reset event WASAPI signals once a period. Closed on drop.
pub struct Event(pub HANDLE);

impl Event {
    pub fn new() -> Result<Self, DriverError> {
        // SAFETY: an unnamed auto-reset event with default security, owned by the result.
        unsafe { CreateEventW(None, false, false, None) }
            .map(Event)
            .map_err(|error| DriverError::Backend(format!("WASAPI event: {error}")))
    }
}

impl Drop for Event {
    fn drop(&mut self) {
        // SAFETY: created in `new` and closed only here.
        let _ = unsafe { CloseHandle(self.0) };
    }
}
