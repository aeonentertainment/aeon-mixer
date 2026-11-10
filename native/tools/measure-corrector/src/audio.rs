//! Audio files on the tool's edge. Decoding goes through the engine's own stem decoder, so a clip
//! reaches the slot at 48 kHz by the resampler a song's stems take.

use std::path::Path;

use engine::backing::{Planar, RenderJob, StemFiles};
use engine::block::RATE;

/// A file as stereo at 48 kHz, through the worker the backing path uses.
pub fn decode_stereo(path: &Path) -> Result<Planar, String> {
    let files = StemFiles { instrumental: path.to_owned(), guide: None };
    let stems = RenderJob::start(files, 0, 1.0).finish().map_err(|e| e.to_string())?;
    Ok(stems.instrumental)
}

/// A file folded to mono at 48 kHz: a slot's input.
pub fn decode_mono(path: &Path) -> Result<Vec<f32>, String> {
    let stereo = decode_stereo(path)?;
    Ok(stereo.left.iter().zip(&stereo.right).map(|(l, r)| 0.5 * (l + r)).collect())
}

/// What `write_wav` wrote, as left and right; refuses any other file.
pub fn read_wav(path: &Path) -> Result<(Vec<f32>, Vec<f32>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let header = bytes.get(..44).ok_or(format!("{} is too short for a wav", path.display()))?;
    let stereo_16_bit =
        header[22..24] == 2u16.to_le_bytes() && header[34..36] == 16u16.to_le_bytes();
    if &header[..4] != b"RIFF" || header[24..28] != RATE.to_le_bytes() || !stereo_16_bit {
        return Err(format!("{} is not a wav this tool wrote", path.display()));
    }
    let sample = |pair: &[u8; 2]| f32::from(i16::from_le_bytes(*pair)) / f32::from(i16::MAX);
    let frames = bytes[44..].as_chunks::<4>().0;
    let left = frames.iter().map(|frame| sample(&[frame[0], frame[1]])).collect();
    let right = frames.iter().map(|frame| sample(&[frame[2], frame[3]])).collect();
    Ok((left, right))
}

/// 16-bit PCM at 48 kHz with nothing else in the file, so two takes differ only in their samples.
pub fn write_wav(path: &Path, left: &[f32], right: &[f32]) -> Result<(), String> {
    let data = (left.len() * 4) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(RATE.to_le_bytes());
    bytes.extend((RATE * 4).to_le_bytes());
    bytes.extend(4u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    for (l, r) in left.iter().zip(right) {
        for sample in [l, r] {
            let scaled = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16;
            bytes.extend(scaled.to_le_bytes());
        }
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}
