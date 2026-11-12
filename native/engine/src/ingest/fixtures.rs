//! Synthetic RTP for the ingest tests: senders that encode a known signal into packets, a
//! network that delays, reorders and loses them, and a rig that plays them through the offline
//! driver. Arrival times are on the output clock, in frames.

use std::f32::consts::TAU;
use std::sync::Arc;

use assert_no_alloc::assert_no_alloc;

use super::codec::Codec;
use super::jitter::JitterBuffer;
use super::packet::{Packet, RING_CAPACITY, packet_ring};
use super::source::PacketSource;
use super::stats::{SharedStats, SlotStats};
use crate::audio_thread::{AudioThread, Controls};
use crate::block::{BLOCK, RATE};
use crate::command::{Command, Event};
use crate::driver::OfflineDriver;
use crate::graph::Graph;
use crate::slot::Slot;

/// 15 ms, the design's playout delay.
pub const DELAY: u32 = RATE * 15 / 1000;

/// Three tones with a long common period, under the clipper's knee: a lag search has one peak
/// and Opus codes it faithfully.
pub fn source(frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            [196.0f32, 311.0, 466.0].iter().map(|hz| 0.15 * (TAU * hz * t).sin()).sum()
        })
        .collect()
}

/// Something SILK takes for speech: a harmonic buzz whose pitch glides, in 300 ms bursts with
/// 150 ms of silence between, so its voice detector stays on and the encoder sends in-band FEC.
pub fn voice(frames: usize) -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..frames)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let in_burst = (t % 0.45) < 0.3;
            let edge = ((t % 0.45).min(0.3 - (t % 0.45)) / 0.02).clamp(0.0, 1.0);
            let envelope = if in_burst { edge } else { 0.0 };
            phase = (phase + (120.0 + 60.0 * (t * 3.0).sin()) / RATE as f32) % 1.0;
            let sample: f32 = (1..=10).map(|n| (TAU * n as f32 * phase).sin() / n as f32).sum();
            0.2 * envelope * sample
        })
        .collect()
}

/// Singing as a phone hears it: `voice` in 1.5 s phrases with 0.8 s breaths, long enough for
/// DTX to start, over a faint noise floor at about -65 dBFS.
pub fn phrases(frames: usize) -> Vec<f32> {
    let mut rng = Lcg(11);
    voice(frames)
        .into_iter()
        .enumerate()
        .map(|(i, sample)| {
            let in_phrase = (i as f32 / RATE as f32) % 2.3 < 1.5;
            let noise = (rng.below(2001) as f32 - 1000.0) * 1e-6;
            if in_phrase { sample + noise } else { noise }
        })
        .collect()
}

/// A packet and the output frame it reaches the socket at.
#[derive(Clone, Copy)]
pub struct Arrival {
    pub at: i64,
    pub packet: Packet,
}

pub struct OpusSender {
    encoder: opus::Encoder,
    pub frame: usize,
    /// The encoder's delay: where the decoded signal sits against the source.
    pub lookahead: usize,
    seq: u16,
    ts: u32,
    ssrc: u32,
    /// With DTX on, whether the last frame was already a DTX frame.
    in_dtx: Option<bool>,
}

impl OpusSender {
    /// A phone's encoder: voice mode, 64 kb/s, in-band FEC for 10 percent loss.
    pub fn new(frame: usize, ssrc: u32) -> Self {
        Self::at_bitrate(frame, ssrc, 64_000)
    }

    pub fn at_bitrate(frame: usize, ssrc: u32, bits: i32) -> Self {
        let mut encoder =
            opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        encoder.set_bitrate(opus::Bitrate::Bits(bits)).unwrap();
        encoder.set_inband_fec(true).unwrap();
        encoder.set_packet_loss_perc(10).unwrap();
        let lookahead = encoder.get_lookahead().unwrap() as usize;
        Self { encoder, frame, lookahead, seq: 65_500, ts: 4_000_000_000, ssrc, in_dtx: None }
    }

