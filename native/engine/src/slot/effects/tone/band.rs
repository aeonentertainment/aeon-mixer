use crate::slot::effects::level::power_db;
use crate::slot::effects::tone::biquad::Biquad;
use crate::slot::effects::tone::follower::Follower;
use crate::slot::effects::tone::section::Section;

const LEVEL_RISE_MS: f32 = 1.0;
const LEVEL_FALL_MS: f32 = 20.0;
/// Under this a band is silence. Without it, the tail of a pop or an s falls as fast in both
/// bands it is judged by, and would hold the cut until the tail reached nothing.
const SILENT_DB: f32 = -70.0;

/// One band a detector listens to: the voice through `pass`, once, or twice for 24 dB an octave,
/// and its level. Only the detector hears it; the voice never passes through it.
pub struct Band {
    pass: Biquad,
    sections: [Section; 2],
    twice: bool,
    level: Follower,
}

impl Band {
    pub fn new(pass: Biquad, twice: bool) -> Self {
        Self {
            pass,
            sections: [Section::new(), Section::new()],
            twice,
            level: Follower::new(LEVEL_RISE_MS, LEVEL_FALL_MS),
        }
    }

    /// The band's level after `x`, in dBFS from the silence floor up.
    pub fn level_db(&mut self, x: f64) -> f32 {
        let [first, second] = &mut self.sections;
        let once = first.tick(&self.pass, x);
        let heard = if self.twice { second.tick(&self.pass, once) } else { once } as f32;
        power_db(self.level.follow(heard * heard)).max(SILENT_DB)
    }
}
