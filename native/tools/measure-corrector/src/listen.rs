//! The blind-listen set: every source as off, key and melody at the natural settings under
//! shuffled labels, level-matched, with the answer key written somewhere else. And the score, once
//! people have filled the sheet in.
//!
//! The listener sees only `LISTENING.md`, the sheet and the clips. Takes are written in label
//! order with no metadata, so neither a file's name, size nor date says which take it is.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use engine::backing::Planar;
use engine::block::RATE;
use engine::params::CorrectorParams;
use engine::slot::guide::Guide;
use kernels::EVERY_PITCH_CLASS;
use serde_json::{Map, Value, json};

use crate::audio::write_wav;
use crate::chain::{Input, Mix, Setup, run};
use crate::contour;
use crate::expression::{Envelope, Expression, compare};
use crate::listen_take::Take;
use crate::music::{Lcg, key_mask};
use crate::sung_clip::SungClip;
use crate::vocadito;

pub const PROCEDURE: &str = include_str!("../LISTENING.md");

const SECONDS: usize = 20;
const FADE_FRAMES: usize = 480;
const LABELS: [&str; 3] = ["A", "B", "C"];
const SHEET_HEADER: &str = "listener,source,rank_A,rank_B,rank_C,warble_A,warble_B,warble_C,robotic_A,robotic_B,robotic_C,notes";

/// strength 0.6, retune 40 ms: what #184's clips and the live procedure call natural.
const NATURAL: CorrectorParams = CorrectorParams { enabled: true, strength: 0.6, retune_ms: 40.0 };
const OFF: CorrectorParams = CorrectorParams { enabled: false, strength: 0.0, retune_ms: 40.0 };

pub struct ListenSource {
    /// The directory the clips go in; names a source, never a take.
    pub name: String,
    pub clip: SungClip,
    /// Mixed under the voice, for a source heard over its song.
    pub backing: Option<Planar>,
    pub attribution: Option<&'static str>,
}

/// `path` as far as it exists, resolved, so `..` and symlinks cannot hide where it is.
fn resolved(path: &Path) -> PathBuf {
    // Absolute first: a relative path that does not exist yet would otherwise stay relative
    let mut existing = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(ToOwned::to_owned) else { break };
        rest.push(name);
        existing.pop();
    }
    let mut full = existing.canonicalize().unwrap_or(existing);
    full.extend(rest.into_iter().rev());
    full
}

fn inside_a_repo(path: &Path) -> bool {
    path.ancestors().any(|ancestor| ancestor.join(".git").exists())
}

/// Clips and their key go apart and outside any repo, and nothing already there is overwritten.
pub fn check_places(clips: &Path, key: &Path) -> Result<(), String> {
    let (clips, key) = (resolved(clips), resolved(key));
    if key.starts_with(&clips) || clips.starts_with(&key) {
        return Err("the answer key must not sit with the clips".to_owned());
    }
    for place in [&clips, &key] {
        if inside_a_repo(place) {
            return Err(format!("{} is inside a repo; audio stays out of it", place.display()));
        }
        if std::fs::read_dir(place).is_ok_and(|mut entries| entries.next().is_some()) {
            return Err(format!("{} already holds files; give an empty place", place.display()));
        }
    }
    Ok(())
}

/// Each source's labels shuffled on its own: the takes in `A, B, C` order.
pub fn shuffled_takes(seed: u64) -> impl FnMut() -> [Take; 3] {
    let mut rng = Lcg(seed);
    move || {
        let mut takes = [Take::Off, Take::Key, Take::Melody];
        for last in (1..takes.len()).rev() {
            takes.swap(last, rng.below(last as u32 + 1) as usize);
        }
        takes
    }
}

struct Rendered {
    left: Vec<f32>,
    right: Vec<f32>,
    measured: Expression,
}

