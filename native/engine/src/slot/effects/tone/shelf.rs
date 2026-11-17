use std::f64::consts::{SQRT_2, TAU};

use crate::block::RATE;
use crate::slot::effects::tone::biquad::Biquad;

/// The Audio EQ Cookbook's shelf at slope 1 with its corner fixed and its gain free: the bands
/// take one gain each, and the de-esser asks for a new one every sample as its cut moves.
#[derive(Clone, Copy, Debug)]
pub struct Shelf {
    cos: f64,
    /// The cookbook's `alpha` at slope 1, `sin(w0) / 2 * sqrt(2)`.
    alpha: f64,
    high: bool,
}

impl Shelf {
    pub fn low(hz: f64) -> Self {
        Self::new(hz, false)
    }

    pub fn high(hz: f64) -> Self {
        Self::new(hz, true)
    }

    fn new(hz: f64, high: bool) -> Self {
        let (sin, cos) = (TAU * hz / f64::from(RATE)).sin_cos();
        Self { cos, alpha: sin / 2.0 * SQRT_2, high }
    }

    pub fn at(&self, gain_db: f64) -> Biquad {
        let a = 10f64.powf(gain_db / 40.0);
        let lift = 2.0 * a.sqrt() * self.alpha;
        let (sum, difference) = (a + 1.0, a - 1.0);
        // The high shelf is the low one with the sign of every odd power of cos flipped
        let (cos, odd) = if self.high { (-self.cos, -1.0) } else { (self.cos, 1.0) };
        Biquad::normalised(
            [
                a * (sum - difference * cos + lift),
                odd * 2.0 * a * (difference - sum * cos),
                a * (sum - difference * cos - lift),
            ],
            [
                sum + difference * cos + lift,
                -odd * 2.0 * (difference + sum * cos),
                sum + difference * cos - lift,
            ],
        )
    }
}
