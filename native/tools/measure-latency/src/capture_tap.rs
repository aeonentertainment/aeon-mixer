use std::time::Instant;

use rtrb::{Producer, RingBuffer};

use crate::clock_line::Wake;
use crate::recording::Recording;
use crate::tap::push_all;

/// Keeps what an input delivers and notes how far it had got at each wake. Lives in the input's
/// sink, on its capture thread.
pub struct CaptureTap {
    samples: Producer<f32>,
    wakes: Producer<Wake>,
    frame: u64,
    epoch: Instant,
}

impl CaptureTap {
    /// A tap with room for `frames`, and the recording it fills.
    pub fn new(frames: usize, epoch: Instant) -> (Self, Recording) {
        let (samples, sample_out) = RingBuffer::new(frames);
        // A wake a frame at the worst, on an input that delivers in very small pieces
        let (wakes, wake_out) = RingBuffer::new(frames);
        let tap = Self { samples, wakes, frame: 0, epoch };
        (tap, Recording { samples: sample_out, wakes: wake_out })
    }

    pub fn deliver(&mut self, samples: &[f32], at: Instant) {
        self.frame += samples.len() as u64;
        let at_ns = at.duration_since(self.epoch).as_nanos() as u64;
        let _ = self.wakes.push(Wake { frame: self.frame, at_ns });
        push_all(&mut self.samples, samples);
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    /// It sits on a realtime thread: it keeps every delivery in order and allocates nothing.
    #[test]
    fn it_keeps_what_an_input_delivers_and_allocates_nothing() {
        let (mut tap, mut recording) = CaptureTap::new(16, Instant::now());
        let at = Instant::now();
        assert_no_alloc(|| {
            tap.deliver(&[1.0, 2.0, 3.0], at);
            tap.deliver(&[4.0], at);
        });
        let (kept, wakes) = recording.take();
        assert_eq!(kept, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(wakes.iter().map(|wake| wake.frame).collect::<Vec<_>>(), [3, 4]);
    }
}
