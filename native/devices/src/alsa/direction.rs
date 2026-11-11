/// Which way a handle's audio runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Playback,
    Capture,
}
