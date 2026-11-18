use kernels::PitchClassMask;

/// What a guide asks for at one moment: the classes to pull toward, and the note behind them,
/// `None` when it is the key's: its pitch, and its first frame on the guide's clock, which tells
/// two notes in a row on one pitch apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub mask: PitchClassMask,
    pub midi: Option<i16>,
    pub start_frame: Option<u64>,
}
