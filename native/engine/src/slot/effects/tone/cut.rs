use crate::slot::effects::tone::follower::Follower;

const RISE_MS: f32 = 1.0;
const FALL_MS: f32 = 50.0;
/// A cut this shallow is at rest, out of the path: a step of a hundredth of a percent.
const REST_DB: f32 = 1e-3;

/// How many dB a moving shelf takes off: in over a millisecond, out over 50.
pub struct Cut {
    follower: Follower,
    db: f32,
}

impl Cut {
    pub fn new() -> Self {
        Self { follower: Follower::new(RISE_MS, FALL_MS), db: 0.0 }
    }

    pub fn follow(&mut self, toward_db: f32) -> f32 {
        self.db = self.follower.follow(toward_db);
        self.db
    }

    pub fn db(&self) -> f32 {
        self.db
    }

    pub fn at_rest(&self) -> bool {
        self.db <= REST_DB
    }
}

impl Default for Cut {
    fn default() -> Self {
        Self::new()
    }
}
