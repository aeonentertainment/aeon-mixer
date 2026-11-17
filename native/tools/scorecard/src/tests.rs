//! #196's done-when, on the committed fixture: a duet written on the offline driver, its silent
//! backing in `fixtures/stems`, scored against `fixtures/baseline.json`.

use std::path::{Path, PathBuf};

use replay::bundle::report::Options as Replay;
use serde_json::Value;

use crate::card::{Entry, Scorecard};
use crate::movement::{Change, Movement};
use crate::{Options, score};

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

fn failing(movements: &[Movement]) -> Vec<&str> {
    movements.iter().filter(|m| m.fails()).map(|m| m.number.as_str()).collect()
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
    assert_eq!(failing(&movements), Vec::<&str>::new(), "{movements:#?}");
}

#[test]
fn a_longer_playout_delay_moves_that_number_and_no_other_beyond_tolerance() {
    let bundle = fixtures().join("bundles/duet");
    let mut profile: Value =
        serde_json::from_str(&std::fs::read_to_string(bundle.join("profile.json")).unwrap())
            .unwrap();
    profile["ingest"]["playoutDelayMs"] = 30.into();
    let path = scratch("delay-30.json");
    std::fs::write(&path, profile.to_string()).unwrap();

    let baseline = Scorecard::read(&fixtures().join("baseline.json")).unwrap();
    let movements = Movement::between(&scored(Some(path), None), &baseline);
    assert_eq!(failing(&movements), ["slot0.playoutDelayMs", "slot1.playoutDelayMs"]);
    for movement in movements.iter().filter(|m| m.fails()) {
        assert!(matches!(movement.change, Change::Moved { now: 30.0, .. }), "{movement:?}");
    }
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
