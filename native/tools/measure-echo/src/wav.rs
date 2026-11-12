//! PCM WAV in and out at 48 kHz: 16-bit or 32-bit float, any channel count.

use std::path::Path;

use engine::block::RATE;

/// Every channel of `path`, as f32.
pub fn read(path: &Path) -> Result<Vec<Vec<f32>>, String> {
    let fail = |why: &str| format!("{}: {why}", path.display());
    let bytes = std::fs::read(path).map_err(|e| fail(&e.to_string()))?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(fail("not a WAV file"));
    }
    let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4]));
    let (mut at, mut format) = (12, None);
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let len = u32_at(at + 4) as usize;
        let body = at + 8;
        if id == b"fmt " {
            // 0xFFFE, the extensible header, names its sub-format at offset 24 of the chunk
            let tag = match u16_at(body) {
                0xFFFE => u16_at(body + 24),
                tag => tag,
            };
            format = Some((tag, u16_at(body + 2) as usize, u32_at(body + 4), u16_at(body + 14)));
        }
        if id == b"data" {
            let (tag, channels, rate, bits) = format.ok_or_else(|| fail("no fmt chunk"))?;
            if rate != RATE {
                return Err(fail(&format!("{rate} Hz; convert it to 48 kHz first")));
            }
            let data = &bytes[body..(body + len).min(bytes.len())];
            let samples: Vec<f32> = match (tag, bits) {
                (1, 16) => data
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0)
                    .collect(),
                (1, 24) => data
                    .as_chunks::<3>()
                    .0
                    .iter()
                    .map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0)
                    .collect(),
                (3, 32) => data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .collect(),
                _ => return Err(fail(&format!("format {tag} at {bits} bits is not read here"))),
            };
            let mut out = vec![Vec::with_capacity(samples.len() / channels); channels];
            for frame in samples.chunks_exact(channels) {
                for (channel, sample) in out.iter_mut().zip(frame) {
                    channel.push(*sample);
                }
            }
            return Ok(out);
        }
        at = body + len + (len & 1);
    }
    Err(fail("no data chunk"))
}

/// Mono 16-bit, for a person to listen to.
pub fn write_mono(path: &Path, samples: &[f32]) -> Result<(), String> {
    let data = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data as usize);
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(RATE.to_le_bytes());
    bytes.extend((RATE * 2).to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    for sample in samples {
        bytes.extend(((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}
