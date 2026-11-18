use kernels::PitchClassMask;

use crate::slot::effects::TargetNote;

/// What a guide asks for at one moment: the classes to pull toward, and the note behind them,
/// `None` when they are the key's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    pub mask: PitchClassMask,
    pub note: Option<TargetNote>,
}
