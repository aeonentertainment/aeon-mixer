//! #180's and #204's done-when: a song written to a bundle on the offline driver, replayed.

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::Path;

use engine::block::RATE;
use engine::command::Command;
use engine::params::{CorrectorParams, ReverbParams, ToneBand, ToneBands, ToneParams};
use serde_json::json;

use super::Bundle;
use super::live_song::{LiveSong, profile};
use super::profile::Profile;
use super::song::{Played, play};
use super::stems;
use crate::round_trip::{phone, voice};
use crate::wav;

const SECOND: u64 = RATE as u64;

fn sender(last: u8) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, last), 50_000))
}

fn replay(dir: &Path, profile: Option<&Profile>, stems_dir: Option<&Path>) -> Played {
    let bundle = Bundle::read(dir).unwrap();
    let stems = stems_dir.map(|stems_dir| {
        stems::find(stems_dir, bundle.manifest.backing.as_ref().unwrap()).unwrap()
    });
    play(&bundle, profile.unwrap_or(&bundle.profile), stems).unwrap()
}

/// A sung second, as a phone sends it.
fn singing(seconds: f32) -> Vec<crate::round_trip::Sent> {
    phone(&voice(seconds, 0.7, 0.3), false, 240, false)
}

fn rms(samples: impl Iterator<Item = f32>) -> f32 {
    let (sum, n) = samples.fold((0.0f64, 0usize), |(sum, n), s| (sum + f64::from(s * s), n + 1));
    (sum / n.max(1) as f64).sqrt() as f32
}

#[test]
fn a_song_replays_to_the_samples_it_played_with_an_effect_change_and_a_seek_on_their_blocks() {
    let stems_dir = std::env::temp_dir().join(format!("aeon-bundle-{}-stems", std::process::id()));
    std::fs::create_dir_all(&stems_dir).unwrap();
    let stem: Vec<f32> = (0..4 * SECOND)
        .map(|i| 0.2 * (std::f32::consts::TAU * 220.0 * i as f32 / RATE as f32).sin())
        .collect();
    let instrumental = stems_dir.join("instrumental.wav");
    wav::write(&instrumental, &stem, 1).unwrap();

    let mut song = LiveSong::new("whole", &["rtp", "rtp", "push"], Some(&instrumental));
    // Armed: the phone already sends, and calls build what start writes; a reset does nothing
    song.send(0, sender(30), 0, &singing(2.5));
    song.line(json!({ "type": "setSlot", "index": 0, "settings": { "gain": 0.8 } }));
    song.params(|p| p.slots[0].gain = 0.8);
    song.line(json!({ "type": "resetSlot", "index": 0 }));
    song.play_to(256);
    song.start();
    song.send(1, sender(31), 2_000, &singing(2.5));
    song.play_to(SECOND);
    // The reverb's values are left to the profile; correction's strength is the singer's choice
    song.line(json!({
        "type": "setSlotEffects",
        "index": 0,
        "effects": [
            { "kind": "reverb", "enabled": true },
            { "kind": "pitch-correction", "enabled": true, "strength": 1 }
        ]
    }));
    song.params(|p| {
        p.slots[0].effects.reverb = ReverbParams { enabled: true, room_size: 0.5, mix: 0.25 };
        p.slots[0].effects.corrector =
            CorrectorParams { enabled: true, strength: 1.0, retune_ms: 40.0 };
    });
    song.line(json!({ "type": "setSlot", "index": 1, "settings": { "pan": 0.5 } }));
    song.params(|p| p.slots[1].pan = 0.5);
    song.line(json!({ "type": "setStemGain", "kind": "instrumental", "gain": 0.5 }));
    song.params(|p| p.stems.instrumental = 0.5);
    song.play_to(SECOND * 3 / 2);
    song.line(json!({ "type": "seek", "toMs": 3000 }));
    song.command(|id| Command::Seek { id, to_frame: 3 * SECOND });
    song.play_to(SECOND * 3 / 2 + 1_024);
    song.line(json!({ "type": "ack", "call": "seek", "positionMs": 3000 }));
    song.play_to(3 * SECOND);
    let (dir, live) = song.end();

    let played = replay(&dir, None, Some(&stems_dir));
    assert!(played.notes.is_empty(), "{:?}", played.notes);
    assert!(played.slots[0].stats.pushed > 100 && played.slots[1].stats.pushed > 100);
    assert!(rms(live.iter().copied()) > 0.05, "the song is not silent");
    assert_eq!(played.mix.len(), live.len());
    assert!(played.mix == live, "the replay plays the song's samples");
    assert!(replay(&dir, None, Some(&stems_dir)).mix == played.mix, "and the same every time");

    // Without the stems the mic path plays alone: the same voices, no backing
    let alone = replay(&dir, None, None);
    assert_eq!(alone.notes, ["no stems given, so the mic path plays alone"]);
    assert!(alone.mix != live);
    assert_eq!(alone.slots[0].stats, played.slots[0].stats);
}

