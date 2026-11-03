//! Opus through the `opus` crate (opus-rs 0.4) over `opusic-sys` 0.7, which bundles libopus
//! 1.6.1 and builds it with cmake. opus-rs is the safe binding Rust voice stacks have used for
//! years, and its `decode_float` takes a caller's buffer and a FEC flag, which is all the jitter
//! buffer needs.
//!
//! On allocation: `opus_decoder_create` is the one malloc, on the control thread. A decode
//! works on the stack, since libopus's cmake build uses C99 variable-length arrays or `alloca`
//! (`cmake/OpusConfig.cmake` picks one; the pseudo-stack that would malloc is never chosen),
//! and the decoder's state lives in the block `create` returned. The tests' `assert_no_alloc`
//! guard sees Rust's allocator only, so this rests on libopus's code the way the kernels'
//! README says of the vendored C++.

use crate::block::RATE;

/// Opus's shortest frame, 2.5 ms. Concealment comes in multiples of it.
pub const MIN_FRAME: usize = 120;

/// Opus's longest packet, 120 ms, and the most one decode writes.
pub const MAX_FRAME: usize = 5760;

pub struct OpusDecoder {
    inner: opus::Decoder,
}

impl OpusDecoder {
    /// Mono: libopus folds a stereo packet to mono for a mono decoder, so a phone sending
    /// either way lands in a mono slot.
    pub fn new() -> Result<Self, opus::Error> {
        Ok(Self { inner: opus::Decoder::new(RATE, opus::Channels::Mono)? })
    }

    /// Decodes one packet into `out` and returns the frames written, or zero for a packet
    /// libopus rejects.
    pub fn decode(&mut self, payload: &[u8], out: &mut [f32]) -> usize {
        self.inner.decode_float(payload, out, false).unwrap_or(0)
    }

    /// Fills `out` for a frame that never came. With the next packet, libopus takes the
    /// in-band FEC it carries when the encoder sent any; without one, or when it did not, its
    /// loss concealment. libopus works in whole 2.5 ms steps; any remainder is silence.
    pub fn conceal(&mut self, next: Option<&[u8]>, out: &mut [f32]) {
        let whole = out.len() / MIN_FRAME * MIN_FRAME;
        let written = match next {
            Some(packet) => self.inner.decode_float(packet, &mut out[..whole], true),
            None => self.inner.decode_float(&[], &mut out[..whole], false),
        }
        .unwrap_or(0);
        out[written..].fill(0.0);
    }
}
