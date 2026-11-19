//! The bundle a bench run writes, as a mixer writes one (docs/design.md 8.9, 8.11). Its time is
//! the output's frames: every `us` is the run's first `us` plus the frames since, so its pairs
//! never drift and it replays to the samples the run rendered. Block 0 of the run is frame 0.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;
use std::time::Duration;

use engine::block::{BLOCK, RATE};
use replay::bundle::Bundle;
use serde_json::{Value, json};
use transport::record::pcap;
use transport::rtp::{HEADER_LEN, Header};

/// What a bench run tells its bundle beyond the song it played.
pub struct Run<'a> {
    /// The wall clock, in microseconds since the Unix epoch, as the run started: block 0's time.
    pub us: i64,
    /// `manifest.json`'s `output`, as the run's output reported itself.
    pub output: Value,
    /// The profile the run played under, as the file it came from holds it.
    pub profile: &'a str,
    /// `stats.jsonl`'s lines, each with its frame on the run's clock.
    pub stats: &'a [(u64, Value)],
}

/// The payload type a written packet carries. The slot's kind, not the header, says the codec.
const PAYLOAD_TYPE: u8 = 96;

/// Writes the bundle of `run` into `dir`, a directory that must not exist yet, from the bundle
/// in `input` that it played.
pub fn write(dir: &Path, input: &Path, bundle: &Bundle, run: &Run) -> Result<(), String> {
    let us_at = |frame: i64| run.us + (frame * 1_000_000).div_euclid(i64::from(RATE));
    std::fs::create_dir(dir).map_err(at(dir))?;
    let slots = dir.join("slots");
    std::fs::create_dir(&slots).map_err(at(&slots))?;

    let mut manifest = read_json(&input.join("manifest.json"))?;
    manifest["frame"] = json!(0);
    manifest["us"] = json!(run.us);
    manifest["output"] = run.output.clone();
    manifest["platform"] = json!(std::env::consts::OS);
    write_text(&dir.join("manifest.json"), &format!("{manifest}\n"))?;
    write_text(&dir.join("profile.json"), run.profile)?;

    // Each call at the frame `replay` takes it up at, which is where the run handed it over
    let origin = bundle.manifest.frame;
    let mut events = String::new();
    let path = input.join("events.jsonl");
    let text = std::fs::read_to_string(&path).map_err(at(&path))?;
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let mut line: Value =
            serde_json::from_str(line).map_err(|e| format!("events.jsonl: {e}"))?;
        let frame = line["frame"].as_f64().ok_or("events.jsonl: a line with no frame")?;
        let frame = (frame - origin).round() as i64;
        line["frame"] = json!(frame);
        line["us"] = json!(us_at(frame));
        events += &format!("{line}\n");
    }
    write_text(&dir.join("events.jsonl"), &events)?;

    let mut stats = String::new();
    for (frame, line) in run.stats {
        let mut line = line.clone();
        line["frame"] = json!(frame);
        line["us"] = json!(us_at(*frame as i64));
        stats += &format!("{line}\n");
    }
    write_text(&dir.join("stats.jsonl"), &stats)?;

    for slot in bundle.manifest.slots.iter().filter(|slot| slot.recorded) {
        let mut file = pcap::global_header().to_vec();
        for (_, arrival) in bundle.arrivals.iter().filter(|(index, _)| *index == slot.index) {
            // Handed over before the first block that starts at or after its arrival, so it is
            // stamped half a block before that block: a rounding either way lands it there again
            let block = (arrival.at.max(0) as u64).next_multiple_of(BLOCK as u64) as i64;
            let at = us_at(block - BLOCK as i64 / 2);
            let packet = &arrival.packet;
            let header = Header {
                marker: packet.marker,
                payload_type: PAYLOAD_TYPE,
                seq: packet.seq,
                timestamp: packet.timestamp,
                ssrc: packet.ssrc,
            };
            let mut fixed = [0u8; HEADER_LEN];
            header.write(&mut fixed);
            let datagram = [fixed.as_slice(), packet.payload()].concat();
            let from = match packet.from {
                SocketAddr::V4(from) => (*from.ip(), from.port()),
                SocketAddr::V6(from) => (Ipv4Addr::UNSPECIFIED, from.port()),
            };
            let micros = Duration::from_micros(at.max(0) as u64);
            pcap::append_record(&mut file, micros, from, 40_000 + slot.index as u16, &datagram);
        }
        let path = slots.join(format!("{}.pcap", slot.index));
        std::fs::write(&path, file).map_err(at(&path))?;
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(at(path))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_text(path: &Path, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(at(path))
}

/// An IO error, named by the file it happened on.
fn at(path: &Path) -> impl FnOnce(std::io::Error) -> String + '_ {
    move |e| format!("{}: {e}", path.display())
}
