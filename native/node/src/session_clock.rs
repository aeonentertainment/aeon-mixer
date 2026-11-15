//! A session bundle's one clock (docs/design.md 8.11, decision 0026). The audio thread calls
//! nothing in the OS, so it cannot stamp a wall-clock time; it counts the frames it renders, and
//! the control thread reads that count and the wall clock in one call. The wall clock is made as
//! a recording makes the one it stamps arrivals with: the time since the epoch when the mixer was
//! built, plus the monotonic time since, so a bundle's lines and its pcaps share a clock.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use napi_derive::napi;

pub struct SessionClock {
    frames: Arc<AtomicU64>,
    started: Instant,
    since_epoch: Duration,
}

/// Both cross as f64, since a u64 is not a JavaScript number: exact to 2^53, which is six
/// thousand years of frames and three centuries of microseconds.
#[napi(object)]
pub struct ClockReading {
    /// Frames the audio thread has rendered since the engine opened.
    pub frame: f64,
    /// Microseconds since the Unix epoch.
    pub us: f64,
}

impl SessionClock {
    /// `frames` is the audio thread's count, `Controls::frames`.
    pub fn new(frames: Arc<AtomicU64>) -> Self {
        let started = Instant::now();
        let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        Self { frames, started, since_epoch }
    }

    pub fn read(&self) -> ClockReading {
        let frame = self.frames.load(Ordering::Relaxed);
        let at = self.since_epoch + self.started.elapsed();
        ClockReading { frame: frame as f64, us: at.as_micros() as f64 }
    }
}
