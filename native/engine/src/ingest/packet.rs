//! The fixed-size packet the network thread hands the audio thread, and the SPSC ring it
//! travels in. One ring per slot: the network thread produces, the audio thread consumes at
//! the top of each block.

/// The largest payload a LAN MTU carries. A longer datagram is dropped at the socket.
pub const MAX_PAYLOAD: usize = 1500;

/// Packets a slot's ring holds: 160 ms of 2.5 ms frames, so the audio thread can miss a few
/// blocks before a packet is lost.
pub const RING_CAPACITY: usize = 64;

/// One RTP packet, header fields unpacked, payload still encoded. `Copy`, so the ring and the
/// jitter buffer's store move it by memcpy and allocate nothing.
#[derive(Clone, Copy)]
pub struct Packet {
    pub seq: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub marker: bool,
    pub len: u16,
    pub payload: [u8; MAX_PAYLOAD],
}

impl Packet {
    /// Copies `payload` in; `None` when it is longer than an MTU.
    pub fn new(seq: u16, timestamp: u32, ssrc: u32, marker: bool, payload: &[u8]) -> Option<Self> {
        if payload.len() > MAX_PAYLOAD {
            return None;
        }
        let mut packet = Self {
            seq,
            timestamp,
            ssrc,
            marker,
            len: payload.len() as u16,
            payload: [0; MAX_PAYLOAD],
        };
        packet.payload[..payload.len()].copy_from_slice(payload);
        Some(packet)
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload[..usize::from(self.len)]
    }
}

pub fn packet_ring(capacity: usize) -> (PacketProducer, PacketConsumer) {
    let (producer, consumer) = rtrb::RingBuffer::new(capacity);
    (PacketProducer(producer), PacketConsumer(consumer))
}

pub struct PacketProducer(rtrb::Producer<Packet>);

impl PacketProducer {
    /// False when the ring is full: the audio thread has stalled, and the packet is lost the way
    /// the network would lose it.
    pub fn push(&mut self, packet: &Packet) -> bool {
        self.0.push(*packet).is_ok()
    }
}

pub struct PacketConsumer(rtrb::Consumer<Packet>);

impl PacketConsumer {
    /// Hands the oldest packet to `take` in place and releases it, so it is copied once, into
    /// wherever `take` puts it. False when the ring is empty.
    pub fn pop_with(&mut self, take: impl FnOnce(&Packet)) -> bool {
        let Ok(chunk) = self.0.read_chunk(1) else { return false };
        let (first, _) = chunk.as_slices();
        take(&first[0]);
        chunk.commit_all();
        true
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    #[test]
    fn a_ring_moves_packets_in_order_and_drops_when_full() {
        let (mut producer, mut consumer) = packet_ring(2);
        let packet = |seq| Packet::new(seq, 0, 1, false, &[seq as u8; 3]).unwrap();
        assert_no_alloc(|| {
            assert!(producer.push(&packet(1)));
            assert!(producer.push(&packet(2)));
            assert!(!producer.push(&packet(3)), "full: the packet is dropped");
            let mut seen = [0u16; 2];
            assert!(consumer.pop_with(|p| seen[0] = p.seq));
            assert!(consumer.pop_with(|p| seen[1] = p.seq));
            assert!(!consumer.pop_with(|_| unreachable!()));
            assert_eq!(seen, [1, 2]);
        });
        assert_eq!(packet(7).payload(), &[7, 7, 7]);
        assert!(Packet::new(0, 0, 0, false, &[0; MAX_PAYLOAD + 1]).is_none());
    }
}
