//! The clock boundary. Whoever calls `render` owns the clock: an OS backend in `devices`, or
//! the offline driver in tests and in `openEngine({ offline: true })`.

use std::fmt;

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

/// An OS audio output. `start` hands `render` to the device's callback and returns once the
/// stream runs. Dropping the `Running` stops it.
pub trait Driver {
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError>;
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

/// The test clock. It calls `render` on the caller's thread, one block per step, with no device
/// and no socket, and hands the blocks back interleaved so a test can assert on samples.
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

    /// Renders `blocks` blocks and returns them interleaved. The output buffer is the caller's;
    /// the renders themselves allocate nothing.
    pub fn pump(&mut self, blocks: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(blocks * BLOCK * 2);
        for _ in 0..blocks {
            self.render_block();
            for (left, right) in self.left.iter().zip(&self.right) {
                out.push(*left);
                out.push(*right);
            }
        }
        out
    }

    fn render_block(&mut self) {
        (self.render)(Stereo { left: &mut self.left, right: &mut self.right });
        self.frames_rendered += BLOCK as u64;
    }
}

/// What the engine renders between songs, and all it renders until the graph workstream lands.
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
        assert_no_alloc(|| driver.render_block());
        assert!(driver.left.iter().chain(&driver.right).all(|s| *s == 0.0));

        let out = driver.pump(3);
        assert_eq!(out.len(), 3 * BLOCK * 2);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(driver.frames_rendered(), 4 * BLOCK as u64);
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
        let first = OfflineDriver::new(counting()).pump(2);
        let second = OfflineDriver::new(counting()).pump(2);
        assert_eq!(first, second);
        assert_eq!(first[0..4], [0.0, -0.0, 1.0, -1.0]);
        assert_eq!(first[2 * BLOCK * 2 - 2], (2 * BLOCK - 1) as f32);
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
