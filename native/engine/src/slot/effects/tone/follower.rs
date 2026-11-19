use crate::slot::effects::level::one_pole;

/// A one-pole that rises at one rate and falls at another: a detector's level, or a cut that
/// comes in fast and lets go slowly.
pub struct Follower {
    value: f32,
    rise: f32,
    fall: f32,
}

impl Follower {
    pub fn new(rise_ms: f32, fall_ms: f32) -> Self {
        Self { value: 0.0, rise: one_pole(rise_ms), fall: one_pole(fall_ms) }
    }

    pub fn follow(&mut self, toward: f32) -> f32 {
        let rate = if toward > self.value { self.rise } else { self.fall };
        self.value += rate * (toward - self.value);
        self.value
    }
}
