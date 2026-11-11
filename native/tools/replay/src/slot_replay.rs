//! The real chain on the offline driver: the packet ring, a `PacketSource` around the jitter
//! buffer, one `Slot` in a `Graph`, the `AudioThread` that drains commands and fades the swap,
//! and `OfflineDriver` as the clock. Each packet goes into the ring before the first block that
//! starts after it arrived, as the network thread and the next callback would have it. The slot
//! is panned hard left at unit gain, so the left channel is the slot's own output.

use assert_no_alloc::assert_no_alloc;
use engine::audio_thread::AudioThread;
use engine::block::{BLOCK, RATE};
use engine::command::{Command, Event};
use engine::driver::OfflineDriver;
use engine::graph::Graph;
use engine::ingest::packet::RING_CAPACITY;
use engine::ingest::{Codec, JitterBuffer, PacketSource, SlotStats, packet_ring};
use engine::slot::Slot;

use crate::stream::Arrival;

/// What the slot did with a stream.
pub struct Replayed {
    /// The slot's mono output, from the first block.
    pub out: Vec<f32>,
    /// The meter's RMS readings, as `level` events carry them.
    pub meter_db: Vec<f32>,
    pub stats: SlotStats,
    /// Packets the ring had no room for, which live would have lost too.
    pub ring_full: usize,
}

/// The frame the first packet's first sample plays on: the block after it arrived, plus the
/// delay (docs/design.md 8.4). Until a resync moves it, every packet plays at this anchor plus
/// its timestamp's distance from the first one's, which is where the reference puts it.
pub fn anchor(arrivals: &[Arrival], playout_delay_frames: u32) -> usize {
    let first = arrivals.first().map_or(0, |a| a.at);
    let block = usize::try_from(first).unwrap_or(0) / BLOCK + 1;
    block * BLOCK + playout_delay_frames as usize
}

/// Plays `arrivals` through a slot with the jitter buffer pinned at `playout_delay_frames`, as
/// Aeon Karaoke pins it, and on for `tail` frames after the last arrival.
pub fn replay(
    arrivals: &[Arrival],
    codec: Codec,
    playout_delay_frames: u32,
    tail: i64,
) -> Replayed {
    let (mut producer, consumer) = packet_ring(RING_CAPACITY);
    let jitter = JitterBuffer::new(playout_delay_frames, codec);
    let stats = jitter.shared_stats();
    let graph =
        Box::new(Graph::new(vec![Slot::new(Box::new(PacketSource::new(consumer, jitter)))]));
    let (audio, mut controls) = AudioThread::new();
    controls.commands.push(Command::Swap { id: 1, graph }).ok().expect("an empty command ring");
    controls.params.update(|params| {
        params.slots[0].pan = -1.0;
        params.slots[0].gain = 1.0;
    });
    let mut driver = OfflineDriver::new(audio.into_render());

    let until = arrivals.last().map_or(0, |last| last.at) + tail;
    let blocks = usize::try_from(until).unwrap_or(0).div_ceil(BLOCK) + 1;
    let mut replayed = Replayed {
        out: Vec::with_capacity(blocks * BLOCK),
        meter_db: Vec::with_capacity(blocks * BLOCK / (RATE as usize / 10) + 1),
        stats: SlotStats::default(),
        ring_full: 0,
    };
    let mut block = [0.0f32; 2 * BLOCK];
    let mut next = arrivals.iter().peekable();
    for k in 0..blocks {
        let pumped = (k * BLOCK) as i64;
        while let Some(arrival) = next.next_if(|arrival| arrival.at < pumped) {
            if !producer.push(&arrival.packet) {
                replayed.ring_full += 1;
            }
        }
        assert_no_alloc(|| driver.pump_into(&mut block));
        replayed.out.extend(block.iter().step_by(2));
        while let Some(event) = controls.events.pop() {
            if let Event::Level { slot: 0, rms_db, .. } = event {
                replayed.meter_db.push(rms_db);
            }
        }
    }
    replayed.stats = stats.read();
    replayed
}
