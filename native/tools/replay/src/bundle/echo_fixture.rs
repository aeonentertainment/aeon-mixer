//! Writes the bundle that holds `scorecard`'s echo numbers still: one house mic in a room that
//! plays it back what the speakers play, a howl that grows and is caught, and then its singer,
//! with the echo canceller and the howl guard on. A house mic, since its PCM keeps the echo's
//! waveform: a phone's Opus at 32 kb/s fills noise in where the backing's waveform was, and leaves
//! the canceller little it can take out. Run it by hand only to change
//! the fixture, then score the new one into the baseline:
//! `cargo test -p replay -- --ignored writes_the_echo_fixture`.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;

use engine::block::RATE;
use engine::params::{EchoCancelParams, HowlGuardParams};
use kernels::fixtures::{at_db, pink_noise, room, talker};
use serde_json::json;

use super::live_song::LiveSong;
use crate::round_trip::{HOUSE_MIC_PACKET, Sent, house_mic_packet};
use crate::wav;

const SECOND: usize = RATE as usize;
const SONG: usize = 16 * SECOND;
/// The singer starts once the canceller has had the room to itself long enough to settle, so
/// most of what it finds is of the room alone, and stops before the howl.
const SINGS_FROM: usize = 10 * SECOND;
const SINGS_TO: usize = 27 * SECOND / 2;
/// A howl at 1.2 kHz in the room after the singer: from -40 dBFS it grows 25 dB a second for
/// 0.45 s, as a loop 2 dB over unity on an 80 ms trip does, then holds until 15.5 s. The guard
/// catches it once, as it grows; held, it grows no further and is not caught again. It comes last
/// because a tone in the master drives the canceller's filter off the room, and the canceller's
/// numbers are of the room.
const HOWLS_FROM: usize = 138 * SECOND / 10;
const HOWL_GROWS: usize = 45 * SECOND / 100;
const HOWL_ENDS: usize = 31 * SECOND / 2;

/// What the mic hears at the mouth, before the room: the howl, then the singer.
fn sung() -> Vec<f32> {
    let mut sung = vec![0.0; SONG];
    // In RMS, as `at_db` levels the voice: a sine's peak is 3 dB over its RMS
    for (i, sample) in sung[HOWLS_FROM..HOWL_ENDS].iter_mut().enumerate() {
        let db = -40.0 + 3.01 + 25.0 * i.min(HOWL_GROWS) as f32 / SECOND as f32;
        let phase = std::f32::consts::TAU * 1200.0 * i as f32 / SECOND as f32;
        *sample = 10f32.powf(db / 20.0) * phase.sin();
    }
    let voice = at_db(talker(SINGS_TO - SINGS_FROM, 140.0, 11), -20.0);
    sung[SINGS_FROM..SINGS_TO].copy_from_slice(&voice);
    sung
}

#[test]
#[ignore = "writes the committed fixture; run by hand to change it"]
fn writes_the_echo_fixture() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scorecard/fixtures");
    let stems = fixtures.join("stems");
    std::fs::create_dir_all(&stems).unwrap();
    // Pink noise, the backing bed `aec:measure` plays: every band, and no period to alias on
    let instrumental = stems.join("pink.wav");
    let backing = at_db(pink_noise(SONG + SECOND / 2, 3), -20.0);
    wav::write(&instrumental, &backing, 1).unwrap();

    let mut song = LiveSong::new("echo-fixture", &["l16"], Some(&instrumental));
    song.line(json!({
        "type": "setSlotEffects",
        "index": 0,
        "effects": [
            { "kind": "echo-cancel", "enabled": true },
            { "kind": "howl-guard", "enabled": true }
        ]
    }));
    song.params(|p| {
        p.slots[0].effects.echo_cancel = EchoCancelParams { enabled: true };
        p.slots[0].effects.howl_guard = HowlGuardParams { enabled: true };
    });
    song.start();
    // The room hears what the speakers play, the master as the engine rendered it, so the mic's
    // echo of the singer and of the howl is the one the canceller sees in the master, ducks and
    // all. Each packet is built once the master it echoes has been rendered: the room's shortest
    // path, 30 ms, is longer than a packet and its wait
    let path = room(30 * SECOND / 1000, 0.3, 0.3, 0.25, 7);
    let shortest = path.iter().map(|&(lag, _)| lag).min().unwrap();
    let sung = sung();
    let sender = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 40), 50_000));
    let mut heard = [0.0; HOUSE_MIC_PACKET];
    for k in 0..SONG / HOUSE_MIC_PACKET {
        let first = k * HOUSE_MIC_PACKET;
        song.play_to((first + HOUSE_MIC_PACKET).saturating_sub(shortest) as u64);
        let played =
            |at: usize| song.out.get(2 * at..2 * at + 2).map_or(0.0, |lr| 0.5 * (lr[0] + lr[1]));
        for (n, sample) in heard.iter_mut().enumerate() {
            let i = first + n;
            let echo: f32 = path
                .iter()
                .filter_map(|&(lag, gain)| Some(gain * played(i.checked_sub(lag)?)))
                .sum();
            *sample = echo + sung[i];
        }
        let packet = house_mic_packet(k, &heard);
        let after = (packet.at as u64).saturating_sub(song.now());
        song.send(0, sender, after, &[Sent { at: 0, ..packet }]);
    }
    song.play_to(SONG as u64);
    song.save(&fixtures.join("bundles/echo"));
}