fn render_take(source: &ListenSource, take: Take) -> Result<Rendered, String> {
    let clip = &source.clip;
    let frames = clip.samples.len().min(SECONDS * RATE as usize);
    let samples = clip.samples[..frames].to_vec();
    let key = key_mask(&clip.key)?;
    let (params, guide) = match take {
        Take::Off => (OFF, clip.guide(key)),
        Take::Key => (NATURAL, Guide { notes: Vec::new(), key_mask: key }),
        Take::Melody => (NATURAL, clip.guide(key)),
    };
    // A voice over a song leaves it room; both come down so the clipper stays out of it
    let (voice_gain, backing) = match &source.backing {
        Some(backing) => {
            let quieter =
                |side: &[f32]| -> Vec<f32> { side.iter().take(frames).map(|s| s * 0.5).collect() };
            let scaled = Planar { left: quieter(&backing.left), right: quieter(&backing.right) };
            (0.6, Some(scaled))
        }
        None => (0.9, None),
    };
    let mut setup = Setup::new(Input::Samples(samples.clone()), frames, params);
    setup.guide = Some(guide);
    setup.mix = Some(Mix { backing, voice_gain });
    let take_out = run(setup);

    // The movement is read from the voice alone, so the song under it does not count as wobble
    let mut voice_only = Setup::new(Input::Samples(samples.clone()), frames, params);
    voice_only.guide = Some(match take {
        Take::Off | Take::Melody => clip.guide(key),
        Take::Key => Guide { notes: Vec::new(), key_mask: key },
    });
    let voice = run(voice_only).voice();
    let notes = clip.second_notes.as_deref().unwrap_or(&clip.notes);
    let measured = compare(
        &contour::read(&samples, 0),
        &contour::read(&voice, 0),
        &Envelope::of(&samples, 0),
        &Envelope::of(&voice, 0),
        notes,
    );
    Ok(Rendered { left: take_out.left, right: take_out.right, measured })
}

fn rms(left: &[f32], right: &[f32]) -> f32 {
    let energy: f32 = left.iter().chain(right).map(|s| s * s).sum();
    (energy / (left.len() + right.len()).max(1) as f32).sqrt()
}

/// Brings the takes to one level, the loudest level the peakiest take allows, and fades the cut
/// ends, so loudness cannot give a take away.
fn level_match(takes: &mut [Rendered]) {
    let target = takes
        .iter()
        .map(|take| {
            let peak = take.left.iter().chain(&take.right).fold(0.0f32, |p, s| p.max(s.abs()));
            0.98 * rms(&take.left, &take.right) / peak.max(1e-6)
        })
        .fold(f32::INFINITY, f32::min);
    for take in takes {
        let gain = target / rms(&take.left, &take.right).max(1e-6);
        let frames = take.left.len();
        for (index, (l, r)) in take.left.iter_mut().zip(&mut take.right).enumerate() {
            let edge = index.min(frames - 1 - index).min(FADE_FRAMES) as f32 / FADE_FRAMES as f32;
            *l *= gain * edge;
            *r *= gain * edge;
        }
    }
}

fn measures_json(measured: &Expression) -> Value {
    json!({
        "motion_kept": measured.motion_kept,
        "motion_added_p95_cents": measured.motion_added_p95,
        "vibrato_kept": measured.vibrato_kept,
        "envelope_added_p95_db": measured.envelope_added_p95_db,
    })
}

