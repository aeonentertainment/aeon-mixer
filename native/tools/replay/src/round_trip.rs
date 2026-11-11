//! A known stream recorded through the real tap and file, then replayed: the recording changes
//! nothing the slot does, and the slot plays a clean stream at libopus's own level.

use std::f32::consts::TAU;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use engine::block::RATE;
use engine::ingest::Codec;
use transport::record::{Recording, Written};
use transport::rtp::Header;

use crate::capture;
use crate::levels::Levels;
use crate::reference::{self, Kind};
use crate::slot_replay::{Replayed, anchor, replay};
use crate::stream::{Arrival, Stream};

const FRAME: usize = 960;
const DELAY: u32 = RATE * 15 / 1000;
const PORT: u16 = 40_000;

/// A harmonic voice in `on` seconds of singing and `off` of silence, so DTX has pauses to take.
fn voice(seconds: f32, on: f32, off: f32) -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..(seconds * RATE as f32) as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let at = t % (on + off);
            let edge = (at.min(on - at) / 0.02).clamp(0.0, 1.0);
            phase = (phase + (180.0 + 40.0 * (t * 2.0).sin()) / RATE as f32) % 1.0;
            let buzz: f32 = (1..=8).map(|n| (TAU * n as f32 * phase).sin() / n as f32).sum();
            if at < on { 0.2 * edge * buzz } else { 0.0 }
        })
        .collect()
}

/// A datagram and the output frame it reached the socket at.
struct Sent {
    at: i64,
    datagram: Vec<u8>,
}

/// What a phone sends: Opus in 20 ms frames at 32 kb/s with FEC, and DTX when asked, which
/// sends one silent frame and then nothing until the voice comes back. A sender report goes
/// with every 50th packet. A LAN delays each by 5 ms and up to `jitter` frames more, and when it
/// `swaps`, trades the arrivals of one pair of neighbours in twenty. Deterministic.
fn phone(signal: &[f32], dtx: bool, jitter: u32, swaps: bool) -> Vec<Sent> {
    let mut encoder =
        opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
    encoder.set_bitrate(opus::Bitrate::Bits(32_000)).unwrap();
    encoder.set_inband_fec(true).unwrap();
    encoder.set_dtx(dtx).unwrap();
    let mut rng = 0x5eed_u64;
    let mut random = |n: u32| {
        rng = rng.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (rng >> 33) as u32 % n.max(1)
    };
    let mut sent = Vec::new();
    let (mut seq, mut in_dtx) = (100u16, false);
    let mut payload = [0u8; 1500];
    for (k, pcm) in signal.as_chunks::<FRAME>().0.iter().enumerate() {
        let len = encoder.encode_float(pcm, &mut payload).unwrap();
        let silent = len <= 2;
        if !(silent && in_dtx) {
            let mut datagram = vec![0u8; 12];
            let header = Header {
                marker: false,
                payload_type: 111,
                seq,
                timestamp: (k * FRAME) as u32,
                ssrc: 0xAE01,
            };
            header.write((&mut datagram[..12]).try_into().unwrap());
            datagram.extend_from_slice(&payload[..len]);
            let at = ((k + 1) * FRAME) as i64 + 240 + i64::from(random(jitter));
            sent.push(Sent { at, datagram });
            if seq % 50 == 0 {
                let mut report = vec![0u8; 28];
                report[0] = 0x80;
                report[1] = 200;
                sent.push(Sent { at: at + 1, datagram: report });
            }
            if swaps && random(100) < 5 && sent.len() > 2 {
                let n = sent.len();
                let (earlier, later) = (sent[n - 2].at, sent[n - 1].at);
                sent[n - 2].at = later;
                sent[n - 1].at = earlier;
            }
            seq = seq.wrapping_add(1);
        }
        in_dtx = silent;
    }
    sent.sort_by_key(|s| s.at);
    let first = sent[0].at;
    sent.iter_mut().for_each(|s| s.at -= first);
    sent
}

/// The packets as a live receiver pushes them, straight from the datagrams.
fn live(sent: &[Sent]) -> Vec<Arrival> {
    sent.iter()
        .filter_map(|s| {
            Some(Arrival { at: s.at, packet: transport::receive::packet_from(&s.datagram)? })
        })
        .collect()
}

