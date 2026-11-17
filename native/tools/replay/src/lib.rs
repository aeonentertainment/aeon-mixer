//! replay: a slot's recording through the real chain on the offline driver, beside libopus's
//! own decode of the same packets (docs/design.md 8.9, decision 0017), or a session bundle's
//! whole song (8.11). It opens no device and no socket. The same input replays to the same
//! samples every time. The `replay` binary prints what it heard; `scorecard` reads a bundle's
//! song through `bundle::song::play`.

pub mod bundle;
pub mod capture;
pub mod levels;
pub mod reference;
pub mod slot_replay;
pub mod stream;
pub mod wav;

#[cfg(test)]
mod round_trip;

// Tests wrap every pump in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