/// Writes the clips, the procedure and a blank sheet under `clips`, and the answer key under `key`.
pub fn render_set(
    sources: &[ListenSource],
    training: Option<&SungClip>,
    clips: &Path,
    key: &Path,
    seed: u64,
) -> Result<(), String> {
    check_places(clips, key)?;
    let make = |place: &Path| {
        std::fs::create_dir_all(place).map_err(|e| format!("{}: {e}", place.display()))
    };
    make(clips)?;
    make(key)?;
    let write = |path: PathBuf, text: String| {
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))
    };

    let mut next_takes = shuffled_takes(seed);
    let mut answers = Vec::new();
    let mut sheet = format!("{SHEET_HEADER}\n");
    let mut attributions = BTreeSet::new();
    for source in sources {
        let order = next_takes();
        let mut rendered =
            order.iter().map(|take| render_take(source, *take)).collect::<Result<Vec<_>, _>>()?;
        level_match(&mut rendered);
        let directory = clips.join(&source.name);
        make(&directory)?;
        let mut labels = Map::new();
        let mut measures = Map::new();
        for ((label, take), render) in LABELS.iter().zip(order).zip(&rendered) {
            write_wav(&directory.join(format!("{label}.wav")), &render.left, &render.right)?;
            labels.insert((*label).to_owned(), json!(take.name()));
            measures.insert(take.name().to_owned(), measures_json(&render.measured));
        }
        answers.push(json!({
            "source": source.name,
            "key": source.clip.key,
            "labels": labels,
            "measures": measures,
        }));
        writeln!(sheet, "YOUR-NAME,{},,,,,,,,,,", source.name).expect("a string takes writes");
        attributions.extend(source.attribution);
    }

    if let Some(clip) = training {
        write_training(clip, &clips.join("training"))?;
        attributions.insert(vocadito::ATTRIBUTION);
    }
    write(clips.join("LISTENING.md"), PROCEDURE.to_owned())?;
    write(clips.join("answer-sheet.csv"), sheet)?;
    let attribution = attributions.into_iter().collect::<Vec<_>>().join("\n\n");
    write(clips.join("ATTRIBUTION.md"), format!("{attribution}\n"))?;

    let answer = json!({
        "note": "Keep this away from listeners until every sheet is in.",
        "seed": seed,
        "settings": { "strength": NATURAL.strength, "retune_ms": NATURAL.retune_ms },
        "sources": answers,
    });
    let pretty = serde_json::to_string_pretty(&answer).map_err(|e| e.to_string())?;
    write(key.join("answer-key.json"), pretty)
}

/// A pair that is not part of the test: the voice, and the same voice hard-tuned, so a listener
/// can hear the kind of thing the sheet asks about before they judge anything.
fn write_training(clip: &SungClip, directory: &Path) -> Result<(), String> {
    std::fs::create_dir_all(directory).map_err(|e| format!("{}: {e}", directory.display()))?;
    let frames = clip.samples.len().min(10 * RATE as usize);
    let samples = clip.samples[..frames].to_vec();
    let hard = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };
    let mut setup = Setup::new(Input::Samples(samples.clone()), frames, hard);
    setup.guide = Some(Guide { notes: Vec::new(), key_mask: EVERY_PITCH_CLASS });
    let hard_tuned = run(setup).voice();
    let scale = |signal: &[f32]| -> Vec<f32> {
        let peak = signal.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        signal.iter().map(|s| s * 0.9 / peak.max(1e-6)).collect()
    };
    let dry = scale(&samples);
    write_wav(&directory.join("as-sung.wav"), &dry, &dry)?;
    let hard_tuned = scale(&hard_tuned);
    write_wav(&directory.join("hard-tuned.wav"), &hard_tuned, &hard_tuned)
}

/// One row of a filled-in sheet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub listener: String,
    pub source: String,
    pub ranks: [u32; 3],
    pub warble: [bool; 3],
    pub robotic: [bool; 3],
}

pub fn parse_sheet(text: &str) -> Result<Vec<Response>, String> {
    let yes = |field: &str| matches!(field.trim().to_lowercase().as_str(), "y" | "yes");
    text.lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty() && !line.starts_with("YOUR-NAME"))
        .map(|line| {
            let fields: Vec<&str> = line.splitn(12, ',').collect();
            let [listener, source, a, b, c, wa, wb, wc, ra, rb, rc, ..] = fields[..] else {
                return Err(format!("a short sheet line: {line}"));
            };
            let rank =
                |field: &str| field.trim().parse().map_err(|_| format!("a bad rank in: {line}"));
            Ok(Response {
                listener: listener.trim().to_owned(),
                source: source.trim().to_owned(),
                ranks: [rank(a)?, rank(b)?, rank(c)?],
                warble: [yes(wa), yes(wb), yes(wc)],
                robotic: [yes(ra), yes(rb), yes(rc)],
            })
        })
        .collect()
}

