/// One audio output the OS offers, as `outputs()` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    /// Stable across restarts: ALSA's `hw:CARD=...,DEV=...`, CoreAudio's device UID, the
    /// MMDevice endpoint id on Windows.
    pub id: String,
    pub name: String,
    pub is_default: bool,
}