#[test]
fn a_second_profile_changes_only_the_slot_whose_call_left_its_value_out() {
    let mut song = LiveSong::new("profiles", &["rtp", "rtp"], None);
    song.start();
    for (slot, pan, last) in [(0, -1.0, 30), (1, 1.0, 31)] {
        song.line(json!({ "type": "setSlot", "index": slot, "settings": { "pan": pan } }));
        song.params(|p| p.slots[slot].pan = pan);
        song.send(slot, sender(last), 0, &singing(2.0));
    }
    // Slot 0's reverb names its values; slot 1's leaves them to the profile
    song.line(json!({
        "type": "setSlotEffects",
        "index": 0,
        "effects": [{ "kind": "reverb", "enabled": true, "roomSize": 0.5, "mix": 0.25 }]
    }));
    song.line(json!({
        "type": "setSlotEffects",
        "index": 1,
        "effects": [{ "kind": "reverb", "enabled": true }]
    }));
    song.params(|p| {
        for slot in &mut p.slots[..2] {
            slot.effects.reverb = ReverbParams { enabled: true, room_size: 0.5, mix: 0.25 };
        }
    });
    song.play_to(2 * SECOND + SECOND / 2);
    let (dir, live) = song.end();

    let own = replay(&dir, None, None);
    assert!(own.mix == live);
    let mut wetter = profile();
    wetter["slot"]["effects"]["reverb"]["mix"] = json!(0.6);
    let wetter = Profile::parse("wetter", &wetter.to_string()).unwrap();
    let other = replay(&dir, Some(&wetter), None);

    // Slot 0 is hard left and slot 1 hard right, so each channel is one slot
    let channel =
        |mix: &[f32], c: usize| mix.iter().skip(c).step_by(2).copied().collect::<Vec<_>>();
    let gap =
        |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f32::max);
    let (left, right) = (
        gap(&channel(&own.mix, 0), &channel(&other.mix, 0)),
        gap(&channel(&own.mix, 1), &channel(&other.mix, 1)),
    );
    // A hard right pan leaves -147 dB of slot 1 on the left, which is all that moves there
    assert!(left < 1e-5, "slot 0 named its reverb, so it plays as it did: {left}");
    assert!(right > 0.01, "slot 1 took the profile's reverb, so it moves: {right}");
}

// A tone stage replays as it played: its bands as given and the amounts it left out at 0, since
// they are not in the profile. A bundle never holds "match", which the mixer refuses.
#[test]
fn a_tone_stage_replays_with_its_left_out_amounts_at_0() {
    let mut song = LiveSong::new("tone", &["rtp"], None);
    song.start();
    song.send(0, sender(40), 0, &singing(2.0));
    song.line(json!({
        "type": "setSlotEffects",
        "index": 0,
        "effects": [{
            "kind": "tone",
            "enabled": true,
            "bands": [
                { "kind": "low-shelf", "hz": 200, "gainDb": 2 },
                { "kind": "peak", "hz": 3000, "gainDb": 4, "q": 1 }
            ],
            "deEssDb": 6
        }]
    }));
    let bands = [
        ToneBand::LowShelf { hz: 200.0, gain_db: 2.0 },
        ToneBand::Peak { hz: 3000.0, gain_db: 4.0, q: 1.0 },
    ];
    song.params(|p| {
        p.slots[0].effects.tone = ToneParams {
            enabled: true,
            bands: ToneBands::given(&bands).unwrap(),
            de_ess_db: 6.0,
            ..ToneParams::default()
        };
    });
    song.play_to(2 * SECOND + SECOND / 2);
    let (dir, live) = song.end();
    assert!(replay(&dir, None, None).mix == live);
}

