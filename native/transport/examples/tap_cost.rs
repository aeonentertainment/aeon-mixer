//! What recording costs the network thread per datagram: the moment it is read, and the tap's
//! copy into the writer's channel. Everything else, the pcap framing and the file write, is the
//! writer thread's. Run it with `cargo run --release -p transport --example tap_cost -- DIR`;
//! it writes one throwaway recording into DIR and deletes it.
//!
//! Paced is one datagram every 2.5 ms, as a phone's shortest frames come, so each record lands on
//! a thread that has just slept. "off" times the same paced loop with no tap in it: what the
//! machine itself adds to any 2.5 ms of a thread's life. A burst is the network thread catching
//! up after a stall.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use transport::record::Recording;

const RECORDS: usize = 4000;

fn main() -> std::io::Result<()> {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: tap_cost DIR"));
    let from = SocketAddr::from(([192, 168, 1, 30], 50_000));
    let datagram = [0x80u8; 112]; // a 32 kb/s Opus frame of 20 ms with its RTP header
    let pace = Duration::from_micros(2500);
    let mut off = Vec::with_capacity(RECORDS);
    for _ in 0..RECORDS {
        let started = Instant::now();
        let _ = std::hint::black_box(Instant::now());
        off.push(started.elapsed().as_nanos() as u64);
        std::thread::sleep(pace);
    }
    report("off, paced", off, None);
    for (name, pace) in [("on, paced", Some(pace)), ("on, burst", None)] {
        let path = dir.join(format!("tap-cost-{}.pcap", std::process::id()));
        let (recording, mut tap) = Recording::create(&path, 40_000)?;
        let mut costs = Vec::with_capacity(RECORDS);
        for _ in 0..RECORDS {
            let started = Instant::now();
            let at = Instant::now();
            tap.record(&datagram, from, at);
            costs.push(started.elapsed().as_nanos() as u64);
            if let Some(pace) = pace {
                std::thread::sleep(pace);
            }
        }
        drop(tap);
        let written = recording.finish()?;
        std::fs::remove_file(&path)?;
        report(name, costs, Some(written));
    }
    Ok(())
}

fn report(name: &str, mut costs: Vec<u64>, written: Option<transport::record::Written>) {
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
