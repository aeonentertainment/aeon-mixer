//! Writes the bundle `scorecard`'s tests and CI score: a short duet on the offline driver with
//! no real singer in it. Run it by hand only to change the fixture, then score the new one into
//! the baseline: `cargo test -p replay -- --ignored writes_the_scorecard_fixture`.

use std::f32::consts::TAU;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;

use engine::block::RATE;
use engine::command::Command;
use engine::params::{CorrectorParams, ReverbParams};
use serde_json::json;

use super::calls;
use super::line::Guide;
use super::live_song::LiveSong;
use crate::round_trip::phone;
use crate::wav;

const SECOND: u64 = RATE as u64;
/// Slot 0 sings one guide note a second, held for 700 ms.
const PHRASES: usize = 8;
const NOTE_MIDI: u8 = 54;

fn sender(last: u8) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, last), 50_000))
}

/// A voice holding `hz` for 700 ms of each second over `phrases` seconds, with a 5.5 Hz vibrato
/// of 15 cents: steady enough that its level and its distance from a guide read the same
/// wherever the windows and readings fall.
fn held_notes(hz: f32, phrases: usize) -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..phrases * SECOND as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let at = t % 1.0;
            let edge = (at.min(0.7 - at) / 0.02).clamp(0.0, 1.0);
            let hz = hz * 2f32.powf(0.15 / 12.0 * (TAU * 5.5 * t).sin());
            phase = (phase + hz / RATE as f32) % 1.0;
            let buzz: f32 = (1..=8).map(|n| (TAU * n as f32 * phase).sin() / n as f32).sum();
            if at < 0.7 { 0.2 * edge * buzz } else { 0.0 }
        })
        .collect()
}

#[test]
#[ignore = "writes the committed fixture; run by hand to change it"]
fn writes_the_scorecard_fixture() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scorecard/fixtures");
    let stems = fixtures.join("stems");
    std::fs::create_dir_all(&stems).unwrap();
    // The backing is silence: it is there for the guide clock, which runs on its playhead
    let instrumental = stems.join("silence.wav");
    wav::write(&instrumental, &vec![0.0; (PHRASES + 1) * SECOND as usize], 1).unwrap();

    let mut song = LiveSong::new("scorecard-fixture", &["rtp", "rtp"], Some(&instrumental));
    song.start();
    // Slot 0 holds each guide note 46 cents sharp, at 190 Hz on F#3's 185; slot 1 holds B3 on a
    // link that loses one packet in thirty and delivers the rest in time
    let first = held_notes(190.0, PHRASES);
    song.send(0, sender(30), 0, &phone(&first, false, 240, false));
    let mut second = phone(&held_notes(247.0, PHRASES), false, 240, false);
    second = second.into_iter().enumerate().filter(|(k, _)| k % 30 != 29).map(|(_, s)| s).collect();
    song.send(1, sender(31), SECOND / 4, &second);

    let notes: Vec<_> = (0..PHRASES)
        .map(|k| json!({ "midi": NOTE_MIDI, "startMs": k * 1000, "endMs": k * 1000 + 700 }))
        .collect();
    let guide = json!({ "notes": notes, "keyMask": 4095 });
    song.line(json!({ "type": "setSlotGuide", "index": 0, "guide": guide, "guideOffsetMs": null }));
    let guide: Guide = serde_json::from_value(guide).unwrap();
    song.command(|_| Command::SetGuide { slot: 0, guide: Some(Box::new(calls::guide(&guide))) });
    // Correction and reverb take the profile's values
    song.line(json!({
        "type": "setSlotEffects",
        "index": 0,
        "effects": [
            { "kind": "pitch-correction", "enabled": true },
            { "kind": "reverb", "enabled": true }
        ]
    }));
    song.params(|p| {
        let effects = &mut p.slots[0].effects;
        effects.corrector = CorrectorParams { enabled: true, strength: 0.6, retune_ms: 40.0 };
        effects.reverb = ReverbParams { enabled: true, room_size: 0.5, mix: 0.25 };
    });
    song.line(json!({ "type": "setSlot", "index": 1, "settings": { "pan": 0.3 } }));
    song.params(|p| p.slots[1].pan = 0.3);
    song.play_to((PHRASES as u64 * 2 + 1) * SECOND / 2);
    song.save(&fixtures.join("bundles/duet"));
}
