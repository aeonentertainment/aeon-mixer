use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
use std::thread;
use std::time::Duration;

use engine::driver::{Driver, DriverError, Render, Running};

/// A driver's start on a thread of its own, so the follower can stop waiting on one that hangs.
/// A start given up on finishes alone: what it returns is dropped on its thread, which stops a
/// stream that started late and frees whatever a failed start held.
pub struct Starting {
    result: Receiver<Result<Running, DriverError>>,
}

impl Starting {
    pub fn spawn(driver: Box<dyn Driver + Send>, render: Render) -> Result<Self, DriverError> {
        let (sender, result) = sync_channel(1);
        thread::Builder::new()
            .name("aeon-output-start".into())
            .spawn(move || {
                // Fails once the follower has given up, and the stream drops here
                let _ = sender.send(driver.start(render));
            })
            .map_err(|error| DriverError::Backend(format!("start thread: {error}")))?;
        Ok(Self { result })
    }

    /// What the start returned within `bound`, or None while it still runs.
    pub fn within(&self, bound: Duration) -> Option<Result<Running, DriverError>> {
        match self.result.recv_timeout(bound) {
            Ok(result) => Some(result),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Err(panicked())),
        }
    }

    /// What the start returns, however long it takes.
    pub fn finish(self) -> Result<Running, DriverError> {
        self.result.recv().unwrap_or_else(|_| Err(panicked()))
    }
}

fn panicked() -> DriverError {
    DriverError::Backend("the start panicked".into())
}
