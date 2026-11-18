use std::path::PathBuf;

/// Where one stem's encoded file is: on disk, or held whole in memory. The bytes are the engine's
/// own copy, which the worker frees once it has decoded them.
pub enum StemSource {
    File(PathBuf),
    Bytes(Vec<u8>),
}

impl From<PathBuf> for StemSource {
    fn from(path: PathBuf) -> Self {
        Self::File(path)
    }
}
