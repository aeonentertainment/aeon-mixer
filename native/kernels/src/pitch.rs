//! What the corrector and the detector share: Aeon Karaoke's `Pitch.h` in Rust.

/// The twelve pitch classes a voice may be pulled toward, as bits, C in bit 0.
pub type PitchClassMask = u16;

pub const EVERY_PITCH_CLASS: PitchClassMask = 0x0FFF;

/// A melody note as a mask: one class, so a singer an octave away is corrected in their own
/// octave.
pub const fn pitch_class_of(midi: i32) -> PitchClassMask {
    1 << midi.rem_euclid(12)
}

/// One detector reading; `hz` is 0 in silence. The layout is the shim's `AeonPitch`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pitch {
    pub hz: f32,
    /// 0 to 1: one minus the depth of YIN's dip; above 0.9 is a voice YIN is sure of.
    pub confidence: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_note_is_its_class_in_every_octave() {
        assert_eq!(pitch_class_of(60), 1);
        assert_eq!(pitch_class_of(69), 1 << 9);
        assert_eq!(pitch_class_of(-3), 1 << 9);
    }
}
