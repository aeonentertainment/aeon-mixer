//! bench: the mixer alone on a real clock, with no host application around it (docs/design.md
//! 8.9). `bench play` plays a session bundle as `replay` plays it, and says whether the clock got
//! every block in time. `loopback` reads chirps back through a house mic's capture slot, its
//! output and input handed to it. A real device opens only in a build with `hardware` and a run
//! with `--open-device`; without it `play` runs on the timer clock, which opens nothing.

pub mod cli;
pub mod loopback;
pub mod play;
pub mod render_times;
pub mod round_trip;
pub mod run_bundle;

#[cfg(test)]
mod tests;

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
