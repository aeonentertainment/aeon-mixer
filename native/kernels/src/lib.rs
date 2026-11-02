//! The DSP leaves the engine runs on the audio thread: the pitch corrector and its detector, the
//! stretcher, the reverb. The C++ ones sit behind the `extern "C"` shims in cpp/shim, and this
//! crate is the only place that calls them, so the engine keeps `#![forbid(unsafe_code)]`.
//! Every `process` allocates nothing and locks nothing once its kernel is built.

mod corrector;
mod ffi;
mod pitch;
mod reverb;
mod stretch;
#[cfg(test)]
mod test_support;
mod yin;

pub use corrector::PitchCorrector;
pub use pitch::{EVERY_PITCH_CLASS, Pitch, PitchClassMask, pitch_class_of};
pub use reverb::Reverb;
pub use stretch::{Preset, Stretch};
pub use yin::YinDetector;

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
// It sees Rust's allocations only: the C++ kernels' own promise covers theirs.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
