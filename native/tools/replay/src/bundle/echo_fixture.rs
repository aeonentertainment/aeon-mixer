//! Writes the bundle that holds `scorecard`'s echo numbers still: one house mic in a room,
//! hearing the backing through the speakers, and then its singer over it. A house mic, since its
//! PCM keeps the echo's waveform: a phone's Opus at 32 kb/s fills noise in where the backing's
//! waveform was, and leaves the canceller little it can take out. Run it by hand only to change
//! the fixture, then score the new one into the baseline:
//! `cargo test -p replay -- --ignored writes_the_echo_fixture`.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;

use engine::block::RATE;
use engine::params::EchoCancelParams;
use kernels::fixtures::{at_db, pink_noise, room, talker, through};
use serde_json::json;

use super::live_song::LiveSong;
use crate::round_trip::house_mic;
use crate::wav;

const SECOND: usize = RATE as usize;
const SONG: usize = 16 * SECOND;
/// The singer starts once the canceller has had the room to itself long enough to settle, so
/// most of what it finds is of the room alone.
const SINGS_FROM: usize = 10 * SECOND;

/// What the mic hears: the backing through a room whose direct path is 30 ms long and 10 dB
/// down, and the singer from `SINGS_FROM`.
fn mic(backing: &[f32]) -> Vec<f32> {
    let mut heard = through(backing, &room(30 * SECOND / 1000, 0.3, 0.3, 0.25, 7));
    heard.truncate(SONG);
    let voice = at_db(talker(SONG - SINGS_FROM, 140.0, 11), -20.0);
    for (heard, sung) in heard[SINGS_FROM..].iter_mut().zip(voice) {
        *heard += sung;
    }
    heard
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
        "effects": [{ "kind": "echo-cancel", "enabled": true }]
    }));
    song.params(|p| p.slots[0].effects.echo_cancel = EchoCancelParams { enabled: true });
    song.start();
    let sender = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 40), 50_000));
    song.send(0, sender, 0, &house_mic(&mic(&backing)));
    song.play_to(SONG as u64);
    let (dir, _) = song.end();

    let bundle = fixtures.join("bundles/echo");
    let _ = std::fs::remove_dir_all(&bundle);
    std::fs::create_dir_all(bundle.join("slots")).unwrap();
    for name in ["manifest.json", "profile.json", "events.jsonl", "stats.jsonl", "slots/0.pcap"] {
        std::fs::copy(dir.join(name), bundle.join(name)).unwrap();
    }
}
