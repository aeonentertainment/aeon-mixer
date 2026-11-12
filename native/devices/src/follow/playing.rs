use std::time::Duration;

use engine::driver::StreamInfo;

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
}
