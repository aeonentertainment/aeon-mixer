//! The end of the graph: the bus through a soft clipper with no lookahead, then the master
//! meter. The return ring producer (docs/design.md 8.7) joins this file with its workstream.

use crate::block::Stereo;
use crate::command::{Event, EventProducer};
use crate::slot::meter::Meter;

/// Below this magnitude the clipper is the identity. Four singers over a backing track at
/// sensible gains sit under it; only the sum's peaks are bent.
pub const KNEE: f32 = 0.75;

pub struct Master {
    meter: Meter,
}

impl Master {
    pub fn new() -> Self {
        Self { meter: Meter::new() }
    }

    pub fn process(&mut self, out: &mut Stereo<'_>, events: &mut EventProducer) {
        for (left, right) in out.left.iter_mut().zip(out.right.iter_mut()) {
            *left = soft_clip(*left);
            *right = soft_clip(*right);
            let power = (*left * *left + *right * *right) / 2.0;
            if let Some(reading) = self.meter.push_frame(power, left.abs().max(right.abs())) {
                events
                    .push(Event::MasterLevel { rms_db: reading.rms_db, peak_db: reading.peak_db });
            }
        }
    }
}

impl Default for Master {
    fn default() -> Self {
        Self::new()
    }
}

/// Identity up to the knee, then the excess is bent with `u / (1 + u)`, which leaves the knee
/// with unit slope and approaches but never reaches full scale.
pub fn soft_clip(sample: f32) -> f32 {
    let magnitude = sample.abs();
    if magnitude <= KNEE {
        return sample;
    }
    let headroom = 1.0 - KNEE;
    let excess = (magnitude - KNEE) / headroom;
    (KNEE + headroom * excess / (1.0 + excess)).copysign(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clipper_passes_the_knee_and_bounds_everything_above() {
        for sample in [0.0, 0.1, -0.5, KNEE, -KNEE] {
            assert_eq!(soft_clip(sample), sample);
        }
        for sample in [0.8, 1.0, 1.5, 4.0, 100.0] {
            let clipped = soft_clip(sample);
            assert!(clipped > KNEE && clipped < 1.0, "{sample} -> {clipped}");
            assert_eq!(soft_clip(-sample), -clipped);
        }
        assert!(soft_clip(1.0) < soft_clip(1.5), "the curve keeps rising");
    }
}
