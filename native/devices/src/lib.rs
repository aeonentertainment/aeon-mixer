//! The engine's `Driver` for each OS: CoreAudio on macOS, WASAPI on Windows, ALSA on Linux, and
//! the measure-sink binary that measures them. docs/design.md section 8.3 is the brief and
//! docs/decisions/0003-device-backends.md the numbers behind it. Each OS has a `Capture` too, for
//! measure-latency today and for the house-mic input of #52 next.
//!
//! The OS code sits behind the `hardware` feature, so the workspace builds and tests with no
//! audio stack. Without it this crate is the pieces every backend shares: the feed that turns
//! 128-frame renders into whatever the device asks for, the feed that takes one channel of what an
//! input delivers, and the stats a stream records.

mod capture;
mod capture_feed;
mod capture_request;
#[cfg(any(test, all(feature = "hardware", target_os = "linux")))]
mod channel_count;
mod feed;
mod output;
mod report;
mod request;
mod sample;
mod stats;

pub use capture::{Capture, CaptureInfo, Captured, Capturing, Sink};
pub use capture_feed::CaptureFeed;
pub use capture_request::CaptureRequest;
pub use feed::{Feed, interleave};
pub use output::{Input, Output};
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
pub use alsa::{AlsaCapture, AlsaDriver, inputs, outputs};
#[cfg(all(feature = "hardware", target_os = "macos"))]
pub use coreaudio::{CoreAudioCapture, CoreAudioDriver, inputs, outputs};
#[cfg(all(feature = "hardware", windows))]
pub use wasapi::{WasapiCapture, WasapiDriver, WasapiMode, inputs, outputs};

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

/// This OS's input for `request`, ready to start. `stats` is the stream's record: its underruns are
/// frames the device lost because nobody took them in time.
#[cfg(feature = "hardware")]
pub fn capture(request: CaptureRequest, stats: std::sync::Arc<Stats>) -> Box<dyn Capture + Send> {
    #[cfg(target_os = "linux")]
    return Box::new(AlsaCapture::new(request, stats));
    #[cfg(target_os = "macos")]
    return Box::new(CoreAudioCapture::new(request, stats));
    #[cfg(windows)]
    return Box::new(WasapiCapture::new(request, stats));
}

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;
