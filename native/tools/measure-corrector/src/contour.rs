//! Reads a pitch contour back out of audio, the way the GStreamer harness's second `aeonpitch`
//! did: the corrector's own detector over the output, every 10 ms.

use engine::block::RATE;
use kernels::YinDetector;

use crate::music::midi_of;

pub const HOP: usize = RATE as usize / 100;
pub const WINDOW: usize = 2048;
/// A reading this sure is a voice: the corrector's own bar.
pub const VOICED_CONFIDENCE: f32 = 0.9;
/// E2. The corrector leaves anything lower alone.
pub const LOWEST_CORRECTED_HZ: f32 = 82.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    /// The centre of the window this read, in milliseconds on the signal's own clock.
    pub ms: f64,
    pub hz: f32,
    pub confidence: f32,
}

impl Point {
    pub fn voiced(&self) -> bool {
        self.hz > 0.0 && self.confidence >= VOICED_CONFIDENCE
    }

    pub fn midi(&self) -> f64 {
        midi_of(f64::from(self.hz))
    }
}

/// Readings of `signal` every hop with a detector of `window` samples, zeros before the start, as
/// the element's history begins. `lag_frames` is how late the signal runs on the clock wanted, so
/// a point's `ms` names the moment in the clip that was sung.
pub fn read_with_window(signal: &[f32], window: usize, lag_frames: i64) -> Vec<Point> {
    let mut detector = YinDetector::new(RATE, window);
    let mut padded = vec![0.0; window];
    padded.extend_from_slice(signal);
    (1..)
        .map(|k| k * HOP)
        .take_while(|end| end + window <= padded.len())
        .map(|end| {
            let read = detector.analyze(&padded[end..end + window]);
            let centre = end as i64 - (window / 2) as i64 - lag_frames;
            Point {
                ms: centre as f64 * 1000.0 / f64::from(RATE),
                hz: read.hz,
                confidence: read.confidence,
            }
        })
        .collect()
}

pub fn read(signal: &[f32], lag_frames: i64) -> Vec<Point> {
    read_with_window(signal, WINDOW, lag_frames)
}
