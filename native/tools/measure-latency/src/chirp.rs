use std::f64::consts::{PI, TAU};

use engine::block::RATE;

/// A linear sweep across the voice band under a Hann taper. The band is what the mic path carries
/// and what small speakers and phone mics can reproduce, so the delay is the voice's delay. The
/// taper holds the matched filter's sidelobes well under the onset rule's 20 dB, so a sidelobe is
/// never taken for an arrival.
#[derive(Clone, Copy, Debug)]
pub struct Chirp {
    pub low_hz: f64,
    pub high_hz: f64,
    pub frames: usize,
    /// Peak amplitude, 1.0 being full scale.
    pub level: f32,
}

impl Chirp {
    /// 0.5 to 7.5 kHz over 200 ms. Docs/decisions/0004 has why.
    pub fn voice_band(level: f32) -> Self {
        Self { low_hz: 500.0, high_hz: 7500.0, frames: RATE as usize / 5, level }
    }

    /// The sweep at a frame position that need not be whole, so a test can build a delay of any
    /// fraction of a frame. Zero outside the sweep.
    pub fn at(&self, frame: f64) -> f32 {
        let length = self.frames as f64;
        if !(0.0..length).contains(&frame) {
            return 0.0;
        }
        let rate = f64::from(RATE);
        let seconds = frame / rate;
        let sweep = (self.high_hz - self.low_hz) / (length / rate);
        let phase = TAU * (self.low_hz * seconds + sweep * seconds * seconds / 2.0);
        let taper = (PI * frame / length).sin().powi(2);
        (f64::from(self.level) * taper * phase.sin()) as f32
    }

    pub fn samples(&self) -> Vec<f32> {
        (0..self.frames).map(|frame| self.at(frame as f64)).collect()
    }
}
