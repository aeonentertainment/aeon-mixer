//! The wall clock a recording stamps arrivals with (docs/design.md 8.4). It reads the system
//! clock once and runs on the monotonic clock from then on, so a step in the system clock
//! mid-song moves no stamp. A session bundle stamps its lines and its pcaps from one (8.11).

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy)]
pub struct WallClock {
    started: Instant,
    since_epoch: Duration,
}

impl WallClock {
    pub fn now() -> Self {
        let started = Instant::now();
        let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Self { started, since_epoch }
    }

    /// `at` as the time since the Unix epoch.
    pub fn at(&self, at: Instant) -> Duration {
        self.since_epoch + at.saturating_duration_since(self.started)
    }
}
