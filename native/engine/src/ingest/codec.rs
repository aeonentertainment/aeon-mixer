//! Which decoder a slot's packets go through: Opus for `rtp` slots, L16 for `l16` slots. An
//! enum, not a trait: the mixer's ingest kinds are fixed.

use super::l16;
use super::opus::OpusDecoder;

pub enum Codec {
    Opus(OpusDecoder),
    L16,
}

impl Codec {
    pub fn opus() -> Result<Self, opus::Error> {
        OpusDecoder::new().map(Self::Opus)
    }

    /// Decodes one payload into `out` and returns the frames written; zero means a bad packet.
    pub fn decode(&mut self, payload: &[u8], out: &mut [f32]) -> usize {
        match self {
            Self::Opus(decoder) => decoder.decode(payload, out),
            Self::L16 => l16::decode(payload, out),
        }
    }

    /// Fills `out` for a frame that never came, given the packet after it when that has
    /// arrived. Raw PCM has nothing to conceal with and goes silent.
    pub fn conceal(&mut self, next: Option<&[u8]>, out: &mut [f32]) {
        match self {
            Self::Opus(decoder) => decoder.conceal(next, out),
            Self::L16 => out.fill(0.0),
        }
    }
}
