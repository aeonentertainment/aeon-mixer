//! measure-corrector: the pitch corrector's numbers beside the GStreamer baseline, a machine
//! readable result to diff, and the blind-listen set. It runs the real slot chain on the offline
//! driver, never a device. README.md beside this crate says how to use it and how far to trust it;
//! docs/decisions/0005-corrector-measurement.md says why it is built this way.

mod added_latency;
mod answer_key;
mod artefacts;
mod audio;
mod bend;
mod chain;
mod clicks;
mod clip_source;
mod condition;
mod contour;
mod cpu;
mod curve;
mod degrader;
mod diff;
mod expression;
mod ingest;
mod key_report;
mod listen;
mod listen_take;
mod melody_sync;
mod music;
mod pitch_error;
mod report;
mod rtp;
mod setup;
mod spectral;
mod sung_clip;
mod sung_rows;
mod synthetic;
mod take;
mod take_delay;
mod truth;
mod variant;
mod vibrato;
mod vocadito;
mod vocal_delays;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use engine::slot::guide::{Guide, Note};
use serde_json::{Map, Value, json};

use crate::chain::frames_of_ms;
use crate::condition::Condition;
use crate::listen::ListenSource;
use crate::music::key_mask;
use crate::sung_clip::{SungClip, load_take, parse_guide_json};
use crate::variant::Variant;

const USAGE: &str = "\
usage:
  measure-corrector run --vocadito DIR [--golden DIR] [--clips 2,6,10,19] [--variant L:MODE:S:R]...
                        [--phone WAV --phone-guide JSON]... [--song WAV --song-guide JSON]...
                        [--no-cpu] [--out RESULT.json]
  measure-corrector table RESULT.json
  measure-corrector diff BEFORE.json AFTER.json [--tolerance N]
  measure-corrector listen --vocadito DIR --golden DIR --out CLIPS --key-out KEYDIR [--seed N]
                           [--phone WAV --phone-guide JSON]...
  measure-corrector score --sheet FILLED.csv --key answer-key.json

  --vocadito DIR   vocadito unzipped, from zenodo.org/records/5578807, outside this repo
  --golden DIR     Aeon Karaoke's app/cloud/pipeline/fixtures/golden: vocals.mp3, backing.mp3,
                   pitch-guide.json
  --variant        one more corrector to compare: label:off|key|melody:strength:retune_ms
  --phone WAV      a person's recorded take; its guide file comes from the same song. Repeats,
                   each with its own --phone-guide, in the same order
  --song WAV       a dry studio vocal, the answer key's song, with the guide made from it in
                   --song-guide. Repeats; the golden vocal is one when --golden is given
  --tolerance N    leave out numbers that moved by N or less (default 0)";

/// The clips `auto-tune.md`'s rows read.
const CLIPS: [u32; 4] = [2, 6, 10, 19];
/// A singer in none of the listening sources, for the training pair.
const TRAINING_CLIP: u32 = 1;
/// The golden vocal's key, from its guide, and the seconds the live rows sampled.
const GOLDEN_KEY_FILE: &str = "pitch-guide.json";

/// Flags with values, in order; a flag may repeat.
struct Flags {
    positional: Vec<String>,
    named: Vec<(String, String)>,
    switches: Vec<String>,
}

impl Flags {
    fn parse(args: impl Iterator<Item = String>, switches: &[&str]) -> Result<Self, String> {
        let mut flags = Self { positional: Vec::new(), named: Vec::new(), switches: Vec::new() };
        let mut args = args;
        while let Some(arg) = args.next() {
            if !arg.starts_with("--") {
                flags.positional.push(arg);
            } else if switches.contains(&arg.as_str()) {
                flags.switches.push(arg);
            } else {
                let value = args.next().ok_or(format!("{arg} needs a value"))?;
                flags.named.push((arg, value));
            }
        }
        Ok(flags)
    }

    fn get(&self, name: &str) -> Option<&str> {
        self.named.iter().find(|(flag, _)| flag == name).map(|(_, value)| value.as_str())
    }

