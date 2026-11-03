use std::ffi::c_int;

/// The sample formats the backend asks a hardware device for, best first.
#[derive(Clone, Copy, Debug)]
pub enum Format {
    F32,
    I32,
    I16,
}

impl Format {
    pub const PREFERRED: [Format; 3] = [Format::F32, Format::I32, Format::I16];

    /// The `snd_pcm_format_t` from alsa/pcm.h.
    pub fn code(self) -> c_int {
        match self {
            Format::F32 => 14,
            Format::I32 => 10,
            Format::I16 => 2,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::F32 => "FLOAT_LE",
            Format::I32 => "S32_LE",
            Format::I16 => "S16_LE",
        }
    }
}
