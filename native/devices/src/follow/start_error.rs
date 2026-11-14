use std::fmt;

use engine::driver::DriverError;

use crate::output::Output;

/// Why `Follower::start` returned no follower. A host tells an output that will not start from
/// everything else by the variant, never by the message.
#[derive(Debug)]
pub enum StartError {
    /// The output the rule picks would not start, or with `output` none, the OS listed nothing to
    /// pick. Another output may still start.
    Unavailable { output: Option<Output>, error: DriverError },
    /// The OS could not list or watch its outputs, or the follower thread would not spawn.
    Backend(DriverError),
}

impl fmt::Display for StartError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { output: Some(output), error } => {
                write!(f, "{} would not start: {error}", output.name)
            }
            Self::Unavailable { output: None, error } | Self::Backend(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for StartError {}
