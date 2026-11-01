//! The fixed numbers of docs/design.md section 4, and the buffer one render fills.

/// Opus's native rate, so nothing resamples.
pub const RATE: u32 = 48_000;

/// Frames per render: the output quantum and the unit of every ramp, 2.67 ms.
pub const BLOCK: usize = 128;

/// `Params` is fixed-size, so the slot count has a ceiling.
pub const MAX_SLOTS: usize = 16;

/// One block of planar stereo output, `BLOCK` frames a side.
pub struct Stereo<'a> {
    pub left: &'a mut [f32],
    pub right: &'a mut [f32],
}

impl Stereo<'_> {
    pub fn silence(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
    }
}
