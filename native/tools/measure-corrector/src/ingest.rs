use std::sync::Arc;

use engine::ingest::{PacketProducer, SharedStats};

use crate::rtp::Arrival;

pub struct Ingest {
    pub producer: PacketProducer,
    pub arrivals: Vec<Arrival>,
    pub next: usize,
    pub stats: Arc<SharedStats>,
}

impl Ingest {
    /// Hands the ring every packet that reached the socket before this block, as the network
    /// thread would have.
    pub fn deliver(&mut self, pumped: i64) {
        while let Some(arrival) = self.arrivals.get(self.next).filter(|a| a.at < pumped) {
            assert!(self.producer.push(&arrival.packet), "the ring never fills here");
            self.next += 1;
        }
    }
}
