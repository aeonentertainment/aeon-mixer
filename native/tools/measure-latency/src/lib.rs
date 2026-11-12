//! Measures the mic path's latency. docs/decisions/0006-measuring-latency.md says why it works the
//! way it does and what its figures can and cannot say.

pub mod capture_tap;
pub mod chirp;
pub mod cli;
pub mod clock_line;
#[cfg(feature = "hardware")]
pub mod device;
pub mod feed;
pub mod ingest;
pub mod machine;
pub mod matched_filter;
pub mod offline;
pub mod opening;
pub mod packets;
pub mod physical;
pub mod programme;
pub mod reading;
pub mod recording;
pub mod report;
pub mod rig;
pub mod tap;

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
