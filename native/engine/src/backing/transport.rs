/// A backing command as it reaches the backing path, less the id its ack carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Start,
    Seek { to_frame: u64 },
    Pause,
    Resume,
    SetKey { semitones: i8 },
    StopAt { at_frame: u64, fade_frames: u32 },
}