    fn all(&self, name: &str) -> impl Iterator<Item = &str> {
        self.named.iter().filter(move |(flag, _)| flag == name).map(|(_, value)| value.as_str())
    }

    fn path(&self, name: &str) -> Result<PathBuf, String> {
        self.get(name).map(PathBuf::from).ok_or(format!("{name} is required"))
    }

    fn has(&self, name: &str) -> bool {
        self.switches.iter().any(|flag| flag == name)
    }
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn golden_guide(dir: &Path) -> Result<Guide, String> {
    let path = dir.join(GOLDEN_KEY_FILE);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (key, notes) = parse_guide_json(&text)?;
    let notes = notes.iter().map(|note| Note {
        midi: note.midi,
        start_frame: frames_of_ms(note.start_ms),
        end_frame: frames_of_ms(note.end_ms),
    });
    Ok(Guide { notes: notes.collect(), key_mask: key_mask(&key)? })
}

/// `flag` and `guide_flag` paired in the order given, each pair once.
fn paired<'a>(
    flags: &'a Flags,
    flag: &str,
    guide_flag: &str,
) -> Result<Vec<(&'a str, &'a str)>, String> {
    let (takes, guides): (Vec<&str>, Vec<&str>) =
        (flags.all(flag).collect(), flags.all(guide_flag).collect());
    if takes.len() != guides.len() {
        return Err(format!("each {flag} needs its own {guide_flag}"));
    }
    Ok(takes.into_iter().zip(guides).collect())
}

/// The takes people recorded, in the order given: `phone`, then `phone-2` and on.
fn phone_takes(flags: &Flags) -> Result<Vec<(SungClip, f64)>, String> {
    paired(flags, "--phone", "--phone-guide")?
        .into_iter()
        .enumerate()
        .map(|(index, (wav, guide))| {
            let name = if index == 0 { "phone".to_owned() } else { format!("phone-{}", index + 1) };
            load_take(&name, Path::new(wav), Path::new(guide))
        })
        .collect()
}

/// Studio vocals with the guide made from them, the answer key's songs: the golden vocal, and
/// each `--song` named by its file.
fn songs(flags: &Flags, golden: Option<&Path>) -> Result<Vec<SungClip>, String> {
    let mut pairs: Vec<(String, PathBuf, PathBuf)> = golden
        .map(|dir| ("golden".to_owned(), dir.join("vocals.mp3"), dir.join(GOLDEN_KEY_FILE)))
        .into_iter()
        .collect();
    for (vocal, guide) in paired(flags, "--song", "--song-guide")? {
        let stem = Path::new(vocal).file_stem().map_or("song".into(), |s| s.to_string_lossy());
        pairs.push((format!("song-{stem}"), PathBuf::from(vocal), PathBuf::from(guide)));
    }
    pairs
        .into_iter()
        .map(|(name, vocal, guide)| {
            let text =
                std::fs::read_to_string(&guide).map_err(|e| format!("{}: {e}", guide.display()))?;
            let (key, notes) = parse_guide_json(&text)?;
            let samples = audio::decode_mono(&vocal)?;
            Ok(SungClip { name, samples, notes, second_notes: None, truth: None, key })
        })
        .collect()
}

