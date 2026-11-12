use engine::block::RATE;

use crate::chirp::Chirp;

/// What one measurement plays: silence for the stream to settle, then chirps with room between
/// them for the path's delay and the room's decay. An arrival is searched for only inside its own
/// chirp's interval, so a path slower than the interval is refused rather than misread.
#[derive(Clone, Copy, Debug)]
pub struct Programme {
    pub chirp: Chirp,
    pub preroll: usize,
    pub gap: usize,
    pub repeats: usize,
}

impl Programme {
    /// No room and no noise, so two chirps with a short gap say all there is to say.
    pub fn offline() -> Self {
        Self {
            chirp: Chirp::voice_band(0.5),
            preroll: RATE as usize * 3 / 10,
            gap: RATE as usize / 10,
            repeats: 2,
        }
    }

    /// Frames from one chirp's start to the next, and the most delay a path may have.
    pub fn interval(&self) -> usize {
        self.chirp.frames + self.gap
    }

    /// Where each chirp starts in `signal`.
    pub fn starts(&self) -> impl Iterator<Item = usize> + use<> {
        let (preroll, interval) = (self.preroll, self.interval());
        (0..self.repeats).map(move |k| preroll + k * interval)
    }

    /// The whole signal, with an interval of silence after the last chirp for its arrival.
    pub fn signal(&self) -> Vec<f32> {
        let chirp = self.chirp.samples();
        let mut signal = vec![0.0; self.frames()];
        for start in self.starts() {
            signal[start..start + chirp.len()].copy_from_slice(&chirp);
        }
        signal
    }

    pub fn frames(&self) -> usize {
        self.preroll + (self.repeats + 1) * self.interval()
    }
}
