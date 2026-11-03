/// A gain that glides in a straight line to its target over a fixed number of frames, one sample
/// at a time. Until its first `aim` it has no value, and that first aim lands at once, so a stem
/// plays at its gain from the first block instead of fading up from silence.
pub struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    frames: f32,
    started: bool,
}

impl Ramp {
    pub fn new(frames: u32) -> Self {
        Self { value: 0.0, target: 0.0, step: 0.0, frames: frames as f32, started: false }
    }

    /// Glides to `target` from wherever the gain is now.
    pub fn aim(&mut self, target: f32) {
        if !self.started {
            self.jump(target);
        } else if target != self.target {
            self.target = target;
            self.step = (target - self.value) / self.frames;
        }
    }

    /// Lands on `value` with no glide.
    pub fn jump(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.step = 0.0;
        self.started = true;
    }

    /// The gain for the next sample.
    pub fn next(&mut self) -> f32 {
        if self.value != self.target {
            self.value += self.step;
            let overshot =
                if self.step > 0.0 { self.value > self.target } else { self.value < self.target };
            if overshot {
                self.value = self.target;
            }
        }
        self.value
    }

    /// Whether the gain has arrived at `value`.
    pub fn rests_at(&self, value: f32) -> bool {
        self.value == value && self.target == value
    }
}
