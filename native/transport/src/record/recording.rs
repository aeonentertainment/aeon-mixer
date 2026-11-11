//! The control thread's end of a recording, and the writer thread behind it. `create` opens a
//! new file and writes its header before it returns, so a bad path is the caller's error. The
//! writer wakes every `WAKE` and appends what the tap has left in the ring, one write per
//! record, so a process that dies leaves a file that reads up to its last packet. It ends once
//! the tap is dropped and every record it sent is written.

use std::fs::File;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::pcap;
use super::tap::{Record, Tap};
use crate::receive::DATAGRAM;

/// Records in flight between the tap and the writer: five seconds of 20 ms frames, 0.64 s of
/// 2.5 ms ones, before a stalled disk costs the recording a record.
pub const CAPACITY: usize = 256;

/// How often the writer drains the ring. A wake per record would cost the network thread a
/// system call each; at 2.5 ms frames this leaves eight records in the ring at a time.
pub const WAKE: Duration = Duration::from_millis(20);

/// What a finished recording holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub records: u64,
    /// Records the tap could not hand over: the recording shows them as lost.
    pub dropped: u32,
}

/// Dropping it leaves the writer to finish on its own; `finish` waits for it.
pub struct Recording {
    writer: JoinHandle<io::Result<u64>>,
    dropped: Arc<AtomicU32>,
}

impl Recording {
    /// A new pcap file at `path`, never over an existing one, for datagrams sent to `port`.
    pub fn create(path: &Path, port: u16) -> io::Result<(Self, Tap)> {
        let mut file = File::options().write(true).create_new(true).open(path)?;
        file.write_all(&pcap::global_header())?;
        let started = Instant::now();
        let since_epoch = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let (mut records, mut inbox) = rtrb::RingBuffer::new(CAPACITY);
        // Every slot written once here, so the network thread never takes the page faults
        let blank = Record {
            at: started,
            from: SocketAddr::from(([0; 4], 0)),
            len: 0,
            datagram: [0; DATAGRAM],
        };
        for _ in 0..CAPACITY {
            let _ = records.push(blank);
        }
        while inbox.pop().is_ok() {}
        let dropped = Arc::new(AtomicU32::new(0));
        let writer = thread::Builder::new()
            .name("aeon-mixer-record".into())
            .spawn(move || write_all(file, inbox, port, started, since_epoch))?;
        let tap = Tap { records, dropped: Arc::clone(&dropped) };
        Ok((Self { writer, dropped }, tap))
    }

    /// Waits for the writer, which ends within a `WAKE` of the tap being dropped.
    pub fn finish(self) -> io::Result<Written> {
        let records = self.writer.join().map_err(|_| io::Error::other("the writer panicked"))??;
        Ok(Written { records, dropped: self.dropped.load(Ordering::Relaxed) })
    }
}

fn write_all(
    mut file: File,
    mut inbox: rtrb::Consumer<Record>,
    port: u16,
    started: Instant,
    since_epoch: Duration,
) -> io::Result<u64> {
    let mut bytes = Vec::with_capacity(pcap::FRAMING_LEN + DATAGRAM);
    let mut written = 0;
    loop {
        // Read before draining: a tap gone by now has pushed all it will, so this drain is last
        let ended = inbox.is_abandoned();
        while let Ok(record) = inbox.pop() {
            let source = match record.from {
                SocketAddr::V4(v4) => (*v4.ip(), v4.port()),
                SocketAddr::V6(v6) => (Ipv4Addr::UNSPECIFIED, v6.port()),
            };
            let at = since_epoch + record.at.saturating_duration_since(started);
            bytes.clear();
            let datagram = &record.datagram[..usize::from(record.len)];
            pcap::append_record(&mut bytes, at, source, port, datagram);
            file.write_all(&bytes)?;
            written += 1;
        }
        if ended {
            return Ok(written);
        }
        thread::sleep(WAKE);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use assert_no_alloc::assert_no_alloc;

    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("aeon-record-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        dir
    }

    /// The network thread's whole cost is a copy into the channel: no allocation, no file.
    /// Every record lands in order, with its time, and the file ends when the tap does.
    #[test]
    fn a_tap_records_without_allocating_and_the_file_holds_every_datagram() {
        let path = scratch("tap");
        let (recording, mut tap) = Recording::create(&path, 40_000).unwrap();
        let from = SocketAddr::from(([10, 0, 0, 7], 50_000));
        let base = Instant::now();
        assert_no_alloc(|| {
            for k in 0..100u8 {
                tap.record(&[k; 40], from, base + Duration::from_micros(20_000 * u64::from(k)));
            }
        });
        drop(tap);
        let written = recording.finish().unwrap();
        assert_eq!(written, Written { records: 100, dropped: 0 });

        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(bytes.len(), pcap::GLOBAL_HEADER_LEN + 100 * (pcap::FRAMING_LEN + 40));
        let micros = |k: usize| {
            let at = pcap::GLOBAL_HEADER_LEN + k * (pcap::FRAMING_LEN + 40);
            let field = |o: usize| {
                u64::from(u32::from_le_bytes(bytes[at + o..at + o + 4].try_into().unwrap()))
            };
            (field(0) * 1_000_000 + field(4), bytes[at + pcap::FRAMING_LEN])
        };
        let (first, _) = micros(0);
        for k in 1..100 {
            let (at, payload) = micros(k);
            assert_eq!(payload, k as u8, "in order");
            let step = (at - first) as i64 - 20_000 * k as i64;
            assert!(step.abs() <= 1, "record {k} is {step} µs off its arrival");
        }
    }

    /// A writer that falls behind costs the recording records, never the network thread a wait:
    /// a burst far past the ring returns at once and every record is written or counted.
    #[test]
    fn a_burst_past_the_ring_drops_records_and_never_blocks() {
        let path = scratch("burst");
        let (recording, mut tap) = Recording::create(&path, 40_000).unwrap();
        let from = SocketAddr::from(([10, 0, 0, 7], 50_000));
        let now = Instant::now();
        for _ in 0..20 * CAPACITY {
            tap.record(&[0; 1200], from, now);
        }
        drop(tap);
        let written = recording.finish().unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(written.records + u64::from(written.dropped), 20 * CAPACITY as u64);
        assert!(written.dropped > 0, "{written:?}");
    }

    #[test]
    fn a_recording_never_overwrites_a_file() {
        let path = scratch("exists");
        std::fs::write(&path, b"keep me").unwrap();
        let refused = Recording::create(&path, 40_000);
        assert_eq!(refused.err().map(|e| e.kind()), Some(io::ErrorKind::AlreadyExists));
        assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
        std::fs::remove_file(&path).unwrap();
    }
}
