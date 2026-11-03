//! The engine's `Driver` for each OS: CoreAudio on macOS, WASAPI on Windows, ALSA on Linux, and
//! the measure-sink binary that measures them. docs/design.md section 8.3 is the brief and
//! docs/decisions/0003-device-backends.md the numbers behind it.
//!
//! The OS code sits behind the `hardware` feature, so the workspace builds and tests with no
//! audio stack. Without it this crate is the pieces every backend shares: the feed that turns
//! 128-frame renders into whatever the device asks for, and the stats a stream records.

mod feed;
mod output;
mod report;
mod request;
mod sample;
mod stats;

pub use feed::{Feed, interleave};
pub use output::Output;
pub use report::Report;
pub use request::Request;
pub use sample::Sample;
pub use stats::{BUCKET, BUCKETS, Stats};

#[cfg(all(feature = "hardware", target_os = "linux"))]
mod alsa;
#[cfg(all(feature = "hardware", target_os = "macos"))]
mod coreaudio;
#[cfg(all(feature = "hardware", windows))]
mod wasapi;

#[cfg(all(feature = "hardware", target_os = "linux"))]
pub use alsa::{AlsaDriver, outputs};
#[cfg(all(feature = "hardware", target_os = "macos"))]
pub use coreaudio::{CoreAudioDriver, outputs};
#[cfg(all(feature = "hardware", windows))]
pub use wasapi::{WasapiDriver, WasapiMode, outputs};

#[cfg(all(feature = "hardware", not(any(target_os = "linux", target_os = "macos", windows))))]
compile_error!("the hardware feature has a backend for Linux, macOS and Windows only");

/// This OS's backend for `request`, ready to start. `stats` is the stream's record; keep the
/// other `Arc` to read it.
#[cfg(feature = "hardware")]
pub fn driver(
    request: Request,
    stats: std::sync::Arc<Stats>,
) -> Box<dyn engine::driver::Driver + Send> {
    #[cfg(target_os = "linux")]
    return Box::new(AlsaDriver::new(request, stats));
    #[cfg(target_os = "macos")]
    return Box::new(CoreAudioDriver::new(request, stats));
    #[cfg(windows)]
    return Box::new(WasapiDriver::new(request, WasapiMode::Auto, stats));
}

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