fn run_rows(flags: &Flags) -> Result<Value, String> {
    let vocadito_dir = flags.path("--vocadito")?;
    let golden = flags.get("--golden").map(PathBuf::from);
    let clip_numbers: Vec<u32> = match flags.get("--clips") {
        Some(text) => text
            .split(',')
            .map(|clip| clip.trim().parse().map_err(|_| format!("{clip} is not a clip")))
            .collect::<Result<_, _>>()?,
        None => CLIPS.to_vec(),
    };
    let mut variants = Variant::standard();
    for text in flags.all("--variant") {
        variants.push(Variant::parse(text)?);
    }
    let conditions = Condition::standard();

    let golden_vocal = match &golden {
        Some(dir) => Some(audio::decode_mono(&dir.join("vocals.mp3"))?),
        None => None,
    };
    let guide = golden.as_deref().map(golden_guide).transpose()?;
    eprintln!("latency and sync");
    let latency = added_latency::rows(golden_vocal.as_deref());
    let sync = melody_sync::rows(guide.as_ref());
    let voices = synthetic::rows();

    let mut clips = Map::new();
    let mut windows = Map::new();
    let mut answer_keys = Map::new();
    let mut first_clip_samples = None;
    for number in clip_numbers {
        let clip = vocadito::load(&vocadito_dir, number)?;
        eprintln!("{} ({:.0} s)", clip.name, clip.seconds());
        clips.insert(clip.name.clone(), sung_rows::clip_rows(&clip, &conditions, &variants)?);
        if let Some(truth) = &clip.truth {
            windows.insert(clip.name.clone(), sung_rows::window_rows(&clip, truth));
        }
        answer_keys.insert(clip.name.clone(), answer_key::rows(&clip, &variants)?);
        first_clip_samples.get_or_insert(clip.samples);
    }
    for song in songs(flags, golden.as_deref())? {
        eprintln!("{} as an answer key ({:.0} s)", song.name, song.seconds());
        answer_keys.insert(song.name.clone(), answer_key::rows(&song, &variants)?);
    }
    let mut phone_alignment = Map::new();
    for (clip, offset) in phone_takes(flags)? {
        eprintln!("{} ({:.0} s), guide found {offset:+.0} ms in", clip.name, clip.seconds());
        let through_a_phone: Vec<Condition> = conditions
            .iter()
            .copied()
            .filter(|condition| matches!(condition, Condition::Opus { .. }))
            .collect();
        clips.insert(clip.name.clone(), sung_rows::clip_rows(&clip, &through_a_phone, &variants)?);
        phone_alignment.insert(clip.name.clone(), json!(offset));
    }

    let cpu = if flags.has("--no-cpu") {
        Value::Null
    } else {
        eprintln!("cpu");
        let vocal = golden_vocal.as_deref().or(first_clip_samples.as_deref());
        vocal.map_or(Value::Null, cpu::rows)
    };

    Ok(json!({
        "latency": latency,
        "sync": sync,
        "cpu": cpu,
        "clips": clips,
        "detector_windows": windows,
        "known_voices": voices,
        "answer_key": answer_keys,
        "phone_guide_offset_ms": phone_alignment,
    }))
}

fn listen_sources(
    flags: &Flags,
    golden: &Path,
    vocadito_dir: &Path,
) -> Result<Vec<ListenSource>, String> {
    let mut sources = Vec::new();
    for clip in CLIPS {
        sources.push(ListenSource {
            name: format!("vocadito-{clip}"),
            clip: vocadito::load(vocadito_dir, clip)?,
            backing: None,
            attribution: Some(vocadito::ATTRIBUTION),
        });
    }
    let text = std::fs::read_to_string(golden.join(GOLDEN_KEY_FILE)).map_err(|e| e.to_string())?;
    let (key, notes) = parse_guide_json(&text)?;
    sources.push(ListenSource {
        name: "golden-with-backing".to_owned(),
        clip: SungClip {
            name: "golden".to_owned(),
            samples: audio::decode_mono(&golden.join("vocals.mp3"))?,
            notes,
            second_notes: None,
            truth: None,
            key,
        },
        backing: Some(audio::decode_stereo(&golden.join("backing.mp3"))?),
        attribution: None,
    });
    for (clip, _) in phone_takes(flags)? {
        sources.push(ListenSource {
            name: clip.name.clone(),
            clip,
            backing: None,
            attribution: None,
        });
    }
    Ok(sources)
}

