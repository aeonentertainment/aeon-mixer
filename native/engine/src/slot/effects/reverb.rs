use crate::block::RATE;
use crate::params::ReverbParams;

/// The slot's reverb stage: Freeverb on the mono voice, its mix ramped across each block so a
/// knob never clicks. Off costs nothing.
pub struct Reverb {
    kernel: kernels::Reverb,
}

impl Reverb {
    pub fn new() -> Self {
        Self { kernel: kernels::Reverb::new(RATE) }
    }

    /// One run of the slot's voice, in place.
    pub fn process(&mut self, params: &ReverbParams, block: &mut [f32]) {
        let mix = if params.enabled { params.mix } else { 0.0 };
        self.kernel.set(params.room_size, mix);
        self.kernel.process(block);
    }
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::block::BLOCK;

    #[test]
    fn off_is_the_voice_itself_and_on_adds_a_tail() {
        let mut reverb = Reverb::new();
        let mut params = ReverbParams { enabled: false, room_size: 0.8, mix: 0.5 };
        let mut block = [0.0f32; BLOCK];
        block[0] = 1.0;
        let input = block;
        assert_no_alloc(|| reverb.process(&params, &mut block));
        assert_eq!(block, input);

        params.enabled = true;
        // One block to ramp in, then an impulse, then the tail it leaves after the dry sample
        assert_no_alloc(|| reverb.process(&params, &mut [0.0; BLOCK]));
        let mut tail = [0.0f32; 20 * BLOCK];
        tail[0] = 1.0;
        assert_no_alloc(|| tail.chunks_mut(BLOCK).for_each(|block| reverb.process(&params, block)));
        assert!(tail[1..].iter().any(|s| s.abs() > 1e-3), "reverb on left no tail");
    }
}