/// The same datagrams through a real recording: the tap at each one's arrival, the writer, the
/// file, and back out of the file.
fn recorded(sent: &[Sent], name: &str) -> (Stream, Written) {
    let path = std::env::temp_dir().join(format!("aeon-replay-{}-{name}.pcap", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let (recording, mut tap) = Recording::create(&path, PORT).unwrap();
    let from = SocketAddr::from(([192, 168, 1, 30], 50_000));
    let base = Instant::now();
    for s in sent {
        let at =
            base + Duration::from_nanos((s.at as u64 * 1_000_000_000).div_ceil(u64::from(RATE)));
        assert_no_alloc::assert_no_alloc(|| tap.record(&s.datagram, from, at));
    }
    drop(tap);
    let written = recording.finish().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    (Stream::of(&capture::read(&bytes).unwrap(), None).unwrap(), written)
}

fn play(arrivals: &[Arrival]) -> Replayed {
    replay(arrivals, Codec::opus().unwrap(), DELAY, RATE as i64 / 4)
}

/// The first frame a signal is louder than -40 dBFS: where the voice starts.
fn onset(signal: &[f32]) -> usize {
    signal.iter().position(|s| s.abs() > 0.01).unwrap()
}

/// The issue's test: a recording made from a known packet stream replays to the same output,
/// sample for sample, as feeding those packets live, with the same counters and the same meter.
/// So recording adds nothing to the mic path: the voice comes out on the same frame.
#[test]
fn a_recorded_stream_replays_sample_for_sample_as_the_live_packets_play() {
    let signal = voice(4.0, 0.7, 0.5);
    let sent = phone(&signal, true, 480, true);
    let reports = sent.iter().filter(|s| s.datagram[1] == 200).count();
    let (stream, written) = recorded(&sent, "round-trip");
    assert_eq!(written, Written { records: sent.len() as u64, dropped: 0 });
    assert_eq!(
        (stream.port, stream.dropped),
        (PORT, reports),
        "RTCP drops out as the receiver drops it"
    );

    let live = live(&sent);
    let at =
        |arrivals: &[Arrival]| arrivals.iter().map(|a| (a.at, a.packet.seq)).collect::<Vec<_>>();
    assert_eq!(at(&stream.arrivals), at(&live), "every packet on the frame it arrived");

    let (from_file, from_live) = (play(&stream.arrivals), play(&live));
    assert!(from_live.stats.pushed > 0 && from_live.stats.concealed > 0, "{:?}", from_live.stats);
    assert_eq!(from_file.stats, from_live.stats);
    assert_eq!(from_file.meter_db, from_live.meter_db);
    assert!(from_file.out == from_live.out, "the same samples");
    let lag =
        |out: &[f32]| (onset(out) as i64 - onset(&signal) as i64) as f64 * 1000.0 / f64::from(RATE);
    println!(
        "voice onset after the source: live {:.2} ms, recorded {:.2} ms",
        lag(&from_live.out),
        lag(&from_file.out)
    );
}

/// The number #100 needs, on a stream with no fault in it: with every packet on time, a slot plays
/// at the level libopus alone decodes, DTX or not, and its meter reads what it plays.
#[test]
fn a_slot_plays_a_clean_stream_at_libopus_level() {
    let signal = voice(6.0, 0.7, 0.5);
    for dtx in [false, true] {
        let arrivals = live(&phone(&signal, dtx, 0, false));
        let played = play(&arrivals);
        assert_eq!((played.stats.lost, played.stats.late), (0, 0), "{dtx}: {:?}", played.stats);
        assert_eq!(played.ring_full, 0);
        let decoded = reference::decode(&arrivals, Kind::Opus).unwrap();
        let reference = Levels::of_signal(&decoded);
        // On the reference's windows, so a window holds the same stretch of voice in both
        let lag = anchor(&arrivals, DELAY);
        assert_eq!(lag, onset(&played.out) - onset(&decoded), "the voice plays at the anchor");
        let slot = Levels::of_signal(&played.out[lag..lag + decoded.len()]);
        // The meter's windows start with the slot's output, not with the voice
        let meter = Levels::of_readings(&played.meter_db);
        let unaligned = Levels::of_signal(&played.out);
        println!("dtx {dtx}:\n  libopus {reference:?}\n  slot {slot:?}\n  meter {meter:?}");
        for (name, levels, against) in [("slot", slot, reference), ("meter", meter, unaligned)] {
            let gaps = [
                levels.top_tenth_db - against.top_tenth_db,
                levels.median_db - against.median_db,
                levels.loudest_db - against.loudest_db,
            ];
            assert!(gaps.iter().all(|gap| gap.abs() < 0.1), "dtx {dtx}, {name}: {gaps:?} dB");
        }
    }
}
