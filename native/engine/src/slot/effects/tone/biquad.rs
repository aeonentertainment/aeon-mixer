use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use crate::block::RATE;
use crate::params::ToneBand;
use crate::slot::effects::tone::shelf::Shelf;

/// One biquad's coefficients, normalised so a0 is 1, in f64: at 80 Hz the poles sit close enough
/// to 1 that f32 would move the curve. The constructors are the Audio EQ Cookbook's at 48 kHz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Biquad {
    pub b: [f64; 3],
    pub a: [f64; 2],
}

impl Biquad {
    pub const IDENTITY: Self = Self { b: [1.0, 0.0, 0.0], a: [0.0, 0.0] };

    /// From the cookbook's unnormalised `b0 b1 b2` over `a0 a1 a2`.
    pub fn normalised([b0, b1, b2]: [f64; 3], [a0, a1, a2]: [f64; 3]) -> Self {
        Self { b: [b0 / a0, b1 / a0, b2 / a0], a: [a1 / a0, a2 / a0] }
    }

    pub fn band(band: &ToneBand) -> Self {
        match *band {
            ToneBand::Peak { hz, gain_db, q } => {
                Self::peak(f64::from(hz), f64::from(gain_db), f64::from(q))
            }
            ToneBand::LowShelf { hz, gain_db } => Shelf::low(f64::from(hz)).at(f64::from(gain_db)),
            ToneBand::HighShelf { hz, gain_db } => {
                Shelf::high(f64::from(hz)).at(f64::from(gain_db))
            }
        }
    }

    pub fn peak(hz: f64, gain_db: f64, q: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (sin, cos) = (TAU * hz / f64::from(RATE)).sin_cos();
        let alpha = sin / (2.0 * q);
        Self::normalised(
            [1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a],
            [1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a],
        )
    }

    /// A Butterworth low-pass: what a detector listens through, never the voice.
    pub fn low_pass(hz: f64) -> Self {
        let (sin, cos) = (TAU * hz / f64::from(RATE)).sin_cos();
        let alpha = sin * FRAC_1_SQRT_2;
        let side = (1.0 - cos) / 2.0;
        Self::normalised([side, 1.0 - cos, side], [1.0 + alpha, -2.0 * cos, 1.0 - alpha])
    }

    /// A Butterworth high-pass, for a detector.
    pub fn high_pass(hz: f64) -> Self {
        let (sin, cos) = (TAU * hz / f64::from(RATE)).sin_cos();
        let alpha = sin * FRAC_1_SQRT_2;
        let side = (1.0 + cos) / 2.0;
        Self::normalised([side, -1.0 - cos, side], [1.0 + alpha, -2.0 * cos, 1.0 - alpha])
    }

    /// The gain in dB and the group delay in samples at `hz`, from the coefficients: what
    /// `ToneParams::check` judges a cascade by.
    pub fn response(&self, hz: f64) -> (f64, f64) {
        let w = TAU * hz / f64::from(RATE);
        let [b0, b1, b2] = self.b;
        let [a1, a2] = self.a;
        let (numerator, numerator_delay) = polynomial(w, [b0, b1, b2]);
        let (denominator, denominator_delay) = polynomial(w, [1.0, a1, a2]);
        (10.0 * (numerator / denominator).log10(), numerator_delay - denominator_delay)
    }

    /// These poles with zeros on them: unity at every frequency.
    pub fn flat(&self) -> Self {
        Self { b: [1.0, self.a[0], self.a[1]], a: self.a }
    }

    /// Partway, `t` from 0 to 1, from these coefficients to `to`. Every biquad between two stable
    /// ones is stable: the region of stable `a` is a triangle, and a triangle is convex.
    pub fn toward(&self, to: &Self, t: f64) -> Self {
        let mix = |from: f64, to: f64| from + t * (to - from);
        Self {
            b: [mix(self.b[0], to.b[0]), mix(self.b[1], to.b[1]), mix(self.b[2], to.b[2])],
            a: [mix(self.a[0], to.a[0]), mix(self.a[1], to.a[1])],
        }
    }
}

/// `p0 + p1 z^-1 + p2 z^-2` on the unit circle at `w`: its squared magnitude and its group delay
/// in samples, `Re(sum k p_k z^-k / sum p_k z^-k)`.
fn polynomial(w: f64, [p0, p1, p2]: [f64; 3]) -> (f64, f64) {
    let (sin1, cos1) = w.sin_cos();
    let (sin2, cos2) = (2.0 * w).sin_cos();
    let (re, im) = (p0 + p1 * cos1 + p2 * cos2, -(p1 * sin1 + p2 * sin2));
    let (ramp_re, ramp_im) = (p1 * cos1 + 2.0 * p2 * cos2, -(p1 * sin1 + 2.0 * p2 * sin2));
    let power = re * re + im * im;
    (power, (ramp_re * re + ramp_im * im) / power)
}
