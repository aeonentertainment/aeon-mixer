//! The control thread's end of one slot's ingest (docs/design.md 8.4): the feed of a `push`
//! slot, or the socket an `rtp` or `l16` slot listens on, running on the network thread and
//! filling the slot's packet ring. Dropping it closes the port.

use std::sync::Arc;

use engine::ingest::packet::RING_CAPACITY;
use engine::ingest::push::{DEFAULT_CAPACITY_FRAMES, PushProducer, push_voice_with_depth};
use engine::ingest::{Codec, JitterBuffer, PacketSource, SharedStats, SlotStats, packet_ring};
use engine::slot::source::Source;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use transport::receive::Receiver;
use transport::{Network, TaskHandle};

/// 30 ms: the most a push slot plays behind the newest frame it holds (docs/design.md 8.4). Three
/// 10 ms chunks, room for a host two chunks late; a default until `measure-latency` gives the push
/// path a number.
pub const PUSH_SLOT_DEPTH: usize = 30 * engine::block::RATE as usize / 1000;

pub enum Ingest {
    Push(PushProducer),
    Packets { _receiver: TaskHandle, port: u16, stats: Arc<SharedStats> },
}

impl Ingest {
    /// Builds a slot's source for `kind` and keeps this end of it. An `rtp` or `l16` slot binds
    /// a port of the OS's choice and joins the network thread here, so a failure is the caller's
    /// to see before the mixer exists.
    pub fn open(
        kind: &str,
        playout_delay_frames: u32,
        network: &Network,
    ) -> Result<(Self, Box<dyn Source>)> {
        match kind {
            "push" => {
                let (feed, voice) = push_voice_with_depth(DEFAULT_CAPACITY_FRAMES, PUSH_SLOT_DEPTH);
                Ok((Self::Push(feed), Box::new(voice)))
            }
            "rtp" => {
                let codec = Codec::opus().map_err(|error| Error::from_reason(error.to_string()))?;
                Self::listen(codec, playout_delay_frames, network)
            }
            "l16" => Self::listen(Codec::L16, playout_delay_frames, network),
            other => Err(Error::from_reason(format!("unknown slot kind {other:?}"))),
        }
    }

    fn listen(
        codec: Codec,
        playout_delay_frames: u32,
        network: &Network,
    ) -> Result<(Self, Box<dyn Source>)> {
        let (producer, consumer) = packet_ring(RING_CAPACITY);
        let receiver = Receiver::bind(None, producer)?;
        let port = receiver.port()?;
        let jitter = JitterBuffer::new(playout_delay_frames, codec);
        let stats = jitter.shared_stats();
        let handle = network.add(receiver)?;
        let source = PacketSource::new(consumer, jitter);
        Ok((Self::Packets { _receiver: handle, port, stats }, Box::new(source)))
    }

    /// The UDP port the slot listens on; none for a push slot.
    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Push(_) => None,
            Self::Packets { port, .. } => Some(*port),
        }
    }

    pub fn stats(&self) -> SlotStats {
        match self {
            Self::Push(feed) => feed.stats(),
            Self::Packets { stats, .. } => stats.read(),
        }
    }

    pub fn feed(&mut self) -> Option<&mut PushProducer> {
        match self {
            Self::Push(feed) => Some(feed),
            Self::Packets { .. } => None,
        }
    }
}

/// `SlotStats` as JavaScript reads it.
#[napi(object)]
pub struct SlotStatsInfo {
    pub pushed: u32,
    pub lost: u32,
    pub late: u32,
    pub concealed: u32,
    pub resyncs: u32,
    pub jitter_ms: f64,
}

impl From<SlotStats> for SlotStatsInfo {
    fn from(stats: SlotStats) -> Self {
        Self {
            pushed: stats.pushed,
            lost: stats.lost,
            late: stats.late,
            concealed: stats.concealed,
            resyncs: stats.resyncs,
            jitter_ms: f64::from(stats.jitter_ms),
        }
    }
}