/// A seed nobody picked, for a set nobody should be able to guess. The key records it.
fn fresh_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0x5EED, |elapsed| elapsed.subsec_nanos() as u64 ^ elapsed.as_secs())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args.next().ok_or(USAGE)?;
    let flags = Flags::parse(args, &["--no-cpu"])?;
    match command.as_str() {
        "run" => {
            let result = run_rows(&flags)?;
            println!("{}", report::table(&result));
            println!("{}", report::known_voices_table(&result));
            println!("{}", report::expression_table(&result));
            println!("{}", key_report::table(&result));
            println!("{}", key_report::artefacts_table(&result));
            if let Some(out) = flags.get("--out") {
                let text = serde_json::to_string_pretty(&result).map_err(|e| e.to_string())?;
                std::fs::write(out, text).map_err(|e| format!("{out}: {e}"))?;
                eprintln!("wrote {out}");
            }
        }
        "table" => {
            let path = flags.positional.first().ok_or(USAGE)?;
            let result = read_json(Path::new(path))?;
            println!("{}", report::table(&result));
            println!("{}", report::known_voices_table(&result));
            println!("{}", report::expression_table(&result));
            println!("{}", key_report::table(&result));
            println!("{}", key_report::artefacts_table(&result));
        }
        "diff" => {
            let [before, after] = &flags.positional[..] else { return Err(USAGE.to_owned()) };
            let tolerance = match flags.get("--tolerance") {
                Some(text) => text.parse().map_err(|_| format!("{text} is not a tolerance"))?,
                None => 0.0,
            };
            let changes = diff::compare(
                &read_json(Path::new(before))?,
                &read_json(Path::new(after))?,
                tolerance,
            );
            print!("{}", diff::print(&changes));
        }
        "listen" => {
            let vocadito_dir = flags.path("--vocadito")?;
            let golden = flags.path("--golden")?;
            let (clips, key) = (flags.path("--out")?, flags.path("--key-out")?);
            listen::check_places(&clips, &key)?;
            let seed = match flags.get("--seed") {
                Some(text) => text.parse().map_err(|_| format!("{text} is not a seed"))?,
                None => fresh_seed(),
            };
            let sources = listen_sources(&flags, &golden, &vocadito_dir)?;
            let training = vocadito::load(&vocadito_dir, TRAINING_CLIP)?;
            listen::render_set(&sources, Some(&training), &clips, &key, seed)?;
            println!("clips, sheet and procedure: {}", clips.display());
            println!("answer key, kept apart:     {}", key.display());
        }
        "score" => {
            let sheet = flags.path("--sheet")?;
            let text =
                std::fs::read_to_string(&sheet).map_err(|e| format!("{}: {e}", sheet.display()))?;
            let responses = listen::parse_sheet(&text)?;
            let key = read_json(&flags.path("--key")?)?;
            print!("{}", listen::score_report(&responses, &key)?);
        }
        _ => return Err(USAGE.to_owned()),
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(reason) => {
            eprintln!("{reason}");
            ExitCode::FAILURE
        }
    }
}

// Tests wrap every realtime call in `assert_no_alloc`; this allocator is what makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sung_clip::MeantNote;

    // The whole point of a diff: on one machine, two runs of every row but CPU are the same number
    #[test]
    fn a_run_is_deterministic() {
        let clip = || SungClip {
            name: "known".to_owned(),
            samples: synthetic::harmonic_voice(48_000, |ms| 220.0 + 8.0 * (ms / 150.0).sin()),
            notes: vec![MeantNote { midi: 57, start_ms: 100.0, end_ms: 900.0 }],
            second_notes: None,
            truth: None,
            key: "C".to_owned(),
        };
        let conditions = [
            Condition::Direct,
            Condition::Noise { snr_db: 20.0 },
            Condition::Opus { impairment: rtp::Impairment::WIFI, dtx: true },
        ];
        let once = sung_rows::clip_rows(&clip(), &conditions, &Variant::standard()).unwrap();
        let again = sung_rows::clip_rows(&clip(), &conditions, &Variant::standard()).unwrap();
        assert_eq!(once, again);
        // And deterministic does not mean stuck: the conditions and variants differ from each other
        let direct = &once["direct"]["variants"];
        assert_ne!(direct["off"]["median_cents"], direct["melody-1.0"]["median_cents"]);
    }
}
