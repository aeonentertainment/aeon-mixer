//! A song played live on the offline driver that writes its own session bundle as it goes, the
//! way a mixer and `src/bundle/bundle.ts` write one: each slot's datagrams through the real tap,
//! senders hidden, on the bundle's wall clock, and a line with the clock pair for each call. The
//! test makes each call twice, once as the line the host's call writes and once as what the
//! engine was sent for it, so a replay that resolves a call differently plays different samples.
//!
//! Time is the song's own: frame F falls at `F / 48 000` seconds after the song's base instant,
//! so the pcaps' stamps and the lines' pairs agree exactly.

use std::fs::File;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use assert_no_alloc::assert_no_alloc;
use engine::audio_thread::{AudioThread, Controls};
use engine::backing::{Backing, RenderJob, StemFiles};
use engine::block::{BLOCK, RATE};
use engine::command::Command;
use engine::driver::OfflineDriver;
use engine::graph::Graph;
use engine::ingest::packet::RING_CAPACITY;
use engine::ingest::{Codec, JitterBuffer, PacketProducer, PacketSource, packet_ring};
use engine::params::{Params, StemGains};
use engine::slot::Slot;
use serde_json::{Value, json};
use transport::receive::packet_from;
use transport::record::{Recording, Tap, WallClock};

use super::silence::Silence;
use super::stems::sha256;
use crate::round_trip::Sent;

/// The default profile with the delay pinned at 15 ms, as Aeon Karaoke pins it.
pub fn profile() -> Value {
    json!({
        "version": 1,
        "output": { "periodFrames": 128 },
        "ingest": { "playoutDelayMs": 15 },
        "slot": {
            "gain": 1,
            "pan": 0,
            "effects": {
                "pitch-correction": { "strength": 0.6, "retuneMs": 40 },
                "reverb": { "roomSize": 0.5, "mix": 0.25 },
                "voice": { "formantSemitones": 0 }
            }
        },
        "backing": { "stemGains": { "instrumental": 1, "guideVocals": 0 } }
    })
}

/// Blocks the engine renders before the mixer exists, so the bundle's clock starts past zero.
const BEFORE: u64 = 3;

pub struct LiveSong {
    pub dir: PathBuf,
    clock: WallClock,
    base: Instant,
    controls: Controls,
    driver: OfflineDriver,
    graph: Option<Box<Graph>>,
    rings: Vec<Option<PacketProducer>>,
    recordings: Vec<(Recording, Tap)>,
    /// Which recording each slot's datagrams go to.
    tapped: Vec<Option<usize>>,
    events: File,
    stats: File,
    /// `(frame, slot, sender, datagram)`, sorted by frame.
    pending: Vec<(u64, usize, SocketAddr, Vec<u8>)>,
    frame: u64,
    next_id: u32,
    /// What the room heard, interleaved stereo, from the mixer's creation.
    pub out: Vec<f32>,
}

