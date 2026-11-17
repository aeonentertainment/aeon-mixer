//! Writes the bundle `scorecard`'s tests and CI score: a short duet on the offline driver with
//! no real singer in it. Run it by hand only to change the fixture, then score the new one into
//! the baseline: `cargo test -p replay -- --ignored writes_the_scorecard_fixture`.

use std::f32::consts::TAU;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;

use engine::block::RATE;
use engine::command::Command;
use engine::params::{
    CleanupParams, CorrectorParams, EchoCancelParams, HowlGuardParams, ReverbParams,
};
use serde_json::json;

use super::calls;
use super::line::Guide;
use super::live_song::LiveSong;
use crate::round_trip::{phone, voice};
use crate::wav;

const SECOND: u64 = RATE as u64;
/// The backing is noise that repeats every quarter second: sharp enough for the echo canceller
/// to find its path, and a pattern git stores in one period's 24 kilobytes.
const PERIOD: usize = RATE as usize / 4;
/// How long the room takes to bring the backing back into slot 0's phone.
const ROOM_FRAMES: usize = 1_440;
/// Slot 0 sings one guide note a second, held for 700 ms.
const PHRASES: usize = 8;
const NOTE_MIDI: u8 = 54;

fn sender(last: u8) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, last), 50_000))
}

fn backing(frames: usize, gain: f32) -> impl Iterator<Item = f32> {
    let mut rng = 0x5eed_u32;
    let period: Vec<f32> = (0..PERIOD)
        .map(|_| {
            rng = rng.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            gain * ((rng >> 8) as f32 / (1 << 23) as f32 - 1.0)
        })
        .collect();
    (0..frames).map(move |i| period[i % PERIOD])
}

/// A voice holding each guide note 46 cents sharp, at 190 Hz on F#3's 185, with a 5.5 Hz
/// vibrato of 15 cents: steady enough that its distance from the guide reads the same wherever
/// the readings fall.
fn held_notes() -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..PHRASES * SECOND as usize)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let at = t % 1.0;
            let edge = (at.min(0.7 - at) / 0.02).clamp(0.0, 1.0);
            let hz = 190.0 * 2f32.powf(0.15 / 12.0 * (TAU * 5.5 * t).sin());
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
    let instrumental = stems.join("noise.wav");
    wav::write(
        &instrumental,
        &backing((PHRASES + 1) * SECOND as usize, 0.1).collect::<Vec<_>>(),
        1,
    )
    .unwrap();

    let mut song = LiveSong::new("scorecard-fixture", &["rtp", "rtp"], Some(&instrumental));
    song.start();
    // Slot 0 sings with the backing coming back into its phone from the room; slot 1 sings on a
    // link that loses one packet in thirty and delivers the rest in time
    let mut first = held_notes();
    for (sample, echo) in first.iter_mut().skip(ROOM_FRAMES).zip(backing(usize::MAX, 0.03)) {
        *sample += echo;
    }
    song.send(0, sender(30), 0, &phone(&first, false, 240, false));
    let mut second = phone(&voice(PHRASES as f32 - 1.0, 0.7, 0.3), false, 240, false);
    second = second.into_iter().enumerate().filter(|(k, _)| k % 30 != 29).map(|(_, s)| s).collect();
    song.send(1, sender(31), SECOND / 4, &second);

    let notes: Vec<_> = (0..PHRASES)
        .map(|k| json!({ "midi": NOTE_MIDI, "startMs": k * 1000, "endMs": k * 1000 + 700 }))
        .collect();
    let guide = json!({ "notes": notes, "keyMask": 4095 });
    song.line(json!({ "type": "setSlotGuide", "index": 0, "guide": guide, "guideOffsetMs": null }));
    let guide: Guide = serde_json::from_value(guide).unwrap();
    song.command(|_| Command::SetGuide { slot: 0, guide: Some(Box::new(calls::guide(&guide))) });
    // Correction and reverb take the profile's values; the rest have none
    song.line(json!({
        "type": "setSlotEffects",
        "index": 0,
        "effects": [
            { "kind": "echo-cancel", "enabled": true },
            { "kind": "howl-guard", "enabled": true },
            { "kind": "cleanup", "enabled": true },
            { "kind": "pitch-correction", "enabled": true },
            { "kind": "reverb", "enabled": true }
        ]
    }));
    song.params(|p| {
        let effects = &mut p.slots[0].effects;
        effects.echo_cancel = EchoCancelParams { enabled: true };
        effects.howl_guard = HowlGuardParams { enabled: true };
        effects.cleanup = CleanupParams { enabled: true };
        effects.corrector = CorrectorParams { enabled: true, strength: 0.6, retune_ms: 40.0 };
        effects.reverb = ReverbParams { enabled: true, room_size: 0.5, mix: 0.25 };
    });
    song.line(json!({ "type": "setSlot", "index": 1, "settings": { "pan": 0.3 } }));
    song.params(|p| p.slots[1].pan = 0.3);
    song.play_to((PHRASES as u64 * 2 + 1) * SECOND / 2);
    let (dir, _) = song.end();

    let bundle = fixtures.join("bundles/duet");
    let _ = std::fs::remove_dir_all(&bundle);
    std::fs::create_dir_all(bundle.join("slots")).unwrap();
    for name in [
        "manifest.json",
        "profile.json",
        "events.jsonl",
        "stats.jsonl",
        "slots/0.pcap",
        "slots/1.pcap",
    ] {
        std::fs::copy(dir.join(name), bundle.join(name)).unwrap();
    }
}
