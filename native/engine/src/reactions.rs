//! The room's reaction sounds: four stereo voices the engine owns for its whole life, so they
//! play between songs and through a graph swap. Each is an SPSC ring of frames the control
//! thread fills with a whole sound; the audio thread sums every playing voice into the bus ahead
//! of the clipper (docs/design.md section 4). A fifth sound while four play is dropped (7.2).

use rtrb::{Consumer, Producer, RingBuffer};

use crate::block::{RATE, Stereo};

pub const VOICES: usize = 4;

/// Eight seconds a voice: longer than any reaction sound the room plays. A longer sound is cut at
/// the ring rather than buffered somewhere else and topped up.
pub const VOICE_FRAMES: usize = 8 * RATE as usize;

pub fn reactions() -> (ReactionsFeed, Reactions) {
    let (feeds, voices) = (0..VOICES)
        .map(|_| {
            let (producer, consumer) = RingBuffer::new(VOICE_FRAMES);
            (producer, consumer)
        })
        .unzip();
    (ReactionsFeed { voices: feeds }, Reactions { voices })
}

/// The control thread's end.
pub struct ReactionsFeed {
    voices: Vec<Producer<[f32; 2]>>,
}

impl ReactionsFeed {
    /// Plays interleaved stereo on an idle voice. False when four already play: the sound is
    /// dropped, as 7.2 promises.
    pub fn play(&mut self, stereo: &[f32]) -> bool {
        let Some(voice) = self.voices.iter_mut().find(|voice| voice.slots() == VOICE_FRAMES) else {
            return false;
        };
        let (frames, _) = stereo.as_chunks::<2>();
        for frame in frames {
            if voice.push(*frame).is_err() {
                break;
            }
        }
        true
    }
}

/// The audio thread's end.
pub struct Reactions {
    voices: Vec<Consumer<[f32; 2]>>,
}

impl Reactions {
    /// One block: the sum of every playing voice, zeros when none is. Allocation-free.
    pub fn render(&mut self, mut out: Stereo<'_>) {
        out.silence();
        for voice in &mut self.voices {
            for (left, right) in out.left.iter_mut().zip(out.right.iter_mut()) {
                let Ok([l, r]) = voice.pop() else { break };
                *left += l;
                *right += r;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::block::BLOCK;

    fn render(reactions: &mut Reactions) -> ([f32; BLOCK], [f32; BLOCK]) {
        let (mut left, mut right) = ([f32::NAN; BLOCK], [f32::NAN; BLOCK]);
        assert_no_alloc(|| reactions.render(Stereo { left: &mut left, right: &mut right }));
        (left, right)
    }

    #[test]
    fn four_voices_sum_and_a_fifth_is_dropped() {
        let (mut feed, mut reactions) = reactions();
        let sound = |left: f32, right: f32| [left, right].repeat(BLOCK);
        for _ in 0..VOICES {
            assert!(feed.play(&sound(0.1, 0.2)));
        }
        assert!(!feed.play(&sound(1.0, 1.0)), "a fifth sound while four play");

        let (left, right) = render(&mut reactions);
        assert!(left.iter().all(|s| (*s - 0.4).abs() < 1e-6), "{left:?}");
        assert!(right.iter().all(|s| (*s - 0.8).abs() < 1e-6));
        let (left, _) = render(&mut reactions);
        assert!(left.iter().all(|s| *s == 0.0), "silence once the voices run dry");
        assert!(feed.play(&sound(0.5, 0.5)), "and a voice is free again");
    }
}
