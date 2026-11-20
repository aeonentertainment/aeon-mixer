use kernels::fixtures::{BUZZ, harmonic_voice};

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

    /// The `k`th phrase: the fixtures' sung vowel at `hz`.
    pub fn sung(&self, k: usize) -> Vec<f32> {
        harmonic_voice(self.length(k), &BUZZ, |_| self.hz)
    }
}
