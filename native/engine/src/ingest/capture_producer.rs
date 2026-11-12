//! The capture thread's end of a `capture` slot's ring: what a device's sink calls with each
//! delivery (docs/design.md 8.4).

/// Pushes one channel of f32 into the ring a `CaptureVoice` reads. Built by `capture_voice`.
pub struct CaptureProducer {
    pub(super) ring: rtrb::Producer<f32>,
}

impl CaptureProducer {
    /// Takes what fits and returns how many samples that was. Never blocks and allocates nothing,
    /// so a realtime capture thread may call it. A full ring means the slot has not pulled for as
    /// long as the ring holds, so what does not fit is dropped.
    pub fn push(&mut self, samples: &[f32]) -> usize {
        let count = samples.len().min(self.ring.slots());
        let Ok(mut chunk) = self.ring.write_chunk(count) else {
            return 0;
        };
        let (first, second) = chunk.as_mut_slices();
        let (head, tail) = samples[..count].split_at(first.len());
        first.copy_from_slice(head);
        second.copy_from_slice(tail);
        chunk.commit_all();
        count
    }
}