/// A source's labels from the key: which of A, B, C each take is.
fn labels_of(entry: &Value) -> Result<BTreeMap<&str, usize>, String> {
    LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            entry["labels"][label]
                .as_str()
                .map(|take| (take, index))
                .ok_or(format!("the key has no {label} for {}", entry["source"]))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub listeners: usize,
    /// Responses that ranked melody under off.
    pub melody_below_off: Vec<(String, String)>,
    /// Responses that heard warble on a corrected take: who, which source, which take.
    pub warble: Vec<(String, String, String)>,
    /// Warble reported on the uncorrected take: how often this ear hears it in the singer.
    pub warble_on_off: usize,
    pub missing: Vec<(String, String)>,
    pub passes: bool,
}

/// The bar from Aeon Karaoke's `auto-tune.md`: three listeners at least, each of them ranks melody
/// above or equal to off on every source, and nobody reports warble on a corrected take. Warble
/// heard on the uncorrected take fails nothing; it is the control.
pub fn verdict(responses: &[Response], key: &Value) -> Result<Verdict, String> {
    let sources = key["sources"].as_array().ok_or("a key with no sources")?;
    let listeners: BTreeSet<&str> = responses.iter().map(|r| r.listener.as_str()).collect();
    let mut answered = BTreeSet::new();
    for response in responses {
        if !answered.insert((&response.listener, &response.source)) {
            return Err(format!(
                "{} answered {} twice; keep one row per source",
                response.listener, response.source
            ));
        }
    }
    let mut result = Verdict {
        listeners: listeners.len(),
        melody_below_off: Vec::new(),
        warble: Vec::new(),
        warble_on_off: 0,
        missing: Vec::new(),
        passes: false,
    };
    for entry in sources {
        let source = entry["source"].as_str().ok_or("a source with no name")?;
        let labels = labels_of(entry)?;
        let (off, melody, key_take) = (labels["off"], labels["melody"], labels["key"]);
        for listener in &listeners {
            let found = responses.iter().find(|r| r.listener == *listener && r.source == source);
            let Some(response) = found else {
                result.missing.push(((*listener).to_owned(), source.to_owned()));
                continue;
            };
            if response.ranks[melody] > response.ranks[off] {
                result.melody_below_off.push(((*listener).to_owned(), source.to_owned()));
            }
            for (index, take) in [(melody, "melody"), (key_take, "key")] {
                if response.warble[index] {
                    let row = ((*listener).to_owned(), source.to_owned(), take.to_owned());
                    result.warble.push(row);
                }
            }
            result.warble_on_off += usize::from(response.warble[off]);
        }
    }
    result.passes = result.listeners >= 3
        && result.melody_below_off.is_empty()
        && result.warble.is_empty()
        && result.missing.is_empty();
    Ok(result)
}

