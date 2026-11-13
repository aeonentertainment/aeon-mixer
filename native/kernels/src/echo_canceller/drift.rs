//! Follows an echo path that slides a fraction of a frame at a time, as a mic on its own clock
//! has: at 100 ppm its echo moves 4.8 frames a second against the master. A delay-locked loop
//! reads how far the filter's estimate trails the echo, from the error against the estimate's
//! slope, and moves the reference by that and by the rate it has learned. So the filter sees a
//! path that holds still, and the rate is the two clocks' own (decision 0022).

use super::FRAME;

/// The loop's gains per frame: a natural frequency of 1.2 Hz, damped at 0.75. It learns a 100 ppm
/// rate within two seconds of the filter adapting. Gains of 0.005 and 2e-5 took four, and 7 to
/// 18 dB less out at the fifth second.
const PROPORTIONAL: f64 = 0.03;
const INTEGRAL: f64 = 4e-4;
/// No frame's reading moves the loop by more than half a frame, as a burst of noise might.
const MOST_PER_FRAME: f64 = 0.5;
/// The fastest drift followed, 500 ppm in frames per frame: beyond any audio crystal.
const MOST_RATE: f64 = 500e-6 * FRAME as f64;
/// A frame whose estimate has less slope than this, on the 16-bit scale, carries no reading.
const LEAST_SLOPE: f64 = 1e3;

/// How far the echo has slid from where the source says it is, and how fast it slides.
#[derive(Default)]
pub struct Drift {
    /// Frames the reference is read later than the path the source names.
    offset: f64,
    /// Frames the offset moves by each frame.
    rate: f64,
    /// The estimate's last sample, for the slope at the next frame's first.
    last: f32,
}

impl Drift {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// A new path: the offset belongs to the old one, but the clocks have not changed.
    pub fn restart(&mut self) {
        self.offset = 0.0;
    }

    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The source moved its delay by `frames`, splicing them in or out of what it hands over.
    pub fn shift(&mut self, frames: f64) {
        self.offset += frames;
    }

    /// The learned rate in frames per second at `rate` Hz.
    #[cfg(test)]
    pub fn frames_per_second(&self, rate: u32) -> f64 {
        self.rate * f64::from(rate) / FRAME as f64
    }

    /// One frame: `echo` is the filter's estimate and `error` the mic less it. `reading` is false
    /// while the error is not the echo's alone, during double talk or before the filter adapts;
    /// then the offset moves on at the rate it has learned.
    pub fn follow(&mut self, echo: &[f32; FRAME], error: &[f32; FRAME], reading: bool) {
        if reading {
            // An estimate a fraction `lag` early leaves an error of `-lag` times its slope
            let (mut along, mut slope_energy) = (0.0f64, 0.0f64);
            let mut before = self.last;
            for i in 0..FRAME - 1 {
                let slope = 0.5 * f64::from(echo[i + 1] - before);
                along += f64::from(error[i]) * slope;
                slope_energy += slope * slope;
                before = echo[i];
            }
            if slope_energy > LEAST_SLOPE {
                let lag = (-along / slope_energy).clamp(-MOST_PER_FRAME, MOST_PER_FRAME);
                self.rate = (self.rate + INTEGRAL * lag).clamp(-MOST_RATE, MOST_RATE);
                self.offset += PROPORTIONAL * lag;
            }
        }
        self.last = echo[FRAME - 1];
        self.offset += self.rate;
    }
}
