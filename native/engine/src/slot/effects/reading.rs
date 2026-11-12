/// One detector reading, every 10 ms, placed at its window's centre on the guide's clock. The
/// slot turns it into `Event::Pitch`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub position_ms: f64,
    pub hz: f32,
    pub confidence: f32,
    /// The guide note the corrector pulls toward, after the key shift; `None` with correction
    /// off, with no guide, or between notes, where it pulls toward the key.
    pub target_midi: Option<i16>,
    /// The class it pulls toward less the reading, in the singer's own octave, so positive is
    /// flat; 0 with correction off or while unvoiced.
    pub error_semitones: f32,
}
