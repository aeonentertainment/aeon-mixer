use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Instant;

use crate::capture::{Captured, Sink};
use crate::sample::Sample;
use crate::stats::Stats;

/// Frames handed to the sink at once. A delivery longer than this arrives in pieces with one `at`.
const CHUNK: usize = 2048;

/// Takes one channel of whatever a device delivers and hands it to the sink as f32. A backend owns
/// one on its capture thread. Allocates nothing after `new`.
pub struct CaptureFeed {
    sink: Sink,
    mono: Vec<f32>,
    stats: Arc<Stats>,
    faulted: bool,
}

impl CaptureFeed {
    pub fn new(sink: Sink, stats: Arc<Stats>) -> Self {
        Self { sink, mono: vec![0.0; CHUNK], stats, faulted: false }
    }

    /// Hands over `channel` of an interleaved buffer that carries `channels`.
    pub fn deliver<S: Sample>(
        &mut self,
        samples: &[S],
        channels: usize,
        channel: usize,
        at: Instant,
    ) {
        if channels == 0 {
            return;
        }
        for frames in samples.chunks(CHUNK * channels) {
            let count = frames.len() / channels;
            let picked = frames.iter().skip(channel).step_by(channels);
            for (out, sample) in self.mono.iter_mut().zip(picked) {
                *out = sample.to_f32();
            }
            self.hand_over(count, at);
        }
    }

    /// `frames` of silence, for a device that marks a delivery silent instead of filling it.
    pub fn deliver_silence(&mut self, frames: usize, at: Instant) {
        let mut left = frames;
        while left > 0 {
            let count = left.min(CHUNK);
            self.mono[..count].fill(0.0);
            self.hand_over(count, at);
            left -= count;
        }
    }

    // A panic in the sink must not unwind into the OS's thread. The stream is marked failed and the
    // sink never called again: its state may be torn.
    fn hand_over(&mut self, count: usize, at: Instant) {
        if self.faulted {
            return;
        }
        let Self { sink, mono, .. } = self;
        let samples = &mono[..count];
        if catch_unwind(AssertUnwindSafe(|| sink(Captured { samples, at }))).is_err() {
            self.faulted = true;
            self.stats.fail();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use assert_no_alloc::assert_no_alloc;

    use super::*;

    /// Runs a feed over `deliver` and returns what its sink saw, in order.
    fn collect(deliver: impl FnOnce(&mut CaptureFeed)) -> Vec<f32> {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink: Sink = Box::new({
            let seen = Arc::clone(&seen);
            move |captured: Captured<'_>| seen.lock().unwrap().extend_from_slice(captured.samples)
        });
        let mut feed = CaptureFeed::new(sink, Arc::new(Stats::new()));
        deliver(&mut feed);
        drop(feed);
        Arc::into_inner(seen).unwrap().into_inner().unwrap()
    }

    #[test]
    fn the_channel_asked_for_comes_out_as_f32_in_any_format() {
        let at = Instant::now();
        let seen = collect(|feed| feed.deliver(&[0.1f32, 0.2, 0.3, 0.4, 0.5, 0.6], 3, 1, at));
        assert_eq!(seen, [0.2, 0.5]);
        let seen = collect(|feed| feed.deliver(&[i16::MIN, 16384, 0, -16384], 2, 1, at));
        assert_eq!(seen, [0.5, -0.5]);
        let seen = collect(|feed| feed.deliver(&[i32::MAX, 0, i32::MIN, 0], 2, 0, at));
        assert_eq!(seen, [1.0, -1.0]);
    }

    /// A delivery longer than the feed's buffer arrives whole, in order, and with no allocation.
    #[test]
    fn a_long_delivery_arrives_whole_without_allocating() {
        let frames: Vec<f32> = (0..3 * CHUNK + 5).flat_map(|f| [f as f32, -1.0]).collect();
        let seen = collect(|feed| feed.deliver(&frames, 2, 0, Instant::now()));
        let expected: Vec<f32> = (0..3 * CHUNK + 5).map(|f| f as f32).collect();
        assert_eq!(seen, expected);

        let mut feed = CaptureFeed::new(Box::new(|_| {}), Arc::new(Stats::new()));
        let at = Instant::now();
        assert_no_alloc(|| {
            feed.deliver(&frames, 2, 0, at);
            feed.deliver_silence(3 * CHUNK, at);
        });
    }

    /// Every backend refuses a channel past the device's when it opens, so `createMixer` rejects;
    /// were one to reach the feed, it would hand over silence, never another channel's audio.
    #[test]
    fn a_channel_past_the_devices_is_silence() {
        let at = Instant::now();
        let seen = collect(|feed| feed.deliver(&[0.1f32, 0.2, 0.3, 0.4], 2, 5, at));
        assert_eq!(seen, [0.0, 0.0]);
        let mut feed = CaptureFeed::new(Box::new(|_| {}), Arc::new(Stats::new()));
        assert_no_alloc(|| feed.deliver(&[0.1f32, 0.2, 0.3, 0.4], 2, 5, at));
    }

    #[test]
    fn a_panicking_sink_fails_the_stream_and_is_never_called_again() {
        let stats = Arc::new(Stats::new());
        let mut calls = 0;
        let sink: Sink = Box::new(move |_| {
            calls += 1;
            assert!(calls < 2, "sink torn");
        });
        let mut feed = CaptureFeed::new(sink, Arc::clone(&stats));
        let at = Instant::now();
        feed.deliver(&[0.0f32], 1, 0, at);
        assert!(!stats.report().failed);
        feed.deliver(&[0.0f32], 1, 0, at);
        feed.deliver(&[0.0f32], 1, 0, at);
        assert!(stats.report().failed);
    }
}
