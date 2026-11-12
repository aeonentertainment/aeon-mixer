//! Synthetic senders: a known signal turned into the packets a phone or the house-mic sender would
//! put on the wire, with the output frame each one reaches the slot's ring at.

use engine::block::RATE;
use engine::ingest::Packet;

use crate::ingest::Encoder;

const SSRC: u32 = 0x00AE_0401;

/// A packet and the output frame it arrives at. A sender on the output clock with no network: all
/// the delay a real network adds is the host's to know.
#[derive(Clone, Copy)]
pub struct Timed {
    pub at: u64,
    pub packet: Packet,
}

/// Every whole frame of `signal` as an Opus packet, each sent on the frame it ends, the first frame
/// starting at output frame `from`. Also returns the encoder's lookahead in frames: where the
/// decoded signal sits against the source.
pub fn opus(
    signal: &[f32],
    frame: usize,
    encoder: Encoder,
    from: u64,
) -> Result<(Vec<Timed>, usize), opus::Error> {
    let application = match encoder {
        Encoder::Voip => opus::Application::Voip,
        Encoder::RestrictedLowDelay => opus::Application::LowDelay,
    };
    let mut opus = opus::Encoder::new(RATE, opus::Channels::Mono, application)?;
    opus.set_bitrate(opus::Bitrate::Bits(64_000))?;
    opus.set_inband_fec(true)?;
    opus.set_packet_loss_perc(10)?;
    let lookahead = opus.get_lookahead()? as usize;
    let mut payload = [0u8; 1500];
    let packets = timed(signal, frame, from, |pcm, seq, timestamp| {
        let length = opus.encode_float(pcm, &mut payload)?;
        Ok(Packet::new(seq, timestamp, SSRC, false, &payload[..length]).expect("under an MTU"))
    })?;
    Ok((packets, lookahead))
}

/// Every whole frame of `signal` as stereo 16-bit big-endian, both channels the signal, which is
/// what the house-mic sender sends and what the slot folds back to mono.
pub fn l16(signal: &[f32], frame: usize, from: u64) -> Result<Vec<Timed>, opus::Error> {
    timed(signal, frame, from, |pcm, seq, timestamp| {
        let payload: Vec<u8> = pcm
            .iter()
            .flat_map(|sample| {
                let [high, low] = ((sample * 32767.0).round() as i16).to_be_bytes();
                [high, low, high, low]
            })
            .collect();
        Ok(Packet::new(seq, timestamp, SSRC, false, &payload).expect("under an MTU"))
    })
}

fn timed(
    signal: &[f32],
    frame: usize,
    from: u64,
    mut packet: impl FnMut(&[f32], u16, u32) -> Result<Packet, opus::Error>,
) -> Result<Vec<Timed>, opus::Error> {
    signal
        .chunks_exact(frame)
        .enumerate()
        .map(|(k, pcm)| {
            Ok(Timed {
                at: from + ((k + 1) * frame) as u64,
                packet: packet(pcm, k as u16, (k * frame) as u32)?,
            })
        })
        .collect()
}
