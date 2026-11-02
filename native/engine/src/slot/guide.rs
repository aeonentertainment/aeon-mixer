//! The melody a slot's corrector follows: built on the control thread from the host's
//! `MelodyGuide`, moved to the audio thread in a `SetGuide` command, and handed back through
//! the retired ring when replaced, so the audio thread never drops it.

pub struct Guide {
    pub notes: Vec<Note>,
    /// The key's pitch classes as bits, C in bit 0.
    pub key_mask: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Note {
    pub midi: u8,
    pub start_frame: u64,
    pub end_frame: u64,
}