#[test]
fn a_tone_stage_the_engine_would_refuse_is_refused_with_the_engines_reason() {
    let mut song = LiveSong::new("tone-refused", &["rtp"], None);
    song.start();
    // As written by hand: the live engine refuses this band in `setSlotEffects`, 2.3 ms late at
    // 96 Hz by decision 0029, so it is a line here and never reaches the engine
    song.line(json!({
        "type": "setSlotEffects",
        "index": 0,
        "effects": [{
            "kind": "tone",
            "enabled": true,
            "bands": [{ "kind": "peak", "hz": 100, "gainDb": 6, "q": 1 }]
        }]
    }));
    song.play_to(SECOND / 2);
    let (dir, _) = song.end();
    let bundle = Bundle::read(&dir).unwrap();
    let error = play(&bundle, &bundle.profile, None).err().unwrap();
    assert!(error.starts_with("the tone stage's bands and air leave 9"), "{error}");
    assert!(error.ends_with("ms late, over the 0.2 ms cap"), "{error}");
}

#[test]
fn a_bundle_cut_short_replays_to_its_last_packet() {
    let mut song = LiveSong::new("cut", &["rtp"], None);
    song.start();
    let sent = singing(1.5);
    let last = sent.last().unwrap().at as u64;
    song.send(0, sender(30), 1_000, &sent);
    song.play_to(2 * SECOND);
    let (dir, live) = song.end();
    // A process that died before `destroy`: everything but the end line
    let events = std::fs::read_to_string(dir.join("events.jsonl")).unwrap();
    let kept: String = events
        .lines()
        .filter(|line| !line.contains("\"end\""))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(dir.join("events.jsonl"), kept).unwrap();

    let played = replay(&dir, None, None);
    assert_eq!(played.notes, ["the bundle has no end line: it was cut short"]);
    let frames = played.mix.len() / 2;
    assert!(frames as u64 >= 1_000 + last && (frames as u64) < 1_000 + last + 128 + 1, "{frames}");
    assert!(played.mix[..] == live[..played.mix.len()]);
}

#[test]
fn a_bundle_of_another_format_is_refused_by_its_number() {
    let song = LiveSong::new("format", &["rtp"], None);
    let (dir, _) = song.end();
    let manifest = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest.replace("\"format\":1", "\"format\":2"))
        .unwrap();
    let error = Bundle::read(&dir).err().unwrap();
    assert_eq!(error, "bundle format 2: this replay reads format 1");
}

#[test]
fn a_profile_is_refused_by_the_name_of_what_is_wrong_with_it() {
    let refused = |change: fn(&mut serde_json::Value)| {
        let mut value = profile();
        change(&mut value);
        Profile::parse("p.json", &value.to_string()).err().unwrap()
    };
    assert!(refused(|p| p["version"] = json!(2)).contains("profile version 2"));
    assert!(
        refused(|p| p["slot"]["effects"]["reverb"]["wet"] = json!(1))
            .contains("unknown field `wet`")
    );
    assert!(
        refused(|p| {
            p["slot"].as_object_mut().unwrap().remove("gain");
        })
        .contains("missing field `gain`")
    );
    assert!(refused(|p| p["slot"]["pan"] = json!(2)).contains("slot.pan"));
    assert!(refused(|p| p["ingest"]["playoutDelayMs"] = json!("soon")).contains("p.json"));
}

/// #204: a slot keeps its sender and turns away the one `resetSlot` released (decision 0028).
/// The bundle holds no address, only each sender's stand-in port, and the replay refuses who the
/// song refused.
#[test]
fn a_replay_refuses_the_senders_the_song_refused_a_reset_slot_included() {
    let sing = |name: &str, second: SocketAddr| {
        let mut song = LiveSong::new(name, &["rtp"], None);
        song.start();
        // The first phone sings for two seconds; the second sends from one second in, refused
        // until the reset at one and a half frees the slot and turns the first away
        song.send(0, sender(30), 0, &singing(2.0));
        song.send(0, second, SECOND, &singing(2.0));
        song.play_to(SECOND * 3 / 2);
        song.line(json!({ "type": "resetSlot", "index": 0 }));
        song.command(|_| Command::ResetSlot { slot: 0 });
        song.play_to(3 * SECOND + SECOND / 2);
        song.end()
    };
    let (dir, live) = sing("senders", sender(31));
    let played = replay(&dir, None, None);
    assert!(played.mix == live, "the replay refuses who the song refused");
    assert!(played.slots[0].stats.pushed > 100);

    // With one address for both phones the slot plays something else, so the test sees the lock
    let (_, as_one) = sing("one-sender", sender(30));
    assert!(as_one != live);
}
