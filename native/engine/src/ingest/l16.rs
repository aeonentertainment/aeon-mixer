//! Raw PCM the house-mic sender puts on loopback: 16-bit big-endian stereo at 48 kHz, folded
//! to mono here because a slot is mono.

pub const BYTES_PER_FRAME: usize = 4;

/// Writes one mono sample per stereo frame of `payload` and returns how many that was. A
/// payload longer than `out` is cut, and a trailing partial frame is ignored.
pub fn decode(payload: &[u8], out: &mut [f32]) -> usize {
    let (frames, _) = payload.as_chunks::<BYTES_PER_FRAME>();
    let n = frames.len().min(out.len());
    for (frame, sample) in frames.iter().zip(out) {
        let left = i16::from_be_bytes([frame[0], frame[1]]);
        let right = i16::from_be_bytes([frame[2], frame[3]]);
        *sample = (f32::from(left) + f32::from(right)) * (0.5 / 32768.0);
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_big_endian_folds_to_mono() {
        // Full scale left with silent right folds to half; a matched pair keeps its level
        let payload = [0x7f, 0xff, 0x00, 0x00, 0x80, 0x00, 0x80, 0x00, 0x12];
        let mut out = [9.0; 3];
        assert_eq!(decode(&payload, &mut out), 2);
        assert!((out[0] - 0.5).abs() < 1e-4);
        assert_eq!(out[1], -1.0);
        assert_eq!(out[2], 9.0, "untouched past the frames");
    }
}
