//! The return branch's crossing (docs/design.md 8.7): the master pushes every block it renders as
//! interleaved stereo frames, and the network thread pops them in whole Opus frames. A ring of
//! frames rather than of blocks, because 480 is not a multiple of 128.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::block::Stereo;

/// 50 ms: five return packets, ten network ticks. Enough that a late tick never costs a block,
/// short enough that a stalled network thread sends stale audio for no longer than that.
pub const RETURN_CAPACITY_FRAMES: usize = 2400;

pub fn return_ring() -> (ReturnProducer, ReturnConsumer) {
    let (producer, consumer) = rtrb::RingBuffer::new(RETURN_CAPACITY_FRAMES);
    let dropped = Arc::new(AtomicU32::new(0));
    (
        ReturnProducer { ring: producer, dropped: Arc::clone(&dropped) },
        ReturnConsumer { ring: consumer, dropped },
    )
}

pub struct ReturnProducer {
    ring: rtrb::Producer<[f32; 2]>,
    dropped: Arc<AtomicU32>,
}

impl ReturnProducer {
    /// The whole block or none of it: a full ring drops the block and counts it, so the
    /// consumer never sees a block cut in half.
    pub fn push(&mut self, block: &Stereo<'_>) {
        match self.ring.write_chunk_uninit(block.left.len()) {
            Ok(chunk) => {
                chunk.fill_from_iter(
                    block.left.iter().zip(block.right.iter()).map(|(l, r)| [*l, *r]),
                );
            }
            Err(_) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

pub struct ReturnConsumer {
    ring: rtrb::Consumer<[f32; 2]>,
    dropped: Arc<AtomicU32>,
}

impl ReturnConsumer {
    /// Fills `out` and returns true when the ring holds that many frames; otherwise takes
    /// nothing, so the frames wait for the rest of the packet.
    pub fn pop_exact(&mut self, out: &mut [[f32; 2]]) -> bool {
        self.ring.pop_entire_slice(out).is_ok()
    }

    /// Blocks the audio thread dropped because the ring was full.
    pub fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The drop count for the control thread, which keeps it after the consumer moves to the
    /// network thread.
    pub fn drops(&self) -> ReturnDrops {
        ReturnDrops(Arc::clone(&self.dropped))
    }
}

/// The control thread's read of the ring's drop count, for `Mixer.stats`.
pub struct ReturnDrops(Arc<AtomicU32>);

impl ReturnDrops {
    pub fn read(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::block::BLOCK;

    fn push_block(producer: &mut ReturnProducer, value: f32) {
        let mut left = [value; BLOCK];
        let mut right = [-value; BLOCK];
        let block = Stereo { left: &mut left, right: &mut right };
        assert_no_alloc(|| producer.push(&block));
    }

    #[test]
    fn blocks_come_out_as_interleaved_frames_in_any_count() {
        let (mut producer, mut consumer) = return_ring();
        push_block(&mut producer, 1.0);
        push_block(&mut producer, 2.0);
        let mut out = [[0.0; 2]; BLOCK + 3];
        assert!(consumer.pop_exact(&mut out));
        assert_eq!(out[BLOCK - 1], [1.0, -1.0]);
        assert_eq!(out[BLOCK], [2.0, -2.0], "the next block follows on");
        assert!(!consumer.pop_exact(&mut out), "fewer frames left than asked for");
        assert!(consumer.pop_exact(&mut out[..BLOCK - 3]), "and what was refused is still there");
    }

    #[test]
    fn a_full_ring_drops_the_whole_block_and_counts_it() {
        let (mut producer, mut consumer) = return_ring();
        let fits = RETURN_CAPACITY_FRAMES / BLOCK;
        for _ in 0..fits {
            push_block(&mut producer, 1.0);
        }
        push_block(&mut producer, 2.0);
        push_block(&mut producer, 3.0);
        assert_eq!(consumer.dropped(), 2);
        assert_eq!(consumer.drops().read(), 2, "and the control thread reads the same count");

        let mut out = vec![[0.0; 2]; fits * BLOCK];
        assert!(consumer.pop_exact(&mut out));
        assert!(out.iter().all(|frame| *frame == [1.0, -1.0]), "no part of a dropped block");
        assert!(!consumer.pop_exact(&mut out[..1]), "nothing after the blocks that fit");
    }
}
