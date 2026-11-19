use std::fmt;

use crate::transport::Transport;

/// One audio output the OS offers, as `outputs()` lists it, or one input as `inputs()` does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// Stable across restarts: ALSA's `hw:CARD=...,DEV=...`, CoreAudio's device UID, the
    /// MMDevice endpoint id on Windows.
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub transport: Transport,
}

/// An input is listed with the same fields as an output.
pub type Input = Output;

/// One line of a tool's device list: the id, the name, the transport and the default marked.
impl fmt::Display for Output {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let default = if self.is_default { " (default)" } else { "" };
        write!(formatter, "{}  {}  {}{default}", self.id, self.name, self.transport)
    }
}