    /// DTX on, sent the way libwebrtc sends it, as a phone does: a packet of two bytes or less
    /// is a DTX frame, and only the first of a run goes out (`WebRtcOpus_Encode`). libopus still
    /// codes a comfort-noise update every 400 ms, which goes out.
    pub fn with_dtx(mut self) -> Self {
        self.encoder.set_dtx(true).unwrap();
        self.in_dtx = Some(false);
        self
    }

    /// Encodes one frame of signal into the next packet, or `None` for a DTX frame the sender
    /// keeps back: its clock runs on and its sequence does not.
    pub fn packet(&mut self, pcm: &[f32]) -> Option<Packet> {
        assert_eq!(pcm.len(), self.frame);
        let mut payload = [0u8; 1500];
        let len = self.encoder.encode_float(pcm, &mut payload).unwrap();
        if let Some(in_dtx) = &mut self.in_dtx {
            let was = std::mem::replace(in_dtx, len <= 2);
            if was && len <= 2 {
                self.pause(self.frame as u32);
                return None;
            }
        }
        let packet = Packet::new(self.seq, self.ts, self.ssrc, false, &payload[..len]).unwrap();
        self.seq = self.seq.wrapping_add(1);
        self.ts = self.ts.wrapping_add(self.frame as u32);
        Some(packet)
    }

    /// The sender paused: its clock ran on, its sequence did not.
    pub fn pause(&mut self, frames: u32) {
        self.ts = self.ts.wrapping_add(frames);
    }

    /// Every frame of `signal` the sender sends, as a packet sent on the frame it ends, from
    /// `from`.
    pub fn send_all(&mut self, signal: &[f32], from: i64) -> Vec<Arrival> {
        let frame = self.frame;
        signal
            .chunks_exact(frame)
            .enumerate()
            .filter_map(|(k, pcm)| {
                let packet = self.packet(pcm)?;
                Some(Arrival { at: from + ((k + 1) * frame) as i64, packet })
            })
            .collect()
    }
}

/// Stereo 16-bit big-endian, both channels the signal, so the fold gives it back.
pub struct L16Sender {
    pub frame: usize,
    pub seq: u16,
    ts: u32,
    pub ssrc: u32,
}

impl L16Sender {
    pub fn new(frame: usize, ssrc: u32) -> Self {
        Self { frame, seq: 10, ts: 500, ssrc }
    }

    pub fn packet(&mut self, pcm: &[f32]) -> Packet {
        assert_eq!(pcm.len(), self.frame);
        let mut payload = Vec::with_capacity(pcm.len() * 4);
        for sample in pcm {
            let bytes = ((sample * 32767.0).round() as i16).to_be_bytes();
            payload.extend_from_slice(&bytes);
            payload.extend_from_slice(&bytes);
        }
        let packet = Packet::new(self.seq, self.ts, self.ssrc, false, &payload).unwrap();
        self.seq = self.seq.wrapping_add(1);
        self.ts = self.ts.wrapping_add(self.frame as u32);
        packet
    }

    pub fn pause(&mut self, frames: u32) {
        self.ts = self.ts.wrapping_add(frames);
    }

    /// Packets on the frame each ends, with the sender's clock scaled by `rate`: 1.001 is a
    /// sender a tenth of a percent fast.
    pub fn send_all(&mut self, signal: &[f32], from: i64, rate: f64) -> Vec<Arrival> {
        signal
            .chunks_exact(self.frame)
            .enumerate()
            .map(|(k, pcm)| Arrival {
                at: from + (((k + 1) * self.frame) as f64 / rate).round() as i64,
                packet: self.packet(pcm),
            })
            .collect()
    }
}

/// What `signal` looks like after the L16 sender's 16-bit rounding.
pub fn quantized(signal: &[f32]) -> Vec<f32> {
    signal.iter().map(|s| (s * 32767.0).round() / 32768.0).collect()
}

/// How a LAN mistreats packets, in percent of them.
#[derive(Clone, Copy)]
pub struct Impairment {
    pub loss: u32,
    pub spikes: u32,
    pub swaps: u32,
}

