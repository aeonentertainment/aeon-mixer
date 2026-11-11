use std::sync::atomic::{AtomicU32, Ordering};

/// How far a slot's corrector holds its voice, published by the audio thread once a block for
/// the control thread's `stats()`. A sawtooth while the voice is corrected and frozen through a
/// rest, so one read is a sample of it: docs/design.md section 3 says what a host may do with it.
#[derive(Debug, Default)]
pub struct CorrectorDelay(AtomicU32);

impl CorrectorDelay {
    pub fn publish(&self, frames: usize) {
        self.0.store(frames as u32, Ordering::Relaxed);
    }

    pub fn frames(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
}
