use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use engine::driver::{DriverError, Movable};

use super::current::Current;
use super::following::{Following, STALL};
use super::start_error::StartError;
use super::system::System;
use crate::request::Request;
use crate::stats::Stats;
use crate::wake::Wake;

/// Keeps the engine on the room's output. It owns the running stream on a thread of its own, so
/// no OS listener and no Node call ever starts or stops a device. Dropping it stops the stream,
/// and returns once the follower thread has dropped the `Movable`.
pub struct Follower {
    current: Current,
    wake: Wake,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Follower {
    /// Opens where `request` says, `request.device` being the pinned output, and follows from
    /// there. Blocks while the first stream opens, as a driver's `start` does. `stats` records
    /// every stream it opens. A start that fails leaves nothing behind: no stream, no watch, and
    /// the `Movable` dropped. One refused after `REFUSE_AFTER` holds its device, on a thread of its
    /// own, until the OS's start returns.
    pub fn start(
        request: Request,
        movable: impl Movable,
        system: impl System,
        stats: Arc<Stats>,
    ) -> Result<Self, StartError> {
        Self::stalling_after(STALL, request, movable, system, stats)
    }

    /// `start`, with a stream lost after `stall` without a callback. Tests that never pump a
    /// stream pass one longer than they run.
    pub(super) fn stalling_after(
        stall: Duration,
        request: Request,
        movable: impl Movable,
        system: impl System,
        stats: Arc<Stats>,
    ) -> Result<Self, StartError> {
        let (wake, woken) = Wake::new();
        stats.wake_on_loss(wake.clone());
        // Watching before the first open, so a change while it opens is not missed
        let watch = system.watch(wake.clone()).map_err(StartError::Backend)?;
        let following = Following::open(request, movable, system, stats, watch, stall)?;
        let current = following.current();
        let stopping = Arc::new(AtomicBool::new(false));
        let thread = thread::Builder::new()
            .name("aeon-output-follower".into())
            .spawn({
                let stopping = Arc::clone(&stopping);
                move || following.run(&woken, &stopping)
            })
            .map_err(|error| {
                StartError::Backend(DriverError::Backend(format!("follower thread: {error}")))
            })?;
        Ok(Self { current, wake, stopping, thread: Some(thread) })
    }

    /// Where the engine plays now.
    pub fn current(&self) -> Current {
        self.current.clone()
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        self.wake.raise();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
