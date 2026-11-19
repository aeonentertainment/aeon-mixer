//! The paired rows on synthetic pairs: a voice as a reference mic heard it, and the same voice
//! through a known colouring as a house mic's slot recorded it, in a bundle these tests write.

use std::collections::BTreeMap;
use std::f32::consts::TAU;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use engine::block::RATE;
use engine::params::ToneBand;
use engine::slot::effects::tone::{Biquad, Section};
use serde_json::{Value, json};
use transport::record::pcap;
use transport::rtp::Header;

use super::frame::BANDS;
use super::gap::BANDS_READ;
use super::reference::Reference;
use super::score;

const SECOND: usize = RATE as usize;
const LINES: usize = 8;
/// The reference's recorder starts this long before the song, as one started by hand would.
const LEAD: usize = 35_000;
/// A phone that is bright at 3 kHz and dull over 8, and the stage's bands that undo it.
const COLOURING: [ToneBand; 2] = [
    ToneBand::Peak { hz: 3000.0, gain_db: 6.0, q: 1.0 },
    ToneBand::HighShelf { hz: 8000.0, gain_db: -6.0 },
];

struct Parts {
    vowels: Vec<f32>,
    esses: Vec<f32>,
    pops: Vec<f32>,
    room: Vec<f32>,
}

/// One line a second: a vowel for 600 ms, on a pitch that climbs from 110 Hz line by line so the
/// spectrum fills, then an s, then the room alone. Lines 1 and 3 open on a 50 Hz pop about 5 dB
/// over the singing, and 5 and 7 on one about 17 dB over it.
fn parts() -> &'static Parts {
    static PARTS: OnceLock<Parts> = OnceLock::new();
    PARTS.get_or_init(|| {
        let mut seed = 7u32;
        let mut noise = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
        };
        let len = LINES * SECOND;
        let room = (0..len).map(|_| 0.0005 * noise()).collect();
        let mut parts =
            Parts { vowels: vec![0.0; len], esses: vec![0.0; len], pops: vec![0.0; len], room };
        let (vowel, pop) = (SECOND * 6 / 10, SECOND * 3 / 100);
        let mut high = [Section::new(), Section::new()];
        for line in 0..LINES {
            let at = line * SECOND;
            let hz = 110.0 * 2f32.powf(line as f32 / 8.0);
            let harmonics = (12_000.0 / hz) as usize;
            for i in 0..vowel {
                let edge = (i.min(vowel - i) as f32 / 960.0).min(1.0);
                let phase = (i as f32 * hz / RATE as f32).fract();
                let buzz: f32 =
                    (1..=harmonics).map(|h| (TAU * h as f32 * phase).sin() / h as f32).sum();
                parts.vowels[at + i] = 0.05 * edge * buzz;
            }
            for i in SECOND * 62 / 100..SECOND * 77 / 100 {
                let hiss = high.iter_mut().fold(f64::from(0.06 * noise()), |y, section| {
                    section.tick(&Biquad::high_pass(5000.0), y)
                });
                parts.esses[at + i] = hiss as f32;
            }
            if line % 2 == 1 {
                let peak = if line < 4 { 0.13 } else { 0.51 };
                for i in 0..pop {
                    let hann = 0.5 - 0.5 * (TAU * i as f32 / pop as f32).cos();
                    parts.pops[at + i] = peak * hann * (TAU * 50.0 * i as f32 / RATE as f32).sin();
                }
            }
        }
        parts
    })
}

/// The take with its s, its pops and its room at these gains, in dB.
fn take(s_db: f32, pops_db: f32, room_db: f32) -> Vec<f32> {
    let Parts { vowels, esses, pops, room } = parts();
    let gain = |db: f32| 10f32.powf(db / 20.0);
    let (s, p, r) = (gain(s_db), gain(pops_db), gain(room_db));
    (0..vowels.len()).map(|i| vowels[i] + s * esses[i] + p * pops[i] + r * room[i]).collect()
}

