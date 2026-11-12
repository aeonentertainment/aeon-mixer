use kernels::EchoHistory;

use super::cue::Cue;

/// What every slot in a graph shares for one block, built by the graph before the first renders.
#[derive(Clone, Copy)]
pub struct Shared<'a> {
    /// Any slot is soloed, which silences every slot that is not.
    pub any_solo: bool,
    /// Where the backing the room hears is, for the corrector.
    pub cue: Cue,
    /// What the room has been played, for the echo canceller.
    pub history: &'a EchoHistory,
}
