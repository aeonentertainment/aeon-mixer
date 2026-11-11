use std::time::Instant;

use engine::driver::DriverError;

/// One delivery from an input: mono samples of the channel asked for, and when the thread woke to
/// receive them. The samples were on the hardware before `at`, however late the wake was.
pub struct Captured<'a> {
    pub samples: &'a [f32],
    pub at: Instant,
}

/// Receives each delivery on the capture thread, which is realtime: it allocates nothing, locks
/// nothing and returns at once.
pub type Sink = Box<dyn FnMut(Captured<'_>) + Send + 'static>;

/// What a started input reports about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureInfo {
    pub sample_rate: u32,
    pub period_frames: u32,
    pub channels: u16,
    /// What the OS says lies between the hardware and a delivery, beyond the period; none when it
    /// says nothing. No software can check it.
    pub input_latency_frames: Option<u32>,
}

/// An OS audio input, implemented for each backend. `start` hands `sink` to the device's capture
/// thread and returns once the stream runs. Dropping the `Capturing` stops it.
pub trait Capture {
    fn start(self: Box<Self>, sink: Sink) -> Result<Capturing, DriverError>;
}

pub struct Capturing {
    pub info: CaptureInfo,
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl Capturing {
    pub fn new(info: CaptureInfo, stop: impl FnOnce() + Send + 'static) -> Self {
        Self { info, stop: Some(Box::new(stop)) }
    }
}

impl Drop for Capturing {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}
