//! The same packets with nothing in front of the codec: libopus straight, or the L16 fold. Each
//! packet is decoded at its RTP timestamp, counted from the first packet's, by one decoder in
//! timestamp order, and time no packet covers is the decoder's own concealment, or silence for
//! L16. A new SSRC is a new sender: its packets follow on, through a new decoder. This is a
//! receiver with all the time in the world, so a slot that plays quieter than this lost the
//! level somewhere in the engine.

use engine::block::RATE;

use crate::stream::Arrival;

/// libopus works in 2.5 ms steps and decodes at most 120 ms at once.
const STEP: usize = 120;
const MOST: usize = 5760;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Opus,
    L16,
}

pub fn decode(arrivals: &[Arrival], kind: Kind) -> Result<Vec<f32>, String> {
    let mut out = Vec::new();
    let mut start = 0;
    while start < arrivals.len() {
        let ssrc = arrivals[start].packet.ssrc;
        let end = arrivals[start..]
            .iter()
            .position(|a| a.packet.ssrc != ssrc)
            .map_or(arrivals.len(), |n| start + n);
        decode_sender(&arrivals[start..end], kind, &mut out)?;
        start = end;
    }
    Ok(out)
}

fn decode_sender(arrivals: &[Arrival], kind: Kind, out: &mut Vec<f32>) -> Result<(), String> {
    // Timestamps unwrapped in arrival order, then sorted: through a DTX pause the sequence holds
    // while the timestamp runs on, so the timestamp is the order that places a packet
    let mut placed = Vec::with_capacity(arrivals.len());
    let (mut last, mut extended) = (arrivals[0].packet.timestamp, 0i64);
    for arrival in arrivals {
        extended += i64::from(arrival.packet.timestamp.wrapping_sub(last) as i32);
        last = arrival.packet.timestamp;
        placed.push((extended, &arrival.packet));
    }
    placed.sort_by_key(|(ts, _)| *ts);
    placed.dedup_by_key(|(ts, _)| *ts);

    let mut opus = match kind {
        Kind::Opus => {
            Some(opus::Decoder::new(RATE, opus::Channels::Mono).map_err(|e| e.to_string())?)
        }
        Kind::L16 => None,
    };
    let mut frame = vec![0.0f32; MOST];
    let base = out.len() as i64 - placed[0].0;
    for (ts, packet) in placed {
        // A packet that starts inside what is already decoded overlaps it, and is dropped
        let Ok(mut gap) = usize::try_from(base + ts - out.len() as i64) else { continue };
        while gap > 0 {
            let n = (gap / STEP * STEP).min(MOST);
            let written = match (&mut opus, n) {
                (Some(decoder), n) if n > 0 => {
                    decoder.decode_float(&[], &mut frame[..n], false).unwrap_or(0)
                }
                _ => 0,
            };
            if written == 0 {
                out.resize(out.len() + gap, 0.0);
                break;
            }
            out.extend_from_slice(&frame[..written]);
            gap -= written;
        }
        let written = match &mut opus {
            Some(decoder) => decoder.decode_float(packet.payload(), &mut frame, false).unwrap_or(0),
            None => fold_l16(packet.payload(), &mut frame),
        };
        out.extend_from_slice(&frame[..written]);
    }
    Ok(())
}

/// Stereo 16-bit big endian, folded to mono by the mean of the two channels.
fn fold_l16(payload: &[u8], out: &mut [f32]) -> usize {
    let frames = (payload.len() / 4).min(out.len());
    for (sample, bytes) in out.iter_mut().zip(payload.as_chunks::<4>().0) {
        let left = i16::from_be_bytes([bytes[0], bytes[1]]);
        let right = i16::from_be_bytes([bytes[2], bytes[3]]);
        *sample = (f32::from(left) + f32::from(right)) / 65_536.0;
    }
    frames
}
