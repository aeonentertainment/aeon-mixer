//! The control thread's end of one slot's ingest (docs/design.md 8.4): the feed of a `push`
//! slot, the socket an `rtp` or `l16` slot listens on, running on the network thread and
//! filling the slot's packet ring, which the host may record, or a `capture` slot's input
//! stream. Dropping it closes the port and ends any recording, or stops the stream.

use std::path::Path;
use std::sync::Arc;

use devices::Capturing;
use engine::ingest::packet::RING_CAPACITY;
use engine::ingest::push::{DEFAULT_CAPACITY_FRAMES, PushProducer, push_voice_with_depth};
use engine::ingest::{Codec, JitterBuffer, PacketSource, SharedStats, SlotStats, packet_ring};
use engine::slot::source::Source;
use napi::bindgen_prelude::*;
use transport::receive::Receiver;
use transport::record::{RecordSwitch, Recording};
use transport::{Network, TaskHandle};

/// 30 ms: the most a push slot plays behind the newest frame it holds (docs/design.md 8.4). Three
/// 10 ms chunks, room for a host two chunks late; a default until `measure-latency` gives the push
/// path a number.
pub const PUSH_SLOT_DEPTH: usize = 30 * engine::block::RATE as usize / 1000;

pub enum Ingest {
    Push(PushProducer),
    Packets { _receiver: TaskHandle, port: u16, stats: Arc<SharedStats>, switch: RecordSwitch },
    Capture { _capturing: Capturing, stats: Arc<SharedStats> },
}

impl Ingest {
    /// Builds a slot's source for `kind` and keeps this end of it. An `rtp` or `l16` slot binds
    /// a port of the OS's choice and joins the network thread here, so a failure is the caller's
    /// to see before the mixer exists.
    /// `playout_delay_frames` pins the delay; without it, each stream chooses its own.
    pub fn open(
        kind: &str,
        playout_delay_frames: Option<u32>,
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

    /// A `capture` slot: starts its input here, so a missing device or channel is the caller's
    /// to see before the mixer exists.
    pub fn capture(
        device: Option<String>,
        channel: u16,
        output_frames: u32,
    ) -> Result<(Self, Box<dyn Source>)> {
        let (capturing, voice) = crate::capture_input::open(device, channel, output_frames)?;
        let stats = voice.shared_stats();
        Ok((Self::Capture { _capturing: capturing, stats }, Box::new(voice)))
    }

    fn listen(
        codec: Codec,
        playout_delay_frames: Option<u32>,
        network: &Network,
    ) -> Result<(Self, Box<dyn Source>)> {
        let (producer, consumer) = packet_ring(RING_CAPACITY);
        let receiver = Receiver::bind(None, producer)?;
        let port = receiver.port()?;
        let jitter = match playout_delay_frames {
            Some(frames) => JitterBuffer::new(frames, codec),
            None => JitterBuffer::chosen(codec),
        };
        let stats = jitter.shared_stats();
        let switch = receiver.switch();
        let handle = network.add(receiver)?;
        let source = PacketSource::new(consumer, jitter);
        Ok((Self::Packets { _receiver: handle, port, stats, switch }, Box::new(source)))
    }

    /// Records every datagram the slot's port reads into a new pcap file at `path`, or stops for
    /// `None` (docs/design.md 8.4). The file and its header are written here, so a bad path is
    /// this call's error; the writer thread finishes the file on its own once the recording ends.
    pub fn record(&self, path: Option<&str>) -> Result<()> {
        let Self::Packets { port, switch, .. } = self else {
            return Err(Error::from_reason("only an rtp or l16 slot can be recorded"));
        };
        let tap = path
            .map(|path| {
                Recording::create(Path::new(path), *port).map(|(_finishes_alone, tap)| tap).map_err(
                    |error| Error::from_reason(format!("cannot record to {path}: {error}")),
                )
            })
            .transpose()?;
        switch.set(tap);
        Ok(())
    }

    /// The UDP port the slot listens on; none for a push or capture slot.
    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Packets { port, .. } => Some(*port),
            Self::Push(_) | Self::Capture { .. } => None,
        }
    }

    pub fn stats(&self) -> SlotStats {
        match self {
            Self::Push(feed) => feed.stats(),
            Self::Packets { stats, .. } | Self::Capture { stats, .. } => stats.read(),
        }
    }

    pub fn feed(&mut self) -> Option<&mut PushProducer> {
        match self {
            Self::Push(feed) => Some(feed),
            Self::Packets { .. } | Self::Capture { .. } => None,
        }
    }
}
