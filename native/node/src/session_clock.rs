//! A session bundle's one clock (docs/design.md 8.11, decision 0026). The audio thread calls
//! nothing in the OS, so it cannot stamp a wall-clock time; it counts the frames it renders, and
//! the control thread reads that count and the wall clock in one call. The bundle's pcaps stamp
//! arrivals on the same `WallClock`, so a line's `us` and a packet's time share one anchor.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use napi_derive::napi;
use transport::record::WallClock;

pub struct SessionClock {
    frames: Arc<AtomicU64>,
    wall: WallClock,
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
        Self { frames, wall: WallClock::now() }
    }

    pub fn read(&self) -> ClockReading {
        let frame = self.frames.load(Ordering::Relaxed);
        let at = self.wall.at(Instant::now());
        ClockReading { frame: frame as f64, us: at.as_micros() as f64 }
    }

    /// The wall clock the bundle's recordings stamp arrivals on.
    pub fn wall(&self) -> WallClock {
        self.wall
    }
}
