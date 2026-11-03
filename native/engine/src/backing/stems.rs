use crate::backing::Planar;

/// A song's stems in memory, decoded and rendered at the armed key and speed, every one `frames`
/// long so one playhead reads them all.
pub struct Stems {
    pub instrumental: Planar,
    pub guide: Option<Planar>,
    pub frames: u64,
}

impl Stems {
    /// Pads the shorter stem with silence to the longer one's length.
    pub fn new(mut instrumental: Planar, mut guide: Option<Planar>) -> Self {
        let frames = instrumental.frames().max(guide.as_ref().map_or(0, Planar::frames));
        instrumental.pad_to(frames);
        if let Some(guide) = &mut guide {
            guide.pad_to(frames);
        }
        Self { instrumental, guide, frames: frames as u64 }
    }
}
