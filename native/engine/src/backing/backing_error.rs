use std::fmt;

/// Why a render job produced no stems.
#[derive(Debug)]
pub enum BackingError {
    /// A stem could not be opened, probed, decoded or resampled. `stem` is its path, or for bytes
    /// its kind.
    Decode { stem: String, reason: String },
    /// The worker thread panicked.
    Worker,
}

impl fmt::Display for BackingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode { stem, reason } => write!(f, "{stem}: {reason}"),
            Self::Worker => f.write_str("the backing render job panicked"),
        }
    }
}

impl std::error::Error for BackingError {}