/// The verdict and, for each source, what each take measured beside what was reported: how an
/// objective measure gets checked against people.
pub fn score_report(responses: &[Response], key: &Value) -> Result<String, String> {
    let result = verdict(responses, key)?;
    let mut report = format!(
        "{}: {} listeners; melody ranked under off in {} responses; warble on a corrected take in \
         {}; warble on the uncorrected take in {}; {} responses missing\n\n",
        if result.passes { "PASS" } else { "FAIL" },
        result.listeners,
        result.melody_below_off.len(),
        result.warble.len(),
        result.warble_on_off,
        result.missing.len(),
    );
    report.push_str(
        "| Source | Take | Warble reports | Robotic reports | Mean rank | Added motion p95 | \
         Motion kept |\n|---|---|---|---|---|---|---|\n",
    );
    for entry in key["sources"].as_array().ok_or("a key with no sources")? {
        let source = entry["source"].as_str().ok_or("a source with no name")?;
        let labels = labels_of(entry)?;
        for (take, index) in labels {
            let answers: Vec<&Response> = responses.iter().filter(|r| r.source == source).collect();
            let warble = answers.iter().filter(|r| r.warble[index]).count();
            let robotic = answers.iter().filter(|r| r.robotic[index]).count();
            let ranks: u32 = answers.iter().map(|r| r.ranks[index]).sum();
            let measured = &entry["measures"][take];
            writeln!(
                report,
                "| {source} | {take} | {warble} | {robotic} | {:.2} | {:.1} c | {:.2} |",
                f64::from(ranks) / answers.len().max(1) as f64,
                measured["motion_added_p95_cents"].as_f64().unwrap_or(f64::NAN),
                measured["motion_kept"].as_f64().unwrap_or(f64::NAN),
            )
            .expect("a string takes writes");
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sung_clip::MeantNote;
    use crate::synthetic::harmonic_voice;

    const NOTE: MeantNote = MeantNote { midi: 57, start_ms: 100.0, end_ms: 1400.0 };

    fn source(name: &str, flat: f64) -> ListenSource {
        ListenSource {
            name: name.to_owned(),
            clip: SungClip {
                name: name.to_owned(),
                samples: harmonic_voice(72_000, |_| 220.0 * 2f64.powf(-flat / 1200.0)),
                notes: vec![NOTE],
                second_notes: None,
                truth: None,
                key: "C".to_owned(),
            },
            backing: None,
            attribution: None,
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let place =
            std::env::temp_dir().join(format!("measure-corrector-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&place).ok();
        place
    }

    // Every take carries each label equally over many shuffles, so a label says nothing
    #[test]
    fn labels_say_nothing_about_the_take() {
        let mut next = shuffled_takes(11);
        let mut counts = [[0u32; 3]; 3];
        for _ in 0..3000 {
            for (label, take) in next().iter().enumerate() {
                counts[label][*take as usize] += 1;
            }
        }
        for row in counts {
            for count in row {
                assert!((900..=1100).contains(&count), "{counts:?}");
            }
        }
    }

    // The set a listener gets holds no answer: the key is elsewhere, the files tell nothing apart
    // by name, size or level, and the same seed gives the same assignment
    #[test]
    fn the_answer_stays_apart_and_the_takes_are_level_matched() {
        let root = scratch("set");
        let (clips, key) = (root.join("clips"), root.join("key"));
        std::fs::create_dir_all(&root).unwrap();
        render_set(&[source("one", 30.0)], None, &clips, &key, 5).unwrap();

        let listed: BTreeSet<String> = std::fs::read_dir(&clips)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            listed,
            BTreeSet::from(
                ["one", "LISTENING.md", "answer-sheet.csv", "ATTRIBUTION.md"].map(str::to_owned)
            )
        );
        let answer: Value =
            serde_json::from_str(&std::fs::read_to_string(key.join("answer-key.json")).unwrap())
                .unwrap();
        assert_eq!(answer["seed"], 5);

        let mut levels = Vec::new();
        let mut sizes = BTreeSet::new();
        for label in LABELS {
            let bytes = std::fs::read(clips.join("one").join(format!("{label}.wav"))).unwrap();
            sizes.insert(bytes.len());
            let samples: Vec<f32> = bytes[44..]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| f32::from(i16::from_le_bytes(*pair)) / 32768.0)
                .collect();
            levels.push(rms(&samples, &samples));
        }
        assert_eq!(sizes.len(), 1, "takes differ in size: {sizes:?}");
        // Within 0.1 dB: a level step a listener could not use
        let (quiet, loud) =
            levels.iter().fold((1.0f32, 0.0f32), |(q, l), v| (q.min(*v), l.max(*v)));
        assert!(20.0 * (loud / quiet).log10() < 0.1, "{levels:?}");

        // Corrected and off really differ, and the off take is the voice
        let measures = &answer["sources"][0]["measures"];
        assert!(measures["off"]["motion_added_p95_cents"].as_f64().unwrap() < 0.5);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn audio_refuses_a_repo_and_a_key_beside_its_clips() {
        let root = scratch("places");
        std::fs::create_dir_all(root.join("repo/.git")).unwrap();
        let elsewhere = root.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert!(check_places(&root.join("repo/clips"), &elsewhere.join("key")).is_err());
        assert!(check_places(&elsewhere.join("clips"), &root.join("repo/key")).is_err());
        assert!(check_places(&elsewhere.join("set"), &elsewhere.join("set/key")).is_err());
        std::fs::write(elsewhere.join("already-here"), "").unwrap();
        assert!(check_places(&elsewhere, &root.join("key")).is_err());
        assert!(check_places(&root.join("clips"), &root.join("key")).is_ok());
        std::fs::remove_dir_all(&root).ok();
    }

    fn key() -> Value {
        json!({ "sources": [{
            "source": "one",
            "labels": { "A": "melody", "B": "off", "C": "key" },
            "measures": {},
        }] })
    }

    fn answered(listener: &str, ranks: [u32; 3], warble: [bool; 3]) -> Response {
        Response {
            listener: listener.to_owned(),
            source: "one".to_owned(),
            ranks,
            warble,
            robotic: [false; 3],
        }
    }

    // The bar, each way it can fail
    #[test]
    fn the_bar_is_all_three_and_no_warble() {
        let quiet = [false; 3];
        let good = |name| answered(name, [1, 1, 3], quiet);
        let pass = verdict(&[good("a"), good("b"), good("c")], &key()).unwrap();
        assert!(pass.passes, "{pass:?}");

        let two = verdict(&[good("a"), good("b")], &key()).unwrap();
        assert!(!two.passes, "two listeners are not three");

        let under = answered("c", [2, 1, 3], quiet);
        let below = verdict(&[good("a"), good("b"), under], &key()).unwrap();
        assert!(!below.passes);
        assert_eq!(below.melody_below_off, [("c".to_owned(), "one".to_owned())]);

        let heard = answered("c", [1, 1, 3], [true, false, false]);
        assert!(!verdict(&[good("a"), good("b"), heard], &key()).unwrap().passes);

        // Heard on the voice itself, it is the control and fails nothing
        let control = answered("c", [1, 1, 3], [false, true, false]);
        let result = verdict(&[good("a"), good("b"), control], &key()).unwrap();
        assert!(result.passes, "{result:?}");
        assert_eq!(result.warble_on_off, 1);
    }

    #[test]
    fn a_revised_answer_is_refused_rather_than_half_counted() {
        let key = json!({ "sources": [] });
        let clean = answered("ana", [1, 2, 3], [false, false, false]);
        let revised = answered("ana", [1, 2, 3], [true, false, false]);
        let error = verdict(&[clean, revised], &key).unwrap_err();
        assert!(error.contains("twice"), "{error}");
    }

    #[test]
    fn a_sheet_reads_back_and_ignores_the_blank_lines() {
        let text = format!(
            "{SHEET_HEADER}\nYOUR-NAME,one,,,,,,,,,,\nAda,one,1,2,3,y,no,,,yes,,fine, really\n"
        );
        let sheet = parse_sheet(&text).unwrap();
        assert_eq!(
            sheet,
            [Response {
                listener: "Ada".to_owned(),
                source: "one".to_owned(),
                ranks: [1, 2, 3],
                warble: [true, false, false],
                robotic: [false, true, false],
            }]
        );
    }
}
