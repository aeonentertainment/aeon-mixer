use kernels::PitchClassMask;

/// What a guide asks for at one moment: the classes to pull toward, and the note behind them,
/// `None` when it is the key's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub mask: PitchClassMask,
    pub midi: Option<i16>,
}
