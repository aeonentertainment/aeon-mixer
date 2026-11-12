use engine::block::{BLOCK, RATE};
use serde_json::{Value, json};

use crate::chain::Take;
use crate::music::percentile;

/// How late the corrector made a take's voice, in milliseconds, between two times on its clock.
pub struct VocalDelays {
    /// Every 50 ms, as the GStreamer harness sampled it.
    pub all: Vec<f64>,
    /// The same samples where the voice was sounding: a rest holds the delay wherever it stopped,
    /// and most of the golden vocal is rest.
    pub voiced: Vec<f64>,
    /// At the first voiced reading after 100 ms or more of rest: when a sung line starts, which
    /// is when a late voice is heard most.
    pub onsets: Vec<f64>,
}

impl VocalDelays {
    pub fn of(take: &Take, from_seconds: f64, to_seconds: f64) -> Self {
        let ms_at = |frame: usize| f64::from(take.delays[frame / BLOCK]) * 1000.0 / f64::from(RATE);
        let inside = |seconds: f64| from_seconds <= seconds && seconds < to_seconds;
        let voiced_near = |ms: f64| {
            let after = take.readings.partition_point(|point| point.ms < ms);
            [after.checked_sub(1), Some(after)]
                .into_iter()
                .flatten()
                .filter_map(|index| take.readings.get(index))
                .min_by(|a, b| (a.ms - ms).abs().total_cmp(&(b.ms - ms).abs()))
                .is_some_and(|point| point.voiced())
        };

        let sample_every = RATE as usize / 20;
        let sampled: Vec<usize> = (0..)
            .map(|k| k * sample_every)
            .take_while(|frame| frame / BLOCK < take.delays.len())
            .filter(|frame| inside(*frame as f64 / f64::from(RATE)))
            .collect();
        let all = sampled.iter().map(|frame| ms_at(*frame)).collect();
        let voiced = sampled
            .iter()
            .filter(|frame| voiced_near(**frame as f64 * 1000.0 / f64::from(RATE)))
            .map(|frame| ms_at(*frame))
            .collect();

        let mut onsets = Vec::new();
        let mut resting_for = 0;
        for point in &take.readings {
            if !point.voiced() {
                resting_for += 1;
                continue;
            }
            let frame = (point.ms.max(0.0) * f64::from(RATE) / 1000.0) as usize;
            if resting_for >= 10 && inside(point.ms / 1000.0) && frame / BLOCK < take.delays.len() {
                onsets.push(ms_at(frame));
            }
            resting_for = 0;
        }
        Self { all, voiced, onsets }
    }

    pub fn mean(delays: &[f64]) -> f64 {
        delays.iter().sum::<f64>() / delays.len().max(1) as f64
    }

    pub fn most(delays: &[f64]) -> f64 {
        delays.iter().copied().fold(0.0, f64::max)
    }

    pub fn summary(&self) -> Value {
        json!({
            "mean_ms": Self::mean(&self.all),
            "p95_ms": percentile(&self.all, 0.95),
            "max_ms": Self::most(&self.all),
            "voiced_mean_ms": Self::mean(&self.voiced),
            "voiced_p95_ms": percentile(&self.voiced, 0.95),
            "onset_median_ms": percentile(&self.onsets, 0.5),
            "onsets": self.onsets.len(),
        })
    }
}
