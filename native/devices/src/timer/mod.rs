//! The clock for a box with no sound card: a thread of ours sleeps to each block's deadline on
//! the OS's monotonic clock and renders the block. docs/decisions/0011-timer-clock.md has the
//! reasons and the numbers.

mod clock;
mod driver;
#[cfg(target_os = "macos")]
mod mach;
mod monotonic;
mod pacer;
mod priority;

pub use clock::Clock;
pub use driver::TimerDriver;
pub use monotonic::MonotonicClock;
pub use pacer::{MAX_CATCH_UP, Pacer};
