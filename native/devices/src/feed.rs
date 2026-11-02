use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use engine::block::{BLOCK, Stereo};
use engine::driver::Render;

use crate::sample::Sample;
use crate::stats::Stats;

/// Serves a device callback of any size from the engine's 128-frame renders. A backend owns one
/// on its audio thread and asks it for exactly the frames the device wants; a block is rendered
/// whenever the last one is spent, so no audio is held beyond the one block in hand.
pub struct Feed {
    render: Render,
    left: [f32; BLOCK],
    right: [f32; BLOCK],
    next: usize,
    stats: Arc<Stats>,
    faulted: bool,
}

impl Feed {
    pub fn new(render: Render, stats: Arc<Stats>) -> Self {
        Self { render, left: [0.0; BLOCK], right: [0.0; BLOCK], next: BLOCK, stats, faulted: false }
    }

    /// Hands out the next `frames` frames as planar chunks. `write(at, left, right)` gets each
    /// chunk with its offset into the callback's frames.
    pub fn take(&mut self, frames: usize, mut write: impl FnMut(usize, &[f32], &[f32])) {
        let mut at = 0;
        while at < frames {
            if self.next == BLOCK {
                self.render_block();
            }
            let count = (frames - at).min(BLOCK - self.next);
            let chunk = self.next..self.next + count;
            write(at, &self.left[chunk.clone()], &self.right[chunk]);
            self.next += count;
            at += count;
        }
    }

    /// `take` into one interleaved buffer of `channels` channels.
    pub fn fill<S: Sample>(&mut self, out: &mut [S], channels: usize) {
        if channels == 0 {
            return;
        }
        self.take(out.len() / channels, |at, left, right| {
            let span = at * channels..(at + left.len()) * channels;
            interleave(left, right, &mut out[span], channels, 0);
        });
    }

    // A panic in render must not unwind into the OS's callback. The block is silence, the stream
    // is marked failed, and render is never called again: its state may be torn.
    fn render_block(&mut self) {
        self.next = 0;
        if !self.faulted {
            let Self { render, left, right, .. } = self;
            if catch_unwind(AssertUnwindSafe(|| render(Stereo { left, right }))).is_ok() {
                return;
            }
            self.faulted = true;
            self.stats.fail();
        }
        Stereo { left: &mut self.left, right: &mut self.right }.silence();
    }
}

/// Writes planar stereo into an interleaved buffer that carries `channels` of the device's
/// channels, the first of them channel `first`. Device channel 0 is left, 1 is right, and any
/// others are silent. A buffer with no channels, which an OS can hand over, is left alone.
pub fn interleave<S: Sample>(
    left: &[f32],
    right: &[f32],
    out: &mut [S],
    channels: usize,
    first: usize,
) {
    if channels == 0 {
        return;
    }
    for ((frame, l), r) in out.chunks_exact_mut(channels).zip(left).zip(right) {
        for (channel, sample) in (first..).zip(frame.iter_mut()) {
            *sample = match channel {
                0 => S::from_f32(*l),
                1 => S::from_f32(*r),
                _ => S::default(),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    // Left carries the frame count and right its negation, so order, channel and gaps all show
    fn counting() -> Render {
        let mut frame = 0.0f32;
        Box::new(move |out: Stereo<'_>| {
            for (left, right) in out.left.iter_mut().zip(out.right) {
                *left = frame;
                *right = -frame;
                frame += 1.0;
            }
        })
    }

    #[test]
    fn callbacks_of_any_size_get_a_continuous_stream() {
        let stats = Arc::new(Stats::new());
        let mut feed = Feed::new(counting(), stats);
        // 64 and 128 are the periods we ask for; 48, 100 and 441 are what devices hand back
        let sizes = [48, 64, 100, 128, 441, 64, 128];
        let total: usize = sizes.iter().sum();
        let mut out = vec![0.0f32; total * 2];
        assert_no_alloc(|| {
            let mut rest = out.as_mut_slice();
            for frames in sizes {
                let (callback, tail) = rest.split_at_mut(frames * 2);
                feed.fill(callback, 2);
                rest = tail;
            }
        });
        let expected: Vec<f32> = (0..total).flat_map(|f| [f as f32, -(f as f32)]).collect();
        assert_eq!(out, expected);
    }

    #[test]
    fn extra_channels_are_silent_and_integers_are_full_scale() {
        let mut out = [7i16; 3 * 4];
        interleave(&[1.0, -1.0, 0.5], &[2.0, 0.0, -0.5], &mut out, 4, 0);
        assert_eq!(out, [32767, 32767, 0, 0, -32767, 0, 0, 0, 16383, -16383, 0, 0]);

        // The second of two mono buffers is the right channel
        let mut right = [0i32; 2];
        interleave(&[0.0, 0.0], &[1.0, -1.0], &mut right, 1, 1);
        assert_eq!(right, [i32::MAX, i32::MIN]);

        // A buffer with no channels is skipped, never a panic on the audio thread
        interleave::<f32>(&[1.0], &[1.0], &mut [], 0, 0);
    }

    #[test]
    fn a_panicking_render_is_silence_and_a_failed_stream() {
        let stats = Arc::new(Stats::new());
        let mut calls = 0;
        let render: Render = Box::new(move |out: Stereo<'_>| {
            calls += 1;
            assert!(calls < 2, "render torn");
            out.left.fill(1.0);
            out.right.fill(1.0);
        });
        let mut feed = Feed::new(render, Arc::clone(&stats));
        let mut out = vec![0.5f32; 3 * BLOCK * 2];
        feed.fill(&mut out, 2);
        assert!(out[..BLOCK * 2].iter().all(|s| *s == 1.0));
        assert!(out[BLOCK * 2..].iter().all(|s| *s == 0.0));
        assert!(stats.report().failed);
    }
}
