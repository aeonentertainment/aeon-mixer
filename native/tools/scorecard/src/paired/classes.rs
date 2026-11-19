//! Which of a pair's frames are singing, which are an s and which are the room between lines,
//! read on the reference alone so both takes are judged on the same moments (docs/design.md 8.9).

use super::frame::{Frame, db};

/// How far under the reference's loud frames a frame may stand and still be singing.
const SINGING_DB: f64 = 30.0;

pub struct Classes {
    pub voiced: Vec<bool>,
    pub unvoiced: Vec<bool>,
    pub between_lines: Vec<bool>,
}

impl Classes {
    /// The reference's frames classed, or `None` when none of them is singing.
    pub fn of(reference: &[Frame]) -> Option<Self> {
        let levels: Vec<f64> = reference.iter().map(|frame| db(frame.power)).collect();
        let mut sorted = levels.clone();
        sorted.sort_by(f64::total_cmp);
        let loud = *sorted.get(sorted.len() * 95 / 100)?;
        let quietest_tenth = sorted[(sorted.len() / 10).max(1) - 1];
        let singing = |k: usize| levels[k] >= loud - SINGING_DB;
        let classes = Self {
            voiced: (0..levels.len())
                .map(|k| singing(k) && reference[k].under >= reference[k].over)
                .collect(),
            unvoiced: (0..levels.len())
                .map(|k| singing(k) && reference[k].over > reference[k].under)
                .collect(),
            between_lines: levels.iter().map(|&level| level <= quietest_tenth).collect(),
        };
        classes.voiced.contains(&true).then_some(classes)
    }
}
