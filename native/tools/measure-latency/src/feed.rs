use engine::ingest::push::PushProducer;
use engine::ingest::{CaptureProducer, PacketProducer};

use crate::packets::Timed;

/// What stands in for the network thread, the host or an input's capture thread: it hands a slot
/// what has arrived, at the top of each block, from what was prepared before the stream began. It
/// allocates nothing, so it can run in the render it feeds, on a device's audio thread too.
pub enum Feed {
    Packets { producer: PacketProducer, packets: Vec<Timed>, next: usize },
    Push { producer: PushProducer, stereo: Vec<f32>, chunk: usize, from: u64, next: usize },
    Capture { producer: CaptureProducer, mono: Vec<f32>, period: usize, from: u64, next: usize },
}

impl Feed {
    /// Hands over everything that arrived before output frame `now`, the top of this block.
    pub fn before_block(&mut self, now: u64) {
        match self {
            Self::Packets { producer, packets, next } => {
                while let Some(timed) = packets.get(*next).filter(|timed| timed.at < now) {
                    producer.push(&timed.packet);
                    *next += 1;
                }
            }
            Self::Push { producer, stereo, chunk, from, next } => {
                while *from + (((*next + 1) * *chunk) as u64) < now {
                    let span = *next * *chunk * 2..(*next + 1) * *chunk * 2;
                    let Some(frames) = stereo.get(span) else { return };
                    producer.push(frames);
                    *next += 1;
                }
            }
            Self::Capture { producer, mono, period, from, next } => {
                while *from + (((*next + 1) * *period) as u64) < now {
                    let Some(frames) = mono.get(*next * *period..(*next + 1) * *period) else {
                        return;
                    };
                    producer.push(frames);
                    *next += 1;
                }
            }
        }
    }
}