impl Impairment {
    pub const NONE: Self = Self { loss: 0, spikes: 0, swaps: 0 };
    pub const WIFI: Self = Self { loss: 5, spikes: 3, swaps: 5 };
}

/// Delays every packet 5 ms plus up to 2 ms, spikes some by 30 ms, swaps some neighbours,
/// loses some, and sorts what is left by arrival. Deterministic for a seed.
pub fn impair(sent: &[Arrival], impairment: Impairment, seed: u64) -> Vec<Arrival> {
    let mut rng = Lcg(seed);
    let mut arrivals: Vec<Arrival> = Vec::new();
    for sent in sent {
        if rng.percent() < impairment.loss {
            continue;
        }
        let mut delay = 240 + rng.below(96) as i64;
        if rng.percent() < impairment.spikes {
            delay += 1440;
        }
        let arrival = Arrival { at: sent.at + delay, ..*sent };
        if rng.percent() < impairment.swaps
            && let Some(previous) = arrivals.last_mut()
        {
            let earlier = previous.at;
            previous.at = arrival.at;
            arrivals.push(Arrival { at: earlier, ..arrival });
            continue;
        }
        arrivals.push(arrival);
    }
    arrivals.sort_by_key(|a| a.at);
    arrivals
}

/// A link with a known spread: every packet takes 1 ms plus up to `jitter` frames, and every
/// `every` frames, give or take a quarter, a stall holds whatever would arrive during it for
/// `stall` frames, the way a busy radio does. Every packet arrives, in order.
#[derive(Clone, Copy)]
pub struct Stalls {
    pub stall: i64,
    pub jitter: i64,
    pub every: i64,
}

impl Stalls {
    pub fn new(stall: i64, every: i64) -> Self {
        Self { stall, jitter: 48, every }
    }

    pub fn apply(&self, sent: &[Arrival], seed: u64) -> Vec<Arrival> {
        let mut rng = Lcg(seed);
        let quarter = self.every as u32 / 4;
        let mut stalls_at = sent.first().map_or(0, |first| first.at) + self.every;
        sent.iter()
            .map(|sent| {
                let mut at = sent.at + 48 + i64::from(rng.below(self.jitter as u32 + 1));
                if at >= stalls_at + self.stall {
                    stalls_at +=
                        self.every - i64::from(quarter) + i64::from(rng.below(2 * quarter));
                }
                if (stalls_at..stalls_at + self.stall).contains(&at) {
                    at = stalls_at + self.stall;
                }
                Arrival { at, ..*sent }
            })
            .collect()
    }
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }

    fn percent(&mut self) -> u32 {
        self.below(100)
    }
}

/// The output frame the first packet of `arrivals` is anchored at: the block after it arrives,
/// plus the playout delay.
pub fn anchor_of(arrivals: &[Arrival]) -> i64 {
    (arrivals[0].at / BLOCK as i64 + 1) * BLOCK as i64 + i64::from(DELAY)
}

/// A slot on the offline driver fed from a packet ring, panned hard left so the left channel
/// is the slot's mono output at unit gain.
pub struct Rig {
    driver: OfflineDriver,
    controls: Controls,
    producer: super::packet::PacketProducer,
    stats: Arc<SharedStats>,
    block: Vec<f32>,
    pub out: Vec<f32>,
    /// The slot's meter, one RMS reading in dBFS per 100 ms: the host's `inputDb`.
    pub levels: Vec<f32>,
    pumped: i64,
}

impl Rig {
    pub fn new(codec: Codec) -> Self {
        Self::around(JitterBuffer::new(DELAY, codec))
    }

    /// A slot that chooses its own delay.
    pub fn choosing(codec: Codec) -> Self {
        Self::around(JitterBuffer::chosen(codec))
    }

