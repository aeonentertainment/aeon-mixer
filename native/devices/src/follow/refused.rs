use crate::output::Output;

/// The output the follower's rule chose that would not start, and what its backend said. The
/// engine plays on the output it left instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refused {
    pub output: Output,
    pub error: String,
}
