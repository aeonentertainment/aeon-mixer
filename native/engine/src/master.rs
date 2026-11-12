//! The end of the graph: the bus through a soft clipper with no lookahead, then the master
//! meter, then the device and, when the mixer has one, the return ring (docs/design.md 8.7).
//! What goes to the device also goes into the echo history, every slot's canceller's reference.

use kernels::EchoHistory;

use crate::block::Stereo;
use crate::command::{Event, EventProducer};
use crate::return_ring::ReturnProducer;
use crate::slot::meter::Meter;

/// Below this magnitude the clipper is the identity. Four singers over a backing track at
/// sensible gains sit under it; only the sum's peaks are bent.
pub const KNEE: f32 = 0.75;

pub struct Master {
    meter: Meter,
    return_ring: Option<ReturnProducer>,
    history: EchoHistory,
}

impl Master {
    pub fn new() -> Self {
        Self { meter: Meter::new(), return_ring: None, history: EchoHistory::new() }
    }

    /// Sends every block from here on down the return branch as well as to the device.
    pub fn set_return(&mut self, ring: ReturnProducer) {
        self.return_ring = Some(ring);
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
        if let Some(ring) = &mut self.return_ring {
            ring.push(out);
        }
        self.history.push(out.left, out.right);
    }

    /// What this graph has sent to the device, reactions included, up to the last block.
    pub fn history(&self) -> &EchoHistory {
        &self.history
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
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::block::BLOCK;
    use crate::command::event_ring;
    use crate::return_ring::return_ring;

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

    #[test]
    fn the_return_ring_carries_the_clipped_block_the_device_gets() {
        let (ring, mut drain) = return_ring();
        let (mut events, _) = event_ring();
        let mut master = Master::new();
        master.set_return(ring);
        let mut left: [f32; BLOCK] = std::array::from_fn(|i| i as f32 / 64.0);
        let mut right = [-2.0; BLOCK];
        let mut out = Stereo { left: &mut left, right: &mut right };
        assert_no_alloc(|| master.process(&mut out, &mut events));

        let mut sent = [[0.0; 2]; BLOCK];
        assert!(drain.pop_exact(&mut sent));
        let device: Vec<[f32; 2]> = left.iter().zip(&right).map(|(l, r)| [*l, *r]).collect();
        assert_eq!(sent[..], device[..]);
        assert!(sent.iter().all(|[_, r]| *r > -1.0), "after the clipper");
    }
}
