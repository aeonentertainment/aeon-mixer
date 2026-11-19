//! #196's done-when, on the committed fixture: a duet written on the offline driver, its silent
//! backing in `fixtures/stems`, scored against `fixtures/baseline.json`.

use std::path::{Path, PathBuf};

use engine::slot::effects::{Reading, TargetNote};
use replay::bundle::report::Options as Replay;
use serde_json::Value;

use crate::card::{Entry, Scorecard};
use crate::movement::{Change, Movement};
use crate::{Options, numbers, score};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures")
}

fn scored(profile: Option<PathBuf>, baseline: Option<&Scorecard>) -> Scorecard {
    let options = Options {
        bundles: vec![fixtures().join("bundles")],
        replay: Replay { profile, backing: Some(fixtures().join("stems")) },
        baseline: None,
        write: None,
    };
    score(&options, baseline).unwrap()
}

fn scratch(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("aeon-scorecard-{}-{name}", std::process::id()))
}

/// Each number that moved past its tolerance, by bundle and name.
fn failing(movements: &[Movement]) -> Vec<String> {
    movements.iter().filter(|m| m.fails()).map(|m| format!("{} {}", m.bundle, m.number)).collect()
}

#[test]
fn a_bundle_scored_against_its_own_scorecard_reports_no_movement() {
    let first = scored(None, None);
    assert!(first.bundles["duet"].len() > 15, "{first:?}");
    let path = scratch("own.json");
    first.write(&path).unwrap();
    let baseline = Scorecard::read(&path).unwrap();
    let again = scored(None, Some(&baseline));
    assert_eq!(again, first, "the same bundle scores to the same numbers");
    assert_eq!(Movement::between(&again, &baseline), []);
}

#[test]
fn the_fixture_scores_within_the_committed_baseline() {
    let baseline = Scorecard::read(&fixtures().join("baseline.json")).unwrap();
    let movements = Movement::between(&scored(None, Some(&baseline)), &baseline);
    assert_eq!(failing(&movements), Vec::<String>::new(), "{movements:#?}");
}

/// The fixture's own profile with `change` made to it, written where `scored` can read it.
fn profile(name: &str, change: impl FnOnce(&mut Value)) -> PathBuf {
    let bundle = fixtures().join("bundles/duet");
    let mut profile: Value =
        serde_json::from_str(&std::fs::read_to_string(bundle.join("profile.json")).unwrap())
            .unwrap();
    change(&mut profile);
    let path = scratch(name);
    std::fs::write(&path, profile.to_string()).unwrap();
    path
}

#[test]
fn a_longer_playout_delay_moves_that_number_and_no_other_beyond_tolerance() {
    let path = profile("delay-30.json", |p| p["ingest"]["playoutDelayMs"] = 30.into());
    let baseline = Scorecard::read(&fixtures().join("baseline.json")).unwrap();
    let movements = Movement::between(&scored(Some(path), None), &baseline);
    assert_eq!(
        failing(&movements),
        ["duet slot0.playoutDelayMs", "duet slot1.playoutDelayMs", "echo slot0.playoutDelayMs"]
    );
    // Each stream's earliest packets wait what they did, 15 ms longer
    for movement in movements.iter().filter(|m| m.fails()) {
        let Change::Moved { baseline, now, .. } = movement.change else { panic!("{movement:?}") };
        assert!((now - baseline - 15.0).abs() < 0.011, "{movement:?}");
    }
}

// The fixture's voice sits 48 cents from its guide, and half the strength leaves more of that
// in the corrected voice; the singer, the delays and the levels stay where they were
#[test]
fn correction_at_another_strength_moves_the_corrected_pitch_and_no_other_number() {
    let path = profile("strength-0.3.json", |p| {
        p["slot"]["effects"]["pitch-correction"]["strength"] = 0.3.into();
    });
    let baseline = Scorecard::read(&fixtures().join("baseline.json")).unwrap();
    let movements = Movement::between(&scored(Some(path), None), &baseline);
    assert_eq!(failing(&movements), ["duet slot0.correctedErrorCents"], "{movements:#?}");
    let corrected = movements.iter().find(|m| m.fails()).unwrap();
    let Change::Moved { baseline, now, .. } = corrected.change else { panic!("{corrected:?}") };
    assert!(now > baseline + 10.0, "{baseline} to {now} cents");
}

#[test]
fn a_move_of_its_tolerance_passes_and_a_number_gone_fails() {
    let card = |entries: &[(&str, f64, f64)]| Scorecard {
        format: crate::card::FORMAT,
        bundles: [(
            "song".to_owned(),
            entries
                .iter()
                .map(|&(name, value, tolerance)| (name.to_owned(), Entry { value, tolerance }))
                .collect(),
        )]
        .into(),
    };
    let baseline = card(&[("held", 1.0, 0.3), ("past", 1.0, 0.3), ("gone", 1.0, 5.0)]);
    let now = card(&[("held", 1.3, 0.0), ("past", 1.31, 0.0), ("new", 2.0, 0.0)]);
    let movements = Movement::between(&now, &baseline);
    let verdicts: Vec<(&str, bool)> =
        movements.iter().map(|m| (m.number.as_str(), m.fails())).collect();
    assert_eq!(verdicts, [("gone", true), ("held", false), ("new", false), ("past", true)]);
}

#[test]
fn a_scorecard_is_refused_by_its_format_and_by_a_field_it_does_not_know() {
    let refused = |text: &str| {
        let path = scratch("refused.json");
        std::fs::write(&path, text).unwrap();
        Scorecard::read(&path).err().unwrap()
    };
    assert!(refused(r#"{"format":2,"bundles":{}}"#).contains("scorecard format 2"));
    let misspelt = r#"{"format":1,"bundles":{"song":{"late":{"value":0,"tolerence":1}}}}"#;
    assert!(refused(misspelt).contains("unknown field `tolerence`"));
}

// A gap in the pull inside one guide note is a let-go; the voice stopping as a note ends, or a
// pull that starts late, is not, even when the next note touches it on the same pitch
#[test]
fn a_let_go_is_a_gap_in_the_pull_while_one_note_sounds() {
    // A note by its pitch and the frame it starts on
    let reading = |note: Option<(i16, u64)>, pulling: bool| Reading {
        position_ms: 0.0,
        hz: 185.0,
        confidence: 1.0,
        note: note.map(|(midi, start_frame)| TargetNote { midi, start_frame }),
        error_semitones: 0.0,
        shift_semitones: 0.0,
        pulling,
    };
    let (first, rest) = (Some((54, 0)), None);
    // The next two notes touch, on one pitch, with a breath between them
    let (second, third) = (Some((56, 24_000)), Some((56, 38_400)));
    let song = [
        reading(first, false),
        reading(first, true),
        reading(first, false),
        reading(first, false),
        reading(first, true),
        reading(first, false),
        reading(rest, true),
        reading(second, true),
        reading(second, false),
        reading(second, true),
        reading(second, false),
        reading(third, false),
        reading(third, true),
    ];
    assert_eq!(numbers::let_goes(&song), Some(2));
    assert_eq!(numbers::let_goes(&[reading(rest, true)]), None);
}