    fn around(jitter: JitterBuffer) -> Self {
        let (producer, consumer) = packet_ring(RING_CAPACITY);
        let stats = jitter.shared_stats();
        let source = PacketSource::new(consumer, jitter);
        let graph = Box::new(Graph::new(vec![Slot::new(Box::new(source))]));
        let (audio, mut controls) = AudioThread::new();
        controls.commands.push(Command::Swap { id: 1, graph }).ok().unwrap();
        controls.params.update(|p| p.slots[0].pan = -1.0);
        Self {
            driver: OfflineDriver::new(audio.into_render()),
            controls,
            producer,
            stats,
            block: vec![0.0; 2 * BLOCK],
            out: Vec::new(),
            levels: Vec::new(),
            pumped: 0,
        }
    }

    pub fn reset_slot(&mut self) {
        self.controls.commands.push(Command::ResetSlot { slot: 0 }).ok().unwrap();
    }

    /// Pumps whole blocks up to `until`, handing each packet to the ring in the block after it
    /// arrives.
    pub fn run(&mut self, arrivals: &[Arrival], until: i64) {
        let pumped = self.pumped;
        let mut next = arrivals.iter().skip_while(|a| a.at < pumped).peekable();
        while self.pumped + BLOCK as i64 <= until {
            let pumped = self.pumped;
            while let Some(arrival) = next.next_if(|a| a.at < pumped) {
                assert!(self.producer.push(&arrival.packet), "the ring never fills in a test");
            }
            let Self { driver, block, .. } = self;
            assert_no_alloc(|| driver.pump_into(block));
            self.out.extend(block.iter().step_by(2));
            while let Some(event) = self.controls.events.pop() {
                if let Event::Level { rms_db, .. } = event {
                    self.levels.push(rms_db);
                }
            }
            self.pumped += BLOCK as i64;
        }
    }

    pub fn stats(&self) -> SlotStats {
        self.stats.read()
    }

    /// The slot's output from `from`, per frame of `frame`: the error energy against `other`
    /// over the signal energy, for finding where two runs part.
    pub fn frame_errors(&self, other: &[f32], from: i64, frame: usize, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|k| {
                let start = from as usize + k * frame;
                let (error, signal) = (start..start + frame)
                    .map(|i| ((self.out[i] - other[i]).powi(2), other[i].powi(2)))
                    .fold((0.0, 0.0), |(e, s), (x, y)| (e + x, s + y));
                error / signal
            })
            .collect()
    }
}

/// One packet through a jitter buffer with no graph around it: pulls block by block under the
/// guard, feeding the ring in the block after each arrival, and returns the mono output.
pub fn pull_direct(jitter: &mut JitterBuffer, arrivals: &[Arrival], until: i64) -> Vec<f32> {
    let mut out = Vec::new();
    let mut block = [0.0f32; BLOCK];
    let mut next = arrivals.iter().peekable();
    let mut pumped = 0i64;
    while pumped + BLOCK as i64 <= until {
        while let Some(arrival) = next.next_if(|a| a.at < pumped) {
            jitter.push(&arrival.packet);
        }
        assert_no_alloc(|| jitter.pull(&mut block));
        out.extend_from_slice(&block);
        pumped += BLOCK as i64;
    }
    out
}

/// The lag, searched within `search` of `expected`, at which `out` best matches `source`, and
/// the error energy over the signal energy there. `source` is compared from `from`.
pub fn compare(source: &[f32], from: usize, out: &[f32], expected: i64, search: i64) -> Fit {
    let mut best = Fit { lag: expected, error: f32::INFINITY };
    for lag in expected - search..=expected + search {
        let mut error = 0.0f32;
        let mut signal = 0.0f32;
        for (i, s) in source.iter().enumerate().skip(from) {
            let Some(o) = usize::try_from(i as i64 + lag).ok().and_then(|at| out.get(at)) else {
                continue;
            };
            error += (o - s) * (o - s);
            signal += s * s;
        }
        let error = error / signal;
        if error < best.error {
            best = Fit { lag, error };
        }
    }
    best
}

#[derive(Clone, Copy, Debug)]
pub struct Fit {
    pub lag: i64,
    pub error: f32,
}

impl Fit {
    pub fn snr_db(&self) -> f32 {
        -10.0 * self.error.log10()
    }
}
