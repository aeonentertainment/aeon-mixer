//! bench: the mixer alone on a real clock, with no host application around it (docs/design.md
//! 8.9). `bench play` plays a session bundle as `replay` plays it, and says whether the clock got
//! every block in time. It opens a real output only when built with `hardware` and run with
//! `--open-device`; otherwise it runs on the timer clock, which opens nothing.

pub mod cli;
pub mod play;
pub mod render_times;
pub mod run_bundle;

#[cfg(test)]
mod tests;

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
