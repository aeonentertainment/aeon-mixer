use std::thread;

use engine::driver::{Driver, DriverError, Render, Running};

/// The test OS's driver: its start parks its thread for the life of the process, as a start the
/// OS never returns from would.
pub struct HangingStart;

impl Driver for HangingStart {
    fn start(self: Box<Self>, _render: Render) -> Result<Running, DriverError> {
        loop {
            thread::park();
        }
    }
}
