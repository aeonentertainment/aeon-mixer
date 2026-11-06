use engine::ingest::PacketProducer;
use engine::ingest::push::PushProducer;

use crate::packets::Timed;

/// What stands in for the network thread or the host: it hands a slot what has arrived, at the top
/// of each block, from what was prepared before the stream began. It allocates nothing, so it can
/// run in the render it feeds, on a device's audio thread too.
pub enum Feed {
    Packets { producer: PacketProducer, packets: Vec<Timed>, next: usize },
    Push { producer: PushProducer, stereo: Vec<f32>, chunk: usize, from: u64, next: usize },
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
        }
    }
}