impl LiveSong {
    /// A mixer of `kinds` writing its bundle into a fresh directory `name`, with the stem at
    /// `instrumental` as its backing when there is one.
    pub fn new(name: &str, kinds: &[&str], instrumental: Option<&Path>) -> Self {
        let dir = std::env::temp_dir().join(format!("aeon-bundle-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["", "slots", "host"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let clock = WallClock::now();
        let base = Instant::now();
        let mut rings = Vec::new();
        let mut recordings = Vec::new();
        let mut tapped = Vec::new();
        let mut slots = Vec::new();
        for (index, kind) in kinds.iter().enumerate() {
            if is_recorded(kind) {
                let (producer, consumer) = packet_ring(RING_CAPACITY);
                let codec = if *kind == "l16" { Codec::L16 } else { Codec::opus().unwrap() };
                let jitter = JitterBuffer::new(15 * RATE / 1000, codec);
                slots.push(Slot::new(Box::new(PacketSource::new(consumer, jitter))));
                rings.push(Some(producer));
                let path = dir.join("slots").join(format!("{index}.pcap"));
                let (recording, mut tap) = Recording::create(&path, 40_000, clock).unwrap();
                tap.hide_senders();
                tapped.push(Some(recordings.len()));
                recordings.push((recording, tap));
            } else {
                slots.push(Slot::new(Box::new(Silence)));
                rings.push(None);
                tapped.push(None);
            }
        }
        let mut graph = Box::new(Graph::new(slots));
        let backing = instrumental.map(|path| {
            let files = StemFiles { instrumental: path.to_owned(), guide: None };
            graph.set_backing(Backing::new(RenderJob::start(files, 0, 1.0).finish().unwrap(), 0));
            json!({
                "stems": [{ "kind": "instrumental", "sha256": sha256(path).unwrap() }],
                "key": 0,
                "speed": 1,
                "stemGains": {},
                "stopAtMs": null
            })
        });
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        driver.pump(BEFORE as usize);
        controls.params.update(|params| {
            params.stems = StemGains { instrumental: 1.0, guide: 0.0 };
        });

        let song = Self {
            events: File::create_new(dir.join("events.jsonl")).unwrap(),
            stats: File::create_new(dir.join("stats.jsonl")).unwrap(),
            dir,
            clock,
            base,
            controls,
            driver,
            graph: Some(graph),
            rings,
            recordings,
            tapped,
            pending: Vec::new(),
            frame: BEFORE * BLOCK as u64,
            next_id: 0,
            out: Vec::new(),
        };
        let (frame, us) = song.pair();
        let slots: Vec<Value> = kinds
            .iter()
            .enumerate()
            .map(
                |(index, kind)| json!({ "index": index, "kind": kind, "recorded": is_recorded(kind) }),
            )
            .collect();
        let manifest = json!({
            "format": 1,
            "package": "0.9.0",
            "platform": "test",
            "output": { "sampleRate": 48000, "periodFrames": 128, "latencyMs": 0, "name": null },
            "frame": frame,
            "us": us,
            "slots": slots,
            "backing": backing,
        });
        std::fs::write(song.dir.join("manifest.json"), manifest.to_string()).unwrap();
        std::fs::write(song.dir.join("profile.json"), profile().to_string()).unwrap();
        song
    }

    /// The mixer's frame now, from its creation.
    pub fn now(&self) -> u64 {
        self.frame - BEFORE * BLOCK as u64
    }

    fn pair(&self) -> (u64, u128) {
        (self.frame, self.clock.at(instant(self.base, self.frame)).as_micros())
    }

    /// A line stamped now, as a host's call or the engine's report writes it.
    pub fn line(&mut self, mut body: Value) {
        let (frame, us) = self.pair();
        body["frame"] = json!(frame);
        body["us"] = json!(us);
        writeln!(self.events, "{body}").unwrap();
    }

    /// `sent` reaching `slot` from `from`, its first datagram `after` frames from now.
    pub fn send(&mut self, slot: usize, from: SocketAddr, after: u64, sent: &[Sent]) {
        let at = self.frame + after;
        self.pending
            .extend(sent.iter().map(|s| (at + s.at as u64, slot, from, s.datagram.clone())));
        self.pending.sort_by_key(|&(at, ..)| at);
    }

    /// What the engine was sent for a call: a parameter write.
    pub fn params(&mut self, change: impl FnOnce(&mut Params)) {
        self.controls.params.update(change);
    }

    /// What the engine was sent for a call: a command, with the next id where it takes one.
    pub fn command(&mut self, command: impl FnOnce(u32) -> Command) {
        self.next_id += 1;
        assert!(self.controls.commands.push(command(self.next_id)).is_ok());
    }

    /// `start`, as the mixer makes it: the graph swapped in, then the backing started.
    pub fn start(&mut self) {
        self.line(json!({ "type": "start" }));
        let graph = self.graph.take().unwrap();
        self.command(|id| Command::Swap { id, graph });
        self.command(|id| Command::Start { id });
    }

    /// Renders until the mixer's frame reaches `frame`, each datagram tapped and pushed before
    /// the first block that starts at or after its arrival, with a stats line once a second.
    pub fn play_to(&mut self, frame: u64) {
        let mut block = [0.0f32; 2 * BLOCK];
        while self.now() < frame {
            let due = self.pending.partition_point(|&(at, ..)| at <= self.frame);
            for (at, slot, from, datagram) in self.pending.drain(..due) {
                if let Some(recording) = self.tapped[slot] {
                    let at = instant(self.base, at);
                    self.recordings[recording].1.record(&datagram, from, at);
                }
                if let (Some(ring), Some(packet)) =
                    (&mut self.rings[slot], packet_from(&datagram, from))
                {
                    assert!(ring.push(&packet), "the slot's ring has room");
                }
            }
            assert_no_alloc(|| self.driver.pump_into(&mut block));
            self.out.extend_from_slice(&block);
            while self.controls.events.pop().is_some() {}
            while self.controls.retired.pop().is_some() {}
            self.frame += BLOCK as u64;
            if self.now() % u64::from(RATE) < BLOCK as u64 {
                let (frame, us) = self.pair();
                let line = json!({ "frame": frame, "us": us, "positionMs": null, "slots": [] });
                writeln!(self.stats, "{line}").unwrap();
            }
        }
    }

    /// `destroy`: the `end` line, and the recordings finished.
    pub fn end(mut self) -> (PathBuf, Vec<f32>) {
        self.line(json!({ "type": "end" }));
        for (recording, tap) in self.recordings {
            drop(tap);
            recording.finish().unwrap();
        }
        (self.dir, self.out)
    }
}

/// Whether a slot of `kind` arrives as packets, which the bundle records.
fn is_recorded(kind: &str) -> bool {
    matches!(kind, "rtp" | "l16")
}

/// Where engine frame `frame` falls on the song's own time.
fn instant(base: Instant, frame: u64) -> Instant {
    base + Duration::from_nanos((frame * 1_000_000_000).div_ceil(u64::from(RATE)))
}
