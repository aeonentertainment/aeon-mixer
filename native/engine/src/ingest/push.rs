//! A `push` slot's source: stereo f32 the control thread hands in through an SPSC ring, folded
//! to mono on the way out. Silence when the ring is empty.

use crate::slot::source::Source;

/// Frames the ring holds unless the caller says otherwise: 100 ms, so a host feeding it once
/// per event-loop turn never runs it dry.
pub const DEFAULT_CAPACITY_FRAMES: usize = 4800;

pub fn push_voice(capacity_frames: usize) -> (PushProducer, PushVoice) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity_frames);
    (PushProducer(producer), PushVoice(consumer))
}

pub struct PushProducer(rtrb::Producer<[f32; 2]>);

impl PushProducer {
    /// Takes interleaved stereo and returns the frames that fit. Never blocks: what does not fit
    /// is the caller's to hand in again.
    pub fn push(&mut self, interleaved: &mut [f32]) -> usize {
        let (frames, _) = interleaved.as_chunks::<2>();
        frames.iter().take_while(|frame| self.0.push(**frame).is_ok()).count()
    }
}

pub struct PushVoice(rtrb::Consumer<[f32; 2]>);

impl Source for PushVoice {
    fn pull(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = match self.0.pop() {
                Ok([left, right]) => (left + right) * 0.5,
                Err(_) => 0.0,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    #[test]
    fn stereo_in_folds_to_mono_out_and_silence_follows() {
        let (mut producer, mut voice) = push_voice(4);
        assert_eq!(producer.push(&mut [1.0, 0.0, 0.5, 0.5, -1.0, 1.0, 0.2, 0.2, 9.0, 9.0]), 4);
        let mut out = [7.0; 6];
        assert_no_alloc(|| voice.pull(&mut out));
        assert_eq!(out, [0.5, 0.5, 0.0, 0.2, 0.0, 0.0]);
    }
}
