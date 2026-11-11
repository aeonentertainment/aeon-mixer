//! The network thread's end of a recording. The receiver calls `record` after a datagram's
//! packet is in the slot's ring; the tap copies the datagram into a ring allocated when the
//! recording began, and returns. It wakes nobody: the writer drains the ring on its own clock, so
//! recording costs the network thread a copy and no system call. Dropping the tap ends the
//! recording.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use crate::receive::DATAGRAM;

/// One datagram on its way to the writer: fixed-size, so the ring moves it without allocating.
#[derive(Clone, Copy)]
pub(super) struct Record {
    pub at: Instant,
    pub from: SocketAddr,
    pub len: u16,
    pub datagram: [u8; DATAGRAM],
}

pub struct Tap {
    pub(super) records: rtrb::Producer<Record>,
    pub(super) dropped: Arc<AtomicU32>,
}

impl Tap {
    /// Never blocks. When the writer is behind by the whole ring, or has stopped on a write error,
    /// the record is dropped and counted, and the slot's packet is untouched.
    pub fn record(&mut self, datagram: &[u8], from: SocketAddr, at: Instant) {
        let len = datagram.len().min(DATAGRAM);
        let mut record = Record { at, from, len: len as u16, datagram: [0; DATAGRAM] };
        record.datagram[..len].copy_from_slice(&datagram[..len]);
        if self.records.push(record).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}
