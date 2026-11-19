//! What recording costs the network thread per datagram: the moment it is read, and the tap's
//! copy into the writer's channel. Everything else, the pcap framing and the file write, is the
//! writer thread's. Run it with `cargo run --release -p transport --example tap_cost -- DIR`;
//! it writes one throwaway recording into DIR and deletes it. `tap_cost DIR SLOTS` records that
//! many slots at once with their senders hidden, as a session bundle does (docs/design.md 8.11).
//!
//! Paced is one datagram every 2.5 ms for each slot, as a phone's shortest frames come, so with
//! SLOTS the thread sleeps 2.5 ms / SLOTS before each one and every record lands on a thread that
//! has just slept. "off" times the same paced loop with no tap in it: what the
//! machine itself adds to any 2.5 ms of a thread's life. A burst is the network thread catching
//! up after a stall.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use transport::record::{Recording, WallClock, Written};

const RECORDS: usize = 4000;

fn main() -> std::io::Result<()> {
    let mut args = std::env::args().skip(1);
    let dir = PathBuf::from(args.next().expect("usage: tap_cost DIR [SLOTS]"));
    let bundle = args.next().map(|slots| slots.parse::<usize>().expect("SLOTS is a count"));
    let slots = bundle.unwrap_or(1);
    let from = SocketAddr::from(([192, 168, 1, 30], 50_000));
    let datagram = [0x80u8; 112]; // a 32 kb/s Opus frame of 20 ms with its RTP header
    let pace = Duration::from_micros(2500) / slots as u32;
    let mut off = Vec::with_capacity(RECORDS);
    for _ in 0..RECORDS * slots {
        let started = Instant::now();
        let _ = std::hint::black_box(Instant::now());
        off.push(started.elapsed().as_nanos() as u64);
        std::thread::sleep(pace);
    }
    report("off, paced", off, None);
    for (name, pace) in [("on, paced", Some(pace)), ("on, burst", None)] {
        let mut paths = Vec::with_capacity(slots);
        let mut recordings = Vec::with_capacity(slots);
        let mut taps = Vec::with_capacity(slots);
        for slot in 0..slots {
            let path = dir.join(format!("tap-cost-{}-{slot}.pcap", std::process::id()));
            let (recording, mut tap) = Recording::create(&path, 40_000, WallClock::now())?;
            if bundle.is_some() {
                tap.hide_senders();
            }
            paths.push(path);
            recordings.push(recording);
            taps.push(tap);
        }
        let mut costs = Vec::with_capacity(RECORDS * slots);
        for _ in 0..RECORDS {
            for tap in &mut taps {
                let started = Instant::now();
                let at = Instant::now();
                tap.record(&datagram, from, at);
                costs.push(started.elapsed().as_nanos() as u64);
                if let Some(pace) = pace {
                    std::thread::sleep(pace);
                }
            }
        }
        drop(taps);
        let mut written = Written { records: 0, dropped: 0 };
        for (recording, path) in recordings.into_iter().zip(&paths) {
            let slot = recording.finish()?;
            std::fs::remove_file(path)?;
            written.records += slot.records;
            written.dropped += slot.dropped;
        }
        report(name, costs, Some(written));
    }
    Ok(())
}

fn report(name: &str, mut costs: Vec<u64>, written: Option<Written>) {
    costs.sort_unstable();
    let at = |q: f64| costs[((costs.len() - 1) as f64 * q) as usize];
    let written =
        written.map_or(String::new(), |w| format!("  written {} dropped {}", w.records, w.dropped));
    println!(
        "{name:<11} median {:>6} ns  p99 {:>6} ns  p99.9 {:>7} ns  max {:>7} ns{written}",
        at(0.5),
        at(0.99),
        at(0.999),
        at(1.0)
    );
}
