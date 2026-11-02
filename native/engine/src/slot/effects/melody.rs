//! What the corrector asks of a `Guide`: Aeon Karaoke's `MelodyGuide` lookups, on frames. The
//! notes may come in any order, so each lookup scans them all; a song's few hundred notes cost
//! the audio thread far less than a block of the corrector.

use kernels::{EVERY_PITCH_CLASS, PitchClassMask, pitch_class_of};

use crate::slot::effects::Target;
use crate::slot::guide::Guide;

/// The latest-starting note sounding at `frame`, so an overlap favours the newer note; the key's
/// classes when none is. `semitones` is the backing's key shift, which moves the melody and the
/// key with it. A note sounds from its start frame up to, not including, its end frame.
pub fn target_at(guide: &Guide, frame: i64, semitones: i8) -> Target {
    let sounding = guide
        .notes
        .iter()
        .filter(|note| signed(note.start_frame) <= frame && frame < signed(note.end_frame))
        .max_by_key(|note| note.start_frame);
    match sounding {
        Some(note) => {
            let midi = i16::from(note.midi) + i16::from(semitones);
            Target { mask: pitch_class_of(midi.into()), midi: Some(midi) }
        }
        None => Target { mask: transpose(guide.key_mask, semitones), midi: None },
    }
}

/// The first note start or end after `frame`; `None` when none is left.
pub fn next_edge_after(guide: &Guide, frame: i64) -> Option<i64> {
    guide
        .notes
        .iter()
        .flat_map(|note| [signed(note.start_frame), signed(note.end_frame)])
        .filter(|edge| *edge > frame)
        .min()
}

/// A frame on the guide's clock, which is signed because the clock starts before the song once
/// its offsets are taken off. A song is never 2^63 frames long.
fn signed(frame: u64) -> i64 {
    i64::try_from(frame).unwrap_or(i64::MAX)
}

/// The mask moved up by `semitones`, each class wrapping round the octave.
fn transpose(mask: PitchClassMask, semitones: i8) -> PitchClassMask {
    let mask = mask & EVERY_PITCH_CLASS;
    let shift = u32::from(semitones.rem_euclid(12).unsigned_abs());
    ((mask << shift) | (mask >> (12 - shift))) & EVERY_PITCH_CLASS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot::guide::Note;

    // A B C D E F G
    const A_MINOR: PitchClassMask = 0b1010_1011_0101;

    fn guide(notes: &[(u8, u64, u64)]) -> Guide {
        let notes = notes.iter().map(|&(midi, start_frame, end_frame)| Note {
            midi,
            start_frame,
            end_frame,
        });
        Guide { notes: notes.collect(), key_mask: A_MINOR }
    }

    fn key() -> Target {
        Target { mask: A_MINOR, midi: None }
    }

    fn melody(midi: i16) -> Target {
        Target { mask: pitch_class_of(midi.into()), midi: Some(midi) }
    }

    #[test]
    fn follows_the_note_and_falls_back_to_the_key() {
        let guide = guide(&[(72, 2500, 3000), (69, 1000, 1500), (71, 1500, 2000)]);
        assert_eq!(target_at(&guide, 999, 0), key(), "before the first note");
        assert_eq!(target_at(&guide, 1000, 0), melody(69), "a note's start");
        assert_eq!(target_at(&guide, 1500, 0), melody(71), "a shared edge");
        assert_eq!(target_at(&guide, 2200, 0), key(), "a gap between notes");
        assert_eq!(target_at(&guide, 3000, 0), key(), "the frame a note ends");
        assert_eq!(target_at(&guide, -50, 0), key(), "before the song");
    }

    #[test]
    fn an_overlap_favours_the_newer_note() {
        let guide = guide(&[(60, 0, 1000), (64, 500, 700)]);
        assert_eq!(target_at(&guide, 600, 0).midi, Some(64), "the overlap kept the older note");
        assert_eq!(target_at(&guide, 800, 0).midi, Some(60), "the older note did not resume");
    }

    #[test]
    fn edges_come_in_order_once() {
        let guide = guide(&[(72, 2500, 3000), (69, 1000, 1500), (71, 1500, 2000)]);
        let edges: Vec<i64> =
            std::iter::successors(next_edge_after(&guide, -1), |at| next_edge_after(&guide, *at))
                .collect();
        assert_eq!(edges, [1000, 1500, 2000, 2500, 3000]);
    }

    // A live key change moves the melody and the key by the same semitones
    #[test]
    fn a_key_shift_moves_the_melody_and_the_key() {
        let guide = guide(&[(69, 1000, 1500)]);
        assert_eq!(target_at(&guide, 1200, 2), melody(71));
        assert_eq!(target_at(&guide, 1200, -1), melody(68));
        // A minor up two is B minor: B C# D E F# G A
        assert_eq!(target_at(&guide, 0, 2).mask, 0b1010_1101_0110);
        assert_eq!(target_at(&guide, 0, -12).mask, A_MINOR);
    }
}
