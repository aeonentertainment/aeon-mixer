use std::f64::consts::TAU;

use engine::block::RATE;

/// A sung vowel ahead of each chirp, each phrase `step` longer than the last, then a rest. The
/// corrector moves its delay through a phrase, and early in the rest, silent here, moves it down
/// to a quarter period above its floor unless it sits lower, then holds it, so the chirp goes
/// through a delay that stands still.
#[derive(Clone, Copy, Debug)]
pub struct Phrases {
    pub hz: f64,
    pub shortest: usize,
    pub step: usize,
    pub rest: usize,
}

impl Phrases {
    /// The `k`th phrase's length in frames.
    pub fn length(&self, k: usize) -> usize {
        self.shortest + k * self.step
    }

    /// Twelve harmonics at `hz`, the buzz of a sung vowel, `frame` into a phrase.
    pub fn at(&self, frame: usize) -> f32 {
        let t = frame as f64 / f64::from(RATE);
        let sample: f64 = (1..=12).map(|h| (TAU * self.hz * h as f64 * t).sin() / h as f64).sum();
        (0.25 * sample) as f32
    }
}
