//! Where a slot's audio comes from. The ingest workstream implements this for the jitter buffer
//! and the push ring (docs/design.md 8.4); the graph only asks for one block at a time.

/// One block of mono input, pulled on the audio thread. `pull` fills `out` completely, with
/// silence when nothing is due, and allocates nothing.
pub trait Source: Send {
    fn pull(&mut self, out: &mut [f32]);

    /// `ResetSlot`: forget the stream and treat the next audio as a new singer.
    fn reset(&mut self) {}

    /// How long ago the singer sang what `pull` hands over: the mic path's delay up to this
    /// slot, such as a jitter buffer's playout delay and a decoder's frame. The guide clock
    /// subtracts it (docs/design.md section 3).
    fn latency_frames(&self) -> u32 {
        0
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use std::f32::consts::TAU;

    use super::Source;
    use crate::block::RATE;

    /// A DC level, for asserting exact gains.
    pub struct Constant(pub f32);

    impl Source for Constant {
        fn pull(&mut self, out: &mut [f32]) {
            out.fill(self.0);
        }
    }

    /// A sine at `hz` with the given amplitude, phase-continuous across blocks.
    pub struct Sine {
        step: f32,
        phase: f32,
        amplitude: f32,
    }

    impl Sine {
        pub fn new(hz: f32, amplitude: f32) -> Self {
            Self { step: TAU * hz / RATE as f32, phase: 0.0, amplitude }
        }
    }

    impl Source for Sine {
        fn pull(&mut self, out: &mut [f32]) {
            for sample in out {
                *sample = self.amplitude * self.phase.sin();
                self.phase = (self.phase + self.step) % TAU;
            }
        }
    }

    /// Panics on the first pull: how a test reaches the fault boundary.
    pub struct Panics;

    impl Source for Panics {
        fn pull(&mut self, _out: &mut [f32]) {
            panic!("a source misbehaved");
        }
    }
}
