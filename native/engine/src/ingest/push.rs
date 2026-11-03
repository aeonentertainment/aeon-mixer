//! A `push` slot's source: stereo f32 the control thread hands in through an SPSC ring, folded
//! to mono on the way out. Silence when the ring is empty. What the ring holds is the slot's
//! latency, so a pull that finds more than the depth drops the oldest down to it and counts a
//! resync (docs/design.md 8.4).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::ingest::stats::{SharedStats, SlotStats};
use crate::slot::source::Source;

/// Frames the ring holds unless the caller says otherwise: 100 ms, so a host feeding it once
/// per event-loop turn never runs it dry.
pub const DEFAULT_CAPACITY_FRAMES: usize = 4800;

/// A ring whose depth is its capacity: it never drops the oldest, so a voice handed a whole sound
/// plays all of it.
pub fn push_voice(capacity_frames: usize) -> (PushProducer, PushVoice) {
    push_voice_with_depth(capacity_frames, capacity_frames)
}

/// A ring played at most `depth_frames` behind the newest frame it holds.
pub fn push_voice_with_depth(
    capacity_frames: usize,
    depth_frames: usize,
) -> (PushProducer, PushVoice) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity_frames);
    let stats = Arc::new(SharedStats::default());
    (
        PushProducer { ring: producer, stats: Arc::clone(&stats) },
        PushVoice { ring: consumer, depth: depth_frames, resyncs: AtomicU32::new(0), stats },
    )
}

pub struct PushProducer {
    ring: rtrb::Producer<[f32; 2]>,
    stats: Arc<SharedStats>,
}

impl PushProducer {
    /// Takes interleaved stereo and returns the frames that fit. Never blocks: what does not fit
    /// is the caller's to hand in again.
    pub fn push(&mut self, interleaved: &[f32]) -> usize {
        let (frames, _) = interleaved.as_chunks::<2>();
        frames.iter().take_while(|frame| self.ring.push(**frame).is_ok()).count()
    }

    /// What the slot reports: only `resyncs`, the times the voice dropped its oldest frames,
    /// means anything for a push slot.
    pub fn stats(&self) -> SlotStats {
        self.stats.read()
    }
}

pub struct PushVoice {
    ring: rtrb::Consumer<[f32; 2]>,
    depth: usize,
    resyncs: AtomicU32,
    stats: Arc<SharedStats>,
}

impl Source for PushVoice {
    fn pull(&mut self, out: &mut [f32]) {
        let excess = self.ring.slots().saturating_sub(self.depth);
        if excess > 0 {
            if let Ok(chunk) = self.ring.read_chunk(excess) {
                chunk.commit_all();
            }
            let resyncs = self.resyncs.fetch_add(1, Ordering::Relaxed) + 1;
            self.stats.publish(&SlotStats { resyncs, ..SlotStats::default() });
        }
        for sample in out {
            *sample = match self.ring.pop() {
                Ok([left, right]) => (left + right) * 0.5,
                Err(_) => 0.0,
            };
        }
    }

    /// What the ring holds is how long ago the host captured what `pull` hands over next.
    fn latency_frames(&self) -> u32 {
        u32::try_from(self.ring.slots()).unwrap_or(u32::MAX)
    }

    /// `ResetSlot`: whatever the host had handed in belongs to the singer before.
    fn reset(&mut self) {
        if let Ok(chunk) = self.ring.read_chunk(self.ring.slots()) {
            chunk.commit_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    #[test]
    fn stereo_in_folds_to_mono_out_and_silence_follows() {
        let (mut producer, mut voice) = push_voice(4);
        assert_eq!(producer.push(&[1.0, 0.0, 0.5, 0.5, -1.0, 1.0, 0.2, 0.2, 9.0, 9.0]), 4);
        let mut out = [7.0; 6];
        assert_no_alloc(|| voice.pull(&mut out));
        assert_eq!(out, [0.5, 0.5, 0.0, 0.2, 0.0, 0.0]);
    }

    #[test]
    fn a_pull_past_the_depth_drops_the_oldest_and_counts_a_resync() {
        let (mut producer, mut voice) = push_voice_with_depth(16, 4);
        producer.push(&[1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 5.0, 5.0, 6.0, 6.0, 7.0, 7.0]);
        let mut out = [0.0; 3];
        assert_no_alloc(|| voice.pull(&mut out));
        assert_eq!(out, [4.0, 5.0, 6.0], "the newest four frames were kept, three of them played");
        assert_eq!(producer.stats().resyncs, 1);
        assert_no_alloc(|| voice.pull(&mut out));
        assert_eq!(out, [7.0, 0.0, 0.0]);
        assert_eq!(producer.stats().resyncs, 1, "within the depth nothing is dropped");
    }

    #[test]
    fn its_latency_is_what_waits_in_the_ring() {
        let (mut producer, mut voice) = push_voice(16);
        assert_eq!(voice.latency_frames(), 0);
        producer.push(&[0.0; 20]);
        voice.pull(&mut [0.0; 4]);
        assert_eq!(voice.latency_frames(), 6, "ten handed in, four played");
    }

    #[test]
    fn reset_forgets_what_the_host_pushed() {
        let (mut producer, mut voice) = push_voice(4);
        producer.push(&[1.0, 1.0, 1.0, 1.0]);
        let mut out = [1.0; 2];
        assert_no_alloc(|| {
            voice.reset();
            voice.pull(&mut out);
        });
        assert_eq!(out, [0.0, 0.0]);
    }
}
