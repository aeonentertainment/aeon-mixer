use kernels::{EchoFindings, EchoHistory, SourceDelay};

use crate::block::RATE;
use crate::params::EchoCancelParams;

/// The slot's echo canceller: the master the room was played, taken back out of what the slot
/// hears. Off it does no work and touches nothing; turned off it forgets the path, so turning it
/// on again finds it afresh.
pub struct EchoCanceller {
    kernel: kernels::EchoCanceller,
    on: bool,
}

impl EchoCanceller {
    pub fn new() -> Self {
        Self { kernel: kernels::EchoCanceller::new(RATE), on: false }
    }

    /// One block of the slot's input, in place. `history` is the master up to the block before
    /// and `own` this slot's share of it; `source` is where the source has its delay, which the
    /// canceller follows as it moves.
    pub fn process(
        &mut self,
        params: &EchoCancelParams,
        history: &EchoHistory,
        own: &EchoHistory,
        source: SourceDelay,
        block: &mut [f32],
    ) {
        if !params.enabled {
            if self.on {
                self.kernel.reset();
                self.on = false;
            }
            return;
        }
        self.on = true;
        self.kernel.process(history, own, source, block);
    }

    pub fn findings(&self) -> Option<EchoFindings> {
        self.kernel.findings()
    }

    /// A new sender: a new path.
    pub fn reset(&mut self) {
        self.kernel.reset();
    }
}

impl Default for EchoCanceller {
    fn default() -> Self {
        Self::new()
    }
}
