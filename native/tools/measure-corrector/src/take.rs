use engine::ingest::SlotStats;

use crate::contour::Point;

/// Below the master clipper's 0.75 knee for any clip that peaks at full scale, so the clipper
/// stays out of a measurement.
pub const MEASURE_GAIN: f32 = 0.5;

/// What came out.
pub struct Take {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// The slot's own pitch readings, on the guide's clock.
    pub readings: Vec<Point>,
    /// How far the corrector made the voice trail, in frames, once per block.
    pub delays: Vec<u32>,
    pub cpu_seconds: f64,
    pub stats: Option<SlotStats>,
}

impl Take {
    /// The voice, when the setup was the measuring one, at the level it went in.
    pub fn voice(&self) -> Vec<f32> {
        self.left.iter().map(|s| s / MEASURE_GAIN).collect()
    }
}
