use std::f64::consts::TAU;

use engine::block::RATE;

use crate::chirp::Chirp;

/// What one measurement plays: silence for the stream to settle, then chirps with room between
/// them for the path's delay and the room's decay. An arrival is searched for only inside its own
/// chirp's interval, so a path slower than the interval is refused rather than misread.
#[derive(Clone, Copy, Debug)]
pub struct Programme {
    pub chirp: Chirp,
    /// What is sung ahead of each chirp, for a row with the corrector engaged.
    pub phrases: Option<Phrases>,
    pub preroll: usize,
    pub gap: usize,
    pub repeats: usize,
}

impl Programme {
    /// No room and no noise, so two chirps with a short gap say all there is to say.
    pub fn offline() -> Self {
        Self {
            chirp: Chirp::voice_band(0.5),
            phrases: None,
            preroll: RATE as usize * 3 / 10,
            gap: RATE as usize / 10,
            repeats: 2,
        }
    }

    /// The same, after the silence a slot that chooses its delay needs to choose it.
    pub fn offline_settled() -> Self {
        Self { preroll: RATE as usize * 11, ..Self::offline() }
    }

    /// A sung vowel at `hz` ahead of each chirp, for the corrector to act on, and the short chirp
    /// it leaves alone. Twelve phrases, each 25 ms longer than the last, leave its delay at as
    /// many heights of its sawtooth.
    pub fn corrected(hz: f64) -> Self {
        let phrases = Phrases {
            hz,
            shortest: RATE as usize / 5,
            step: RATE as usize / 40,
            rest: RATE as usize / 5,
        };
        Self { chirp: Chirp::short(0.5), phrases: Some(phrases), repeats: 12, ..Self::offline() }
    }

    /// A room rings and a device can hide a lot, so the gap is long and the chirps many, which
    /// shows how much the figure moves from one to the next.
    pub fn device(level: f32) -> Self {
        Self {
            chirp: Chirp::voice_band(level),
            phrases: None,
            preroll: RATE as usize * 4 / 10,
            gap: RATE as usize * 4 / 10,
            repeats: 8,
        }
    }

    /// Frames from one chirp's start to the next, and the most delay a path may have.
    pub fn interval(&self) -> usize {
        self.chirp.frames + self.gap
    }

    /// Where each chirp starts in `signal`.
    pub fn starts(&self) -> impl Iterator<Item = usize> + use<> {
        let (interval, phrases) = (self.interval(), self.phrases);
        let lead = move |k| phrases.map_or(0, |phrases| phrases.length(k) + phrases.rest);
        (0..self.repeats).scan(self.preroll, move |next, k| {
            let start = *next + lead(k);
            *next = start + interval;
            Some(start)
        })
    }

    /// The whole signal, with an interval of silence after the last chirp for its arrival.
    pub fn signal(&self) -> Vec<f32> {
        let chirp = self.chirp.samples();
        let mut signal = vec![0.0; self.frames()];
        for (k, start) in self.starts().enumerate() {
            if let Some(phrases) = self.phrases {
                let end = start - phrases.rest;
                let phrase = &mut signal[end - phrases.length(k)..end];
                for (frame, sample) in phrase.iter_mut().enumerate() {
                    *sample = phrases.at(frame);
                }
            }
            signal[start..start + chirp.len()].copy_from_slice(&chirp);
        }
        signal
    }

    pub fn frames(&self) -> usize {
        self.starts().last().unwrap_or(self.preroll) + 2 * self.interval()
    }
}

/// A sung vowel ahead of each chirp, each phrase `step` longer than the last, then a rest. The
/// corrector moves its delay through a phrase and holds it through the rest, so the chirp goes
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
