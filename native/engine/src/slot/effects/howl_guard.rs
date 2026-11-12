use crate::block::RATE;
use crate::params::HowlGuardParams;

/// The slot's howl guard. Off it does no work and touches nothing, and turned off it lifts any
/// duck at once.
pub struct HowlGuard {
    kernel: kernels::HowlGuard,
    on: bool,
}

impl HowlGuard {
    pub fn new() -> Self {
        Self { kernel: kernels::HowlGuard::new(RATE), on: false }
    }

    /// One block of the slot's voice, in place.
    pub fn process(&mut self, params: &HowlGuardParams, block: &mut [f32]) {
        if !params.enabled {
            if self.on {
                self.kernel.reset();
                self.on = false;
            }
            return;
        }
        self.on = true;
        self.kernel.process(block);
    }

    /// Ducks since the slot was built.
    pub fn catches(&self) -> u32 {
        self.kernel.catches()
    }

    pub fn reset(&mut self) {
        self.kernel.reset();
    }
}

impl Default for HowlGuard {
    fn default() -> Self {
        Self::new()
    }
}
