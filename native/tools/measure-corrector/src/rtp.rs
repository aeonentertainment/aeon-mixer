//! What a phone does to a voice before the slot's jitter buffer: Opus at WebRTC's defaults for
//! the app's mono voice, in packets on the output clock, over a LAN that delays, reorders and
//! loses them. Everything here is deterministic for a seed.

use engine::block::{BLOCK, RATE};
use engine::ingest::Packet;

use crate::music::Lcg;

/// 15 ms: the design's playout delay.
pub const PLAYOUT_DELAY: u32 = RATE * 15 / 1000;

/// 20 ms, what mediasoup sends. A phone's WebRTC stack packs Opus this way unless told otherwise,
/// and `useAudioProducer.ts` does not tell it.
pub const FRAME: usize = 960;

/// libwebrtc's default for mono Opus. `useAudioProducer.ts` sets no bitrate.
const BITRATE: i32 = 32_000;

/// A payload this short is DTX with nothing in it. WebRTC sends the first and drops the rest.
const DTX_PAYLOAD: usize = 2;

/// A packet and the output frame it reaches the socket at.
#[derive(Clone, Copy)]
pub struct Arrival {
    pub at: i64,
    pub packet: Packet,
}

/// How a LAN mistreats packets, in percent of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Impairment {
    pub loss: u32,
    pub spikes: u32,
    pub swaps: u32,
}

impl Impairment {
    pub const NONE: Self = Self { loss: 0, spikes: 0, swaps: 0 };
    /// What docs/design.md 8.4's done-when asks a stream to survive.
    pub const WIFI: Self = Self { loss: 5, spikes: 3, swaps: 5 };
}

pub struct Encoded {
    pub arrivals: Vec<Arrival>,
    /// How late the clip plays against its own time once it leaves the slot's source.
    pub lag_frames: i64,
    /// The jitter buffer's claim for the same delay, which the guide clock subtracts.
    pub claimed_frames: i64,
}

/// `clip` through a phone's encoder and the LAN, `dtx` being what `useAudioProducer.ts` asks for
/// unless a row is finding out what it costs. The lag is what docs/design.md 8.4 builds: the
/// block after the first packet arrives plus the playout delay, plus the encoder's lookahead,
/// which no one at the receiving end can know.
pub fn encode(clip: &[f32], impairment: Impairment, dtx: bool, seed: u64) -> Encoded {
    let mut encoder = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip)
        .expect("the encoder could not be made");
    encoder.set_bitrate(opus::Bitrate::Bits(BITRATE)).expect("a bitrate Opus takes");
    encoder.set_inband_fec(true).expect("FEC on");
    encoder.set_dtx(dtx).expect("DTX set");
    encoder.set_packet_loss_perc(impairment.loss as i32).expect("a loss Opus takes");
    let lookahead = i64::from(encoder.get_lookahead().expect("a lookahead"));

    let mut sent = Vec::with_capacity(clip.len() / FRAME + 1);
    let mut payload = [0u8; 1500];
    let mut seq = 1000u16;
    let mut in_dtx = false;
    for (k, pcm) in clip.chunks_exact(FRAME).enumerate() {
        let len = encoder.encode_float(pcm, &mut payload).expect("a frame Opus takes");
        let silent = len <= DTX_PAYLOAD;
        if !(silent && in_dtx) {
            let ts = (k * FRAME) as u32;
            let packet =
                Packet::new(seq, ts, 0xAE01, false, &payload[..len]).expect("under an MTU");
            sent.push(Arrival { at: ((k + 1) * FRAME) as i64, packet });
            seq = seq.wrapping_add(1);
        }
        in_dtx = silent;
    }
    let arrivals = impair(&sent, impairment, seed);
    let anchor = arrivals
        .first()
        .map_or(0, |first| (first.at / BLOCK as i64 + 1) * BLOCK as i64 + i64::from(PLAYOUT_DELAY));
    let first_ts = arrivals.first().map_or(0, |first| i64::from(first.packet.timestamp));
    Encoded {
        arrivals,
        lag_frames: anchor - first_ts + lookahead,
        claimed_frames: i64::from(PLAYOUT_DELAY) + FRAME as i64,
    }
}

/// Delays every packet 5 ms plus up to 2 ms, spikes some by 30 ms, swaps some neighbours, loses
/// some, and sorts what is left by arrival.
fn impair(sent: &[Arrival], impairment: Impairment, seed: u64) -> Vec<Arrival> {
    let mut rng = Lcg(seed);
    let mut arrivals: Vec<Arrival> = Vec::with_capacity(sent.len());
    for (index, sent) in sent.iter().enumerate() {
        // The first packet anchors the stream, which would hide nothing and move everything
        if index > 0 && rng.below(100) < impairment.loss {
            continue;
        }
        let mut delay = 240 + i64::from(rng.below(96));
        if index > 0 && rng.below(100) < impairment.spikes {
            delay += 1440;
        }
        let arrival = Arrival { at: sent.at + delay, ..*sent };
        if index > 1
            && rng.below(100) < impairment.swaps
            && let Some(previous) = arrivals.last_mut()
        {
            let earlier = previous.at;
            previous.at = arrival.at;
            arrivals.push(Arrival { at: earlier, ..arrival });
            continue;
        }
        arrivals.push(arrival);
    }
    arrivals.sort_by_key(|arrival| arrival.at);
    arrivals
}
