use engine::audio_thread::{AudioThread, Controls};
use engine::block::{BLOCK, RATE, Stereo};
use engine::command::Command;
use engine::driver::Render;
use engine::graph::Graph;
use engine::ingest::packet::RING_CAPACITY;
use engine::ingest::push::{DEFAULT_CAPACITY_FRAMES, push_voice_with_depth};
use engine::ingest::{Codec, JitterBuffer, PacketSource, packet_ring};
use engine::slot::Slot;
use engine::slot::source::Source;

use crate::feed::Feed;
use crate::ingest::Ingest;
use crate::packets;

/// The most a push slot plays behind the newest frame it holds, as `node/src/ingest.rs` sets it.
const PUSH_DEPTH: usize = RATE as usize * 30 / 1000;

/// The real mic path around one slot: its ingest, jitter buffer and decoder, the slot's chain and
/// the master, on a real `AudioThread`. The slot is panned hard left at unit gain, so the left
/// channel is the slot's mono output. `render` feeds the slot and renders, so whichever clock
/// calls it, the offline driver or a device, is the clock the whole path runs on.
pub struct Rig {
    pub render: Render,
    /// The control thread's ends, kept alive as long as the render is.
    pub controls: Controls,
    /// The encoder's lookahead in frames, which the source pays and the engine does not.
    pub lookahead: usize,
}

impl Rig {
    /// `signal` starts `phase` frames past a block boundary, as a stream starts wherever it likes.
    pub fn new(
        ingest: Ingest,
        playout_frames: u32,
        signal: &[f32],
        phase: u64,
    ) -> Result<Self, opus::Error> {
        let (source, mut feed, lookahead) = wire(ingest, playout_frames, signal, phase)?;
        let graph = Box::new(Graph::new(vec![Slot::new(source)]));
        let (mut audio, mut controls) = AudioThread::new();
        controls
            .commands
            .push(Command::Swap { id: 1, graph })
            .ok()
            .expect("a new command ring has room");
        controls.params.update(|params| params.slots[0].pan = -1.0);
        let mut now = 0u64;
        let render: Render = Box::new(move |out: Stereo<'_>| {
            feed.before_block(now);
            now += BLOCK as u64;
            audio.render(out);
        });
        Ok(Self { render, controls, lookahead })
    }
}

fn wire(
    ingest: Ingest,
    playout_frames: u32,
    signal: &[f32],
    phase: u64,
) -> Result<(Box<dyn Source>, Feed, usize), opus::Error> {
    let packet_source = |codec: Codec, packets| {
        let (producer, consumer) = packet_ring(RING_CAPACITY);
        let source = PacketSource::new(consumer, JitterBuffer::new(playout_frames, codec));
        let feed = Feed::Packets { producer, packets, next: 0 };
        (Box::new(source) as Box<dyn Source>, feed)
    };
    Ok(match ingest {
        Ingest::Rtp { frame, encoder } => {
            let (packets, lookahead) = packets::opus(signal, frame, encoder, phase)?;
            let (source, feed) = packet_source(Codec::opus()?, packets);
            (source, feed, lookahead)
        }
        Ingest::L16 { frame } => {
            let (source, feed) = packet_source(Codec::L16, packets::l16(signal, frame, phase)?);
            (source, feed, 0)
        }
        Ingest::Push { chunk } => {
            let (producer, voice) = push_voice_with_depth(DEFAULT_CAPACITY_FRAMES, PUSH_DEPTH);
            let stereo = signal.iter().flat_map(|sample| [*sample, *sample]).collect();
            let feed = Feed::Push { producer, stereo, chunk, from: phase, next: 0 };
            (Box::new(voice), feed, 0)
        }
    })
}
