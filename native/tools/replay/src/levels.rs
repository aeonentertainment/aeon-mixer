//! A voice's level as #100's table reads it: the 100 ms RMS readings the slot's meter makes,
//! summed up as the level 10 percent of readings are above, the median and the loudest. A signal
//! is cut into the same 100 ms windows so it reads on the same scale as the meter.

const WINDOW: usize = 4800;
const FLOOR_DB: f32 = -120.0;

#[derive(Clone, Copy, Debug)]
pub struct Levels {
    pub top_tenth_db: f32,
    pub median_db: f32,
    pub loudest_db: f32,
    pub readings: usize,
}

impl Levels {
    pub fn of_signal(signal: &[f32]) -> Self {
        let readings: Vec<f32> = signal
            .as_chunks::<WINDOW>()
            .0
            .iter()
            .map(|window| {
                let power = window.iter().map(|s| f64::from(s * s)).sum::<f64>() / WINDOW as f64;
                (10.0 * power.log10() as f32).max(FLOOR_DB)
            })
            .collect();
        Self::of_readings(&readings)
    }

    pub fn of_readings(readings: &[f32]) -> Self {
        let mut sorted = readings.to_vec();
        sorted.sort_by(f32::total_cmp);
        let at = |fraction: f64| {
            sorted
                .get(((sorted.len().saturating_sub(1)) as f64 * fraction).round() as usize)
                .copied()
                .unwrap_or(FLOOR_DB)
        };
        Self {
            top_tenth_db: at(0.9),
            median_db: at(0.5),
            loudest_db: at(1.0),
            readings: sorted.len(),
        }
    }
}
