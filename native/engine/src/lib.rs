//! The pure core of the mixer: what the audio thread runs, with no device, socket or Node in
//! it. docs/design.md section 6 is the map of what lives here.
#![forbid(unsafe_code)]

pub mod audio_thread;
pub mod backing;
pub mod block;
pub mod command;
pub mod driver;
pub mod graph;
pub mod ingest;
pub mod master;
pub mod params;
pub mod reactions;
pub mod slot;

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
