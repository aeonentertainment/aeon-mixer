//! Where a slot's audio comes from. The ingest workstream implements this for the jitter buffer
//! and the push ring (docs/design.md 8.4); the graph only asks for one block at a time.

use kernels::SourceDelay;

/// One block of mono input, pulled on the audio thread. `pull` fills `out` completely, with
/// silence when nothing is due, and allocates nothing.
pub trait Source: Send {
    fn pull(&mut self, out: &mut [f32]);

    /// `ResetSlot`: forget the stream and treat the next audio as a new singer.
    fn reset(&mut self) {}

    /// A move to another output: what the source holds was paced by the clock that stopped, and
    /// the move's gap waited in it. Unless the source says otherwise, that is a reset.
    fn restart(&mut self) {
        self.reset();
    }

    /// Whether the source hears one device for the slot's whole life, so that `ResetSlot` brings no
    /// new sender and leaves the slot's echo stages and levels the path they have learned.
    fn keeps_its_sender(&self) -> bool {
        false
    }

    /// The output moved to a device whose callback takes `callback_frames` at a wake. Only a source
    /// whose ring the output's wakes shape needs it.
    fn output_moved(&mut self, _callback_frames: u32) {}

    /// How long ago the singer sang what `pull` hands over: the mic path's delay up to this
    /// slot, such as a jitter buffer's playout delay and a decoder's frame. The guide clock
    /// subtracts it (docs/design.md section 3).
    fn latency_frames(&self) -> u32 {
        0
    }

    /// Where the echo canceller finds the source's delay: its latency, and the frames it has
    /// spliced in less those it took out, which move the echo without moving the latency.
    fn delay(&self) -> SourceDelay {
        SourceDelay { latency_frames: self.latency_frames(), ..SourceDelay::default() }
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use std::f32::consts::TAU;

    use rtrb::Producer;

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

    /// A source with a tap on what it hands the slot: what the slot plays with the canceller off,
    /// which hands its source over bit for bit.
    pub struct Tapped {
        pub inner: Box<dyn Source>,
        pub handed: Producer<f32>,
    }

    impl Source for Tapped {
        fn pull(&mut self, out: &mut [f32]) {
            self.inner.pull(out);
            for sample in out.iter() {
                let _ = self.handed.push(*sample);
            }
        }

        fn reset(&mut self) {
            self.inner.reset();
        }

        fn restart(&mut self) {
            self.inner.restart();
        }

        fn keeps_its_sender(&self) -> bool {
            self.inner.keeps_its_sender()
        }

        fn latency_frames(&self) -> u32 {
            self.inner.latency_frames()
        }

        fn delay(&self) -> kernels::SourceDelay {
            self.inner.delay()
        }
    }
}
