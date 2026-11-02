//! RMS and peak over the last 100 ms, read out at 10 Hz: the level numbers of docs/design.md
//! section 4. Fed one frame at a time so a window boundary can fall inside a block.

use crate::block::RATE;

/// 100 ms of frames; one reading per window gives the 10 Hz cadence.
pub const WINDOW_FRAMES: u32 = RATE / 10;

/// The quietest level reported, so silence reads as a number and not minus infinity.
pub const FLOOR_DB: f32 = -120.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub rms_db: f32,
    pub peak_db: f32,
}

#[derive(Default)]
pub struct Meter {
    power: f64,
    peak: f32,
    frames: u32,
}

impl Meter {
    pub fn new() -> Self {
        Self::default()
    }

    /// `power` is the frame's mean square across its channels and `peak` its largest magnitude.
    /// Returns a reading on the frame that completes a window.
    pub fn push_frame(&mut self, power: f32, peak: f32) -> Option<Reading> {
        self.power += f64::from(power);
        self.peak = self.peak.max(peak);
        self.frames += 1;
        if self.frames < WINDOW_FRAMES {
            return None;
        }
        let mean_power = (self.power / f64::from(self.frames)) as f32;
        let reading = Reading {
            rms_db: (10.0 * mean_power.log10()).max(FLOOR_DB),
            peak_db: (20.0 * self.peak.log10()).max(FLOOR_DB),
        };
        *self = Self::default();
        Some(reading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_half_scale_sine_reads_minus_nine_and_minus_six_db() {
        let mut meter = Meter::new();
        let mut readings = Vec::new();
        for n in 0..2 * WINDOW_FRAMES {
            let s = 0.5 * (n as f32 * std::f32::consts::TAU * 1000.0 / RATE as f32).sin();
            if let Some(reading) = meter.push_frame(s * s, s.abs()) {
                readings.push(reading);
            }
        }
        assert_eq!(readings.len(), 2);
        for reading in readings {
            assert!((reading.rms_db - -9.03).abs() < 0.1, "rms {}", reading.rms_db);
            assert!((reading.peak_db - -6.02).abs() < 0.1, "peak {}", reading.peak_db);
        }
    }

    #[test]
    fn silence_reads_the_floor() {
        let mut meter = Meter::new();
        let reading = (0..WINDOW_FRAMES).find_map(|_| meter.push_frame(0.0, 0.0)).unwrap();
        assert_eq!(reading, Reading { rms_db: FLOOR_DB, peak_db: FLOOR_DB });
    }
}
