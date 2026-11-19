//! What a slot's ingest reports: counters the jitter buffer keeps on the audio thread and
//! publishes after each block through atomics, so `stats()` on the control thread never locks.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SlotStats {
    /// Packets taken from the ring.
    pub pushed: u32,
    /// Packets that never came, known by a later one arriving first. A sender's pause, its
    /// sequence number held while its timestamp runs on, is no loss.
    pub lost: u32,
    /// Packets that arrived after their samples fell due, and were dropped.
    pub late: u32,
    /// Of those, the ones that would have been late at the delay the stream opened at: the same
    /// song on the same network with the delay held, compared in one run. Equal to `late` when
    /// the delay has not moved.
    pub late_at_opening: u32,
    /// Frames the decoder filled in for a missing packet, a pause or a skew correction.
    pub concealed: u32,
    /// Anchor moves: a new stream, a move of the playout delay, or a skew correction.
    pub resyncs: u32,
    /// RFC 3550 interarrival jitter, in milliseconds.
    pub jitter_ms: f32,
    /// The delay the stream's earliest packets wait now, in milliseconds. Zero for a `push` slot,
    /// which has no jitter buffer. For a `capture` slot, how long its mic's samples wait from the
    /// input to the pull, the input's period included.
    pub playout_delay_ms: f32,
}

/// The crossing: the audio thread stores a snapshot, the control thread loads one.
#[derive(Default)]
pub struct SharedStats {
    pushed: AtomicU32,
    lost: AtomicU32,
    late: AtomicU32,
    late_at_opening: AtomicU32,
    concealed: AtomicU32,
    resyncs: AtomicU32,
    jitter_bits: AtomicU32,
    delay_bits: AtomicU32,
    /// Set the first time a capture slot plays its input rather than the silence it plays while
    /// it opens, primes and settles. It stays set.
    played: AtomicBool,
    /// How many times a capture slot has judged its input stopped.
    stops: AtomicU32,
}

impl SharedStats {
    pub fn publish(&self, stats: &SlotStats) {
        self.pushed.store(stats.pushed, Ordering::Relaxed);
        self.lost.store(stats.lost, Ordering::Relaxed);
        self.late.store(stats.late, Ordering::Relaxed);
        self.late_at_opening.store(stats.late_at_opening, Ordering::Relaxed);
        self.concealed.store(stats.concealed, Ordering::Relaxed);
        self.resyncs.store(stats.resyncs, Ordering::Relaxed);
        self.jitter_bits.store(stats.jitter_ms.to_bits(), Ordering::Relaxed);
        self.delay_bits.store(stats.playout_delay_ms.to_bits(), Ordering::Relaxed);
    }

    /// A capture slot calls this where it plays its input. One store: no allocation, no lock.
    pub fn set_played(&self) {
        self.played.store(true, Ordering::Relaxed);
    }

    /// A capture slot calls this once for each stop it judges. One add: no allocation, no lock.
    pub fn add_stop(&self) {
        self.stops.fetch_add(1, Ordering::Relaxed);
    }

    /// Whether the slot has played its input yet. Not in `stats()`: a tool that must not measure
    /// the silence a capture slot opens with waits for it (design.md section 10).
    pub fn played(&self) -> bool {
        self.played.load(Ordering::Relaxed)
    }

    /// The stops the slot has judged, each as it judged it. Not in `stats()`.
    pub fn stops(&self) -> u32 {
        self.stops.load(Ordering::Relaxed)
    }

    pub fn read(&self) -> SlotStats {
        SlotStats {
            pushed: self.pushed.load(Ordering::Relaxed),
            lost: self.lost.load(Ordering::Relaxed),
            late: self.late.load(Ordering::Relaxed),
            late_at_opening: self.late_at_opening.load(Ordering::Relaxed),
            concealed: self.concealed.load(Ordering::Relaxed),
            resyncs: self.resyncs.load(Ordering::Relaxed),
            jitter_ms: f32::from_bits(self.jitter_bits.load(Ordering::Relaxed)),
            playout_delay_ms: f32::from_bits(self.delay_bits.load(Ordering::Relaxed)),
        }
    }
}
