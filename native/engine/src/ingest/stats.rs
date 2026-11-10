//! What a slot's ingest reports: counters the jitter buffer keeps on the audio thread and
//! publishes after each block through atomics, so `stats()` on the control thread never locks.

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SlotStats {
    /// Packets taken from the ring.
    pub pushed: u32,
    /// Packets that never came, known by a later one arriving first. A sender's pause, its
    /// sequence number held while its timestamp runs on, is no loss.
    pub lost: u32,
    /// Packets that arrived after their samples fell due, and were dropped.
    pub late: u32,
    /// Frames the decoder filled in for a missing packet, a pause or a skew correction.
    pub concealed: u32,
    /// Anchor moves: a new stream, or a one-frame skew correction.
    pub resyncs: u32,
    /// RFC 3550 interarrival jitter, in milliseconds.
    pub jitter_ms: f32,
}

/// The crossing: the audio thread stores a snapshot, the control thread loads one.
#[derive(Default)]
pub struct SharedStats {
    pushed: AtomicU32,
    lost: AtomicU32,
    late: AtomicU32,
    concealed: AtomicU32,
    resyncs: AtomicU32,
    jitter_bits: AtomicU32,
}

impl SharedStats {
    pub fn publish(&self, stats: &SlotStats) {
        self.pushed.store(stats.pushed, Ordering::Relaxed);
        self.lost.store(stats.lost, Ordering::Relaxed);
        self.late.store(stats.late, Ordering::Relaxed);
        self.concealed.store(stats.concealed, Ordering::Relaxed);
        self.resyncs.store(stats.resyncs, Ordering::Relaxed);
        self.jitter_bits.store(stats.jitter_ms.to_bits(), Ordering::Relaxed);
    }

    pub fn read(&self) -> SlotStats {
        SlotStats {
            pushed: self.pushed.load(Ordering::Relaxed),
            lost: self.lost.load(Ordering::Relaxed),
            late: self.late.load(Ordering::Relaxed),
            concealed: self.concealed.load(Ordering::Relaxed),
            resyncs: self.resyncs.load(Ordering::Relaxed),
            jitter_ms: f32::from_bits(self.jitter_bits.load(Ordering::Relaxed)),
        }
    }
}