fn coloured(take: &[f32]) -> Vec<f32> {
    let mut sections = COLOURING.map(|_| Section::new());
    let biquads = COLOURING.map(|band| Biquad::band(&band));
    take.iter()
        .map(|&x| {
            sections.iter_mut().zip(&biquads).fold(f64::from(x), |y, (s, b)| s.tick(b, y)) as f32
        })
        .collect()
}

/// `samples` as a mono, 48 kHz, 32-bit float WAV, as a reference is exported.
fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data = (samples.len() * 4) as u32;
    let mut wav = b"RIFF".to_vec();
    wav.extend((36 + data).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    for field in [16u32, 3 | 1 << 16, RATE, RATE * 4, 4 | 32 << 16] {
        wav.extend(field.to_le_bytes());
    }
    wav.extend(b"data");
    wav.extend(data.to_le_bytes());
    wav.extend(samples.iter().flat_map(|s| s.to_le_bytes()));
    wav
}

/// A bundle of one house mic whose slot received `phone`, with `effects` set on it, and
/// `reference` as its reference mic, started `LEAD` before the song.
fn bundle(name: &str, phone: &[f32], effects: Value, reference: &[f32]) -> PathBuf {
    const US: u64 = 1_800_000_000_000_000;
    const PACKET: usize = 350;
    let dir =
        std::env::temp_dir().join(format!("aeon-scorecard-paired-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("slots")).unwrap();
    std::fs::create_dir_all(dir.join("reference")).unwrap();
    let us = |frame: usize| US + (frame as u64 * 1000) / 48;
    let line = |frame: usize, mut body: Value| {
        body["frame"] = json!(frame + 384);
        body["us"] = json!(us(frame));
        body.to_string() + "\n"
    };
    let manifest = json!({
        "format": 1, "package": "0.9.0", "platform": "test", "frame": 384, "us": us(0), "backing": null,
        "output": { "latencyMs": 0, "name": null, "periodFrames": 128, "sampleRate": 48000 },
        "slots": [{ "index": 0, "kind": "l16", "recorded": true }],
    });
    let write = |file: &str, text: String| std::fs::write(dir.join(file), text).unwrap();
    write("manifest.json", manifest.to_string());
    write("stats.jsonl", String::new());
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/bundles/echo/profile.json");
    write("profile.json", std::fs::read_to_string(fixture).unwrap());
    let effects = json!({ "type": "setSlotEffects", "index": 0, "effects": effects });
    let start = json!({ "type": "start" });
    let end = json!({ "type": "end" });
    write(
        "events.jsonl",
        line(0, effects) + &line(0, start) + &line(phone.len() + SECOND / 2, end),
    );

    let mut capture = pcap::global_header().to_vec();
    for (k, pcm) in phone.as_chunks::<PACKET>().0.iter().enumerate() {
        let header = Header {
            marker: false,
            payload_type: 96,
            seq: k as u16,
            timestamp: (k * PACKET) as u32,
            ssrc: 1,
        };
        let mut datagram = vec![0u8; 12];
        header.write((&mut datagram[..12]).try_into().unwrap());
        for sample in pcm {
            let bytes = ((sample.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_be_bytes();
            datagram.extend_from_slice(&bytes);
            datagram.extend_from_slice(&bytes);
        }
        let at = Duration::from_micros(us((k + 1) * PACKET + 48));
        pcap::append_record(&mut capture, at, (Ipv4Addr::UNSPECIFIED, 40_000), 5004, &datagram);
    }
    std::fs::write(dir.join("slots/0.pcap"), capture).unwrap();

    let mut recorded = parts().room[..LEAD].to_vec();
    recorded.extend_from_slice(reference);
    std::fs::write(dir.join("reference/0.wav"), wav_bytes(&recorded)).unwrap();
    write(
        "reference/0.json",
        json!({ "format": 1, "mic": "a studio mic", "distanceCm": 10 }).to_string(),
    );
    dir
}

fn scored(dir: &Path) -> BTreeMap<String, f64> {
    let paired = score("pair", dir, None).unwrap();
    for line in &paired.lines {
        println!("{line}");
    }
    paired.numbers
}

fn near(numbers: &BTreeMap<String, f64>, name: &str, want: f64, within: f64) {
    let got = numbers[name];
    assert!(
        (got - want).abs() <= within,
        "{name} read {got:.2}, where {want:.2} was due: {numbers:#?}"
    );
}

// The colouring's gap in each third octave is its curve at the band's centre, as the cookbook draws
// it. A band's reading averages the curve across the band, which moves it from the centre's value
// by at most 0.12 dB, at the top of the 3 kHz peak, so each row is held to 0.15 dB. The cookbook's
// peak and shelf at minus a gain are the reciprocals of the same at plus it, so the stage with the
// inverse bands leaves only the rounding of 16-bit samples
#[test]
fn spectral_rows_read_a_known_colouring_and_its_inverse_bands_bring_the_phone_back() {
    let reference = take(0.0, 0.0, 0.0);
    let inverse = json!([{ "kind": "tone", "enabled": true, "bands": [
        { "kind": "peak", "hz": 3000, "gainDb": -6, "q": 1 },
        { "kind": "high-shelf", "hz": 8000, "gainDb": 6 },
    ]}]);
    let numbers = scored(&bundle("colouring", &coloured(&reference), inverse, &reference));

    let gaps: [f64; BANDS] = std::array::from_fn(|b| {
        let hz = 1000.0 * 10f64.powf((b as f64 - 10.0) / 10.0);
        COLOURING.iter().map(|band| Biquad::band(band).response(hz).0).sum()
    });
    let level = gaps.iter().sum::<f64>() / BANDS as f64;
    let matched = gaps.map(|gap| gap - level);
    let distance = (matched.iter().map(|g| g * g).sum::<f64>() / BANDS as f64).sqrt();
    near(&numbers, "slot0.pairDistanceDb.off", distance, 0.15);
    for (row, _, bands) in BANDS_READ {
        let want = matched[bands.clone()].iter().sum::<f64>() / bands.len() as f64;
        near(&numbers, &format!("slot0.{row}.off"), want, 0.15);
    }
    assert!(numbers["slot0.pairDistanceDb.on"] < 0.1, "{numbers:#?}");
}

// The s 6 dB down and the room 10 dB up, with the singing as it was: in an s the hiss stands 30 dB
// over the room, and the room 50 dB under the singing, so neither moves the other's row by 0.01 dB
#[test]
fn sibilance_and_the_room_read_their_known_gaps() {
    let reference = take(0.0, 0.0, 0.0);
    let numbers = scored(&bundle("sibilance", &take(-6.0, 0.0, 10.0), json!([]), &reference));
    let mic = |row: &str| numbers[&format!("slot0.{row}.reference")];
    near(&numbers, "slot0.sibilanceDb.off", mic("sibilanceDb") - 6.0, 0.1);
    near(&numbers, "slot0.noiseFloorDb.off", mic("noiseFloorDb") + 10.0, 0.1);
    near(&numbers, "slot0.plosivesPerMinute.off", mic("plosivesPerMinute"), 0.0);
    near(&numbers, "slot0.pairDistanceDb.off", 0.0, 0.1);
    assert!(
        !numbers.contains_key("slot0.pairDistanceDb.on"),
        "the stage was never on: {numbers:#?}"
    );
}

// Pops 12 dB down: the two that stood about 5 dB over the singing fall under it and the two at
// about 17 dB stay over, so the phone counts half the mic's four
#[test]
fn pops_cut_under_100_hz_count_half() {
    let reference = take(0.0, 0.0, 0.0);
    let numbers = scored(&bundle("pops", &take(0.0, -12.0, 0.0), json!([]), &reference));
    let mic = numbers["slot0.plosivesPerMinute.reference"];
    let seconds = 4.0 * 60.0 / mic;
    assert!((8.0..9.0).contains(&seconds), "four pops over {seconds:.2} s: {numbers:#?}");
    near(&numbers, "slot0.plosivesPerMinute.off", mic / 2.0, 0.01);
}

#[test]
fn a_bundle_with_no_reference_scores_none_and_says_so() {
    let duet = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/bundles/duet");
    let paired = score("duet", &duet, None).unwrap();
    assert!(paired.numbers.is_empty());
    assert_eq!(
        paired.lines,
        ["duet: no reference mic in reference/, so the paired rows were not scored"]
    );
}

#[test]
fn a_reference_of_an_empty_room_scores_none_and_says_why() {
    let dir = bundle("empty", &take(0.0, 0.0, 0.0), json!([]), &parts().room);
    let paired = score("pair", &dir, None).unwrap();
    assert!(paired.numbers.is_empty(), "{:#?}", paired.numbers);
    assert!(paired.lines[0].contains("does not line up"), "{:?}", paired.lines);
}

/// A folder holding `reference/` with `files`, each a reference's WAV or its `.json` by extension,
/// or empty.
fn references(name: &str, files: &[&str]) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("aeon-scorecard-refs-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("reference")).unwrap();
    for file in files {
        let bytes = match Path::new(file).extension().and_then(|e| e.to_str()) {
            Some("wav" | "WAV") => wav_bytes(&[0.0; 4]),
            Some("json" | "JSON") => {
                json!({ "format": 1, "mic": "a studio mic", "distanceCm": 10 })
                    .to_string()
                    .into_bytes()
            }
            _ => Vec::new(),
        };
        std::fs::write(dir.join("reference").join(file), bytes).unwrap();
    }
    dir
}

// A recorder may write `.WAV`, and a person may name a description `.JSON`. Each test makes
// exactly these names, so a filesystem that keeps case finds only what is there
#[test]
fn a_reference_is_read_whatever_the_case_of_its_extensions() {
    for files in [["0.WAV", "0.json"], ["0.wav", "0.JSON"]] {
        let (found, skipped) = Reference::find(&references(files[1], &files)).unwrap();
        assert_eq!(found.iter().map(|r| r.slot).collect::<Vec<_>>(), [0], "{files:?}");
        assert_eq!(skipped, Vec::<String>::new(), "{files:?}");
    }
}

// `01.wav` and `+1.wav` parse as slot 1, so either would be read as another file's slot
#[test]
fn a_reference_not_named_by_its_slot_is_refused_by_its_own_name() {
    for file in ["01", "+1"] {
        let dir =
            references(file, &[&format!("{file}.wav"), &format!("{file}.json"), "1.wav", "1.json"]);
        let Err(refused) = Reference::find(&dir) else { panic!("{file}.wav was read") };
        assert_eq!(
            refused,
            format!("reference/{file}.wav: a reference is named by its slot, as 0.wav")
        );
    }
}

// A note kept beside the reference, and what macOS leaves when a recorder's card is copied
#[test]
fn a_note_or_a_hidden_file_beside_a_reference_does_not_stop_it_being_read() {
    let dir =
        references("beside", &["0.wav", "0.json", "mic.json", "take.wav", "._0.wav", ".DS_Store"]);
    let (found, skipped) = Reference::find(&dir).unwrap();
    assert_eq!(found.iter().map(|r| r.slot).collect::<Vec<_>>(), [0]);
    assert_eq!(
        skipped,
        [
            "reference/mic.json was skipped: a reference is a .wav and a .json named by its slot",
            "reference/take.wav was skipped: a reference is a .wav and a .json named by its slot",
        ]
    );
}

#[test]
fn files_that_are_not_references_are_named_and_never_called_no_reference() {
    let dir = references("skipped", &["notes.txt", "0.WAV", "1.JSON"]);
    let paired = score("pair", &dir, None).unwrap();
    assert_eq!(
        paired.lines,
        [
            "pair: reference/notes.txt was skipped: a reference is a .wav and a .json named by its slot",
            "pair: reference/0.WAV was skipped: it has no description beside it, a .json of the same name",
            "pair: reference/1.JSON was skipped: it has no recording beside it, a .wav of the same name",
            "pair: no file in reference/ is a reference, so the paired rows were not scored",
        ]
    );
}
