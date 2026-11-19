use crate::slot::effects::TargetNote;

/// One detector reading, every 10 ms, placed at its window's centre on the guide's clock. The
/// slot pushes it whole as `Event::Pitch`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub position_ms: f64,
    pub hz: f32,
    pub confidence: f32,
    /// The guide note the corrector pulls toward; `None` with correction off, with no guide, or
    /// between notes, where it pulls toward the key.
    pub note: Option<TargetNote>,
    /// The class it pulls toward less the reading, in the singer's own octave, so positive is
    /// flat; 0 with correction off or while unvoiced.
    pub error_semitones: f32,
    /// The transposition the voice at the reading's centre came out with, so the corrected
    /// voice sat `error_semitones - shift_semitones` from its class; 0 with correction off.
    pub shift_semitones: f32,
    /// Whether the corrector was pulling the voice as it read: a voice it hears, a class to pull
    /// it to, and a strength over 0.
    pub pulling: bool,
}
