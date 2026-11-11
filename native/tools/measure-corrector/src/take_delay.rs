use engine::block::{BLOCK, RATE};

use crate::contour::Point;

/// How late the corrector made a take's voice, block by block, as the slot reported it. A row
/// that compares the voice in with the voice out frame by frame reads the output this much later,
/// so a vibrato at 5.5 Hz is not called 9 cents wrong for being 5 ms late.
pub struct TakeDelay<'a> {
    per_block: &'a [u32],
}

impl<'a> TakeDelay<'a> {
    pub fn new(per_block: &'a [u32]) -> Self {
        Self { per_block }
    }

    /// No delay at all: a signal already on the input's clock.
    pub const NONE: TakeDelay<'static> = TakeDelay { per_block: &[] };

    /// The delay, in frames, of the output around input frame `frame`.
    pub fn frames_at(&self, frame: usize) -> usize {
        let block = frame / BLOCK;
        self.per_block.get(block).or(self.per_block.last()).map_or(0, |&delay| delay as usize)
    }

    /// Readings of the output moved back onto the input's clock.
    pub fn onto_input(&self, readings: &[Point]) -> Vec<Point> {
        let frames_per_ms = f64::from(RATE) / 1000.0;
        readings
            .iter()
            .map(|point| {
                let frame = (point.ms.max(0.0) * frames_per_ms) as usize;
                let delay = self.frames_at(frame) as f64 / frames_per_ms;
                Point { ms: point.ms - delay, ..*point }
            })
            .collect()
    }
}
