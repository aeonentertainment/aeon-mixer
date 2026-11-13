use std::time::Duration;

use engine::driver::StreamInfo;

use crate::follow::refused::Refused;
use crate::output::Output;

/// Where the engine plays now, as the follower last opened it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playing {
    pub info: StreamInfo,
    pub output: Output,
    /// Moves since the engine opened; 0 on the output it opened on.
    pub moves: u32,
    /// The last move's gap, from the wake that started it to the new stream running.
    pub gap: Duration,
    /// Where the rule says to play when that output would not start: the engine stays on `output`
    /// until the rule picks another. None while it plays where the rule says.
    pub refused: Option<Refused>,
}
