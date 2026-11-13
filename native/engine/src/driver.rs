//! The clock boundary. Whoever calls `render` owns the clock: an OS backend in `devices`, or
//! the offline driver in tests and in `openEngine({ offline: true })`.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;

use crate::block::{BLOCK, RATE, Stereo};

/// Renders one block into the buffers it is handed. Runs on the audio thread.
pub type Render = Box<dyn FnMut(Stereo<'_>) + Send + 'static>;

/// What a started stream reports about itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub period_frames: u32,
    pub output_latency_frames: u32,
}

/// An OS audio output, implemented by the backends in `devices`. `start` hands `render` to the
/// device's callback and returns once the stream runs. Dropping the `Running` stops it.
pub trait Driver {
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError>;
}

/// What a device follower moves from one output to the next: the engine's `AudioThread`, or a
/// test's render. The follower holds it only while no stream runs, so moving it is not a crossing.
pub trait Movable: Send + 'static {
    /// One block, as `Render` does.
    fn render(&mut self, out: Stereo<'_>);
    /// Called between streams, never on the audio thread: one clock stopped and another starts.
    fn moved(&mut self);
    /// Where the follower writes each stream's granted period, `StreamInfo.period_frames`, as its
    /// `start` returns: the most it pulls at a normal wake. Taken before the movable is lent, since
    /// the stream holds it from then on. A movable that has no use for it keeps the default.
    fn callback_frames(&self) -> Arc<AtomicU32> {
        Arc::default()
    }
    /// Where the follower writes each stream's `StreamInfo.output_latency_frames` beside its
    /// period, so the guide clock follows the output the room hears. Taken and defaulted as
    /// `callback_frames` is.
    fn output_latency_frames(&self) -> Arc<AtomicU32> {
        Arc::default()
    }
}

pub struct Running {
    pub info: StreamInfo,
    stop: Option<Box<dyn FnOnce() + Send>>,
}

impl Running {
    pub fn new(info: StreamInfo, stop: impl FnOnce() + Send + 'static) -> Self {
        Self { info, stop: Some(Box::new(stop)) }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            stop();
        }
    }
}

#[derive(Debug)]
pub enum DriverError {
    NoDevice,
    Backend(String),
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => f.write_str("no output device"),
            Self::Backend(reason) => write!(f, "audio backend: {reason}"),
        }
    }
}

impl std::error::Error for DriverError {}

/// The test clock. Its caller pumps it, so it is not a `Driver`: it calls `render` on the
/// caller's thread, one block at a time, with no device and no socket, and hands the blocks back
/// interleaved so a test can assert on samples.
pub struct OfflineDriver {
    render: Render,
    left: [f32; BLOCK],
    right: [f32; BLOCK],
    frames_rendered: u64,
}

impl OfflineDriver {
    pub fn new(render: Render) -> Self {
        Self { render, left: [0.0; BLOCK], right: [0.0; BLOCK], frames_rendered: 0 }
    }

    pub fn info(&self) -> StreamInfo {
        StreamInfo { sample_rate: RATE, period_frames: BLOCK as u32, output_latency_frames: 0 }
    }

    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    /// Renders into `out`, interleaved, one block per `2 * BLOCK` samples, and allocates nothing,
    /// so a test runs a whole pump under the guard. Samples past the last whole block are left as
    /// they were.
    pub fn pump_into(&mut self, out: &mut [f32]) {
        let (blocks, _short) = out.as_chunks_mut::<{ 2 * BLOCK }>();
        for block in blocks {
            self.render_block();
            let (frames, _) = block.as_mut_slice().as_chunks_mut::<2>();
            for ((left, right), frame) in self.left.iter().zip(&self.right).zip(frames) {
                *frame = [*left, *right];
            }
        }
    }

    /// `pump_into` with a buffer of its own: the binding's convenience, on the control thread.
    pub fn pump(&mut self, blocks: usize) -> Vec<f32> {
        let mut out = vec![0.0; blocks * 2 * BLOCK];
        self.pump_into(&mut out);
        out
    }

    fn render_block(&mut self) {
        (self.render)(Stereo { left: &mut self.left, right: &mut self.right });
        self.frames_rendered += BLOCK as u64;
    }
}

/// A render of nothing: what the binding plays until it hosts an `AudioThread` (8.8).
pub fn silent_render() -> Render {
    Box::new(|mut out: Stereo<'_>| out.silence())
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    #[test]
    fn silence_renders_zeros_without_allocating() {
        let mut driver = OfflineDriver::new(silent_render());
        let mut out = [1.0f32; 3 * 2 * BLOCK];
        assert_no_alloc(|| driver.pump_into(&mut out));
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(driver.frames_rendered(), 3 * BLOCK as u64);

        // The binding's path: the same samples in a buffer of its own
        assert_eq!(driver.pump(1), vec![0.0; 2 * BLOCK]);
    }

    #[test]
    fn pump_interleaves_and_is_deterministic() {
        // A render that writes its frame count, so left and right are told apart in the output
        let counting = || -> Render {
            let mut frame = 0.0f32;
            Box::new(move |out: Stereo<'_>| {
                for (left, right) in out.left.iter_mut().zip(out.right) {
                    *left = frame;
                    *right = -frame;
                    frame += 1.0;
                }
            })
        };
        let mut a = OfflineDriver::new(counting());
        let mut b = OfflineDriver::new(counting());
        let mut first = [0.0f32; 2 * 2 * BLOCK];
        let mut second = [0.0f32; 2 * 2 * BLOCK];
        assert_no_alloc(|| {
            a.pump_into(&mut first);
            b.pump_into(&mut second);
        });
        assert_eq!(first, second);
        assert_eq!(first[0..4], [0.0, -0.0, 1.0, -1.0]);
        assert_eq!(first[first.len() - 2], (2 * BLOCK - 1) as f32);
    }

    #[test]
    fn info_is_the_fixed_numbers() {
        let driver = OfflineDriver::new(silent_render());
        assert_eq!(
            driver.info(),
            StreamInfo { sample_rate: 48_000, period_frames: 128, output_latency_frames: 0 }
        );
    }
}
