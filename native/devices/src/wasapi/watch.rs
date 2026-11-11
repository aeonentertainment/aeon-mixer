use std::sync::mpsc;
use std::thread::{self, JoinHandle};

use engine::driver::DriverError;

use super::com::Com;
use super::notifications::Notifications;
use super::{enumerator, failed};
use crate::wake::Wake;

/// Endpoint notifications for the follower, registered from a COM thread of its own so the
/// caller's apartment never matters, and unregistered from it when dropped.
pub struct Watch {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Returns once the notifications are registered.
    pub fn start(wake: Wake) -> Result<Self, DriverError> {
        let (registered, ready) = mpsc::sync_channel(1);
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = thread::Builder::new()
            .name("aeon-output-watch".into())
            .spawn(move || {
                let watching = (|| -> Result<_, DriverError> {
                    let com = Com::init()?;
                    let enumerator = enumerator()?;
                    let client = Notifications::client(wake);
                    // SAFETY: COM is initialised on this thread, and `client` lives until it is
                    // unregistered below.
                    unsafe { enumerator.RegisterEndpointNotificationCallback(&client) }
                        .map_err(failed("watch the outputs"))?;
                    Ok((com, enumerator, client))
                })();
                let (_com, enumerator, client) = match watching {
                    Ok(watching) => watching,
                    Err(error) => {
                        let _ = registered.send(Err(error));
                        return;
                    }
                };
                let _ = registered.send(Ok(()));
                let _ = stopped.recv();
                // SAFETY: registered above on this thread; COM goes after both objects
                let _ = unsafe { enumerator.UnregisterEndpointNotificationCallback(&client) };
                drop(client);
                drop(enumerator);
            })
            .map_err(|error| DriverError::Backend(format!("output watch thread: {error}")))?;
        let watch = Self { stop: Some(stop), thread: Some(thread) };
        match ready.recv() {
            Ok(Ok(())) => Ok(watch),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(DriverError::Backend("the output watch ended before it began".into())),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
