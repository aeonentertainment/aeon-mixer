use std::fmt;
use std::path::PathBuf;

/// Why a render job produced no stems.
#[derive(Debug)]
pub enum BackingError {
    /// The file could not be opened, probed, decoded or resampled.
    Decode { path: PathBuf, reason: String },
    /// The worker thread panicked.
    Worker,
}

impl fmt::Display for BackingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode { path, reason } => write!(f, "{}: {reason}", path.display()),
            Self::Worker => f.write_str("the backing render job panicked"),
        }
    }
}

impl std::error::Error for BackingError {}
