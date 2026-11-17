//! A `push` or `capture` slot in a replay. Format 1 records no PCM for either, so each plays as
//! silence (docs/design.md 8.11).

use engine::slot::source::Source;

pub struct Silence;

impl Source for Silence {
    fn pull(&mut self, out: &mut [f32]) {
        out.fill(0.0);
    }
}
