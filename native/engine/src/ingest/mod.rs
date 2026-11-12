//! How a slot's audio arrives (docs/design.md 8.4). The network thread parses RTP into a
//! fixed-size `Packet` and pushes it into the slot's SPSC ring; the audio thread drains the ring
//! into the slot's jitter buffer at the top of each block and decodes what falls due. A `push`
//! slot skips all of that: the control thread writes stereo f32 into a ring the slot reads, and a
//! `capture` slot's ring is filled by a device's capture thread.

pub mod capture;
#[cfg(test)]
mod capture_edges;
pub mod capture_producer;
pub mod codec;
pub mod delay_chooser;
pub mod fifo;
pub mod jitter;
pub mod l16;
pub mod opus;
pub mod packet;
pub mod push;
pub mod source;
pub mod stats;

#[cfg(test)]
pub(crate) mod fixtures;

pub use capture::{CaptureVoice, capture_voice};
pub use capture_producer::CaptureProducer;
pub use codec::Codec;
pub use jitter::JitterBuffer;
pub use packet::{Packet, PacketConsumer, PacketProducer, packet_ring};
pub use push::{PushProducer, PushVoice, push_voice};
pub use source::PacketSource;
pub use stats::{SharedStats, SlotStats};
