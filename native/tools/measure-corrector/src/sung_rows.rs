//! The rows read off real singing: the detector against human-checked f0, and each note's pitch
//! against the note meant, scored as the harness scored them, then what correction did to the
//! voice's movement.

use engine::block::RATE;
use engine::slot::guide::Guide;
use serde_json::{Map, Value, json};

use crate::chain::{Input, Setup, Take, run};
use crate::condition::{Condition, Prepared};
use crate::contour::{self, LOWEST_CORRECTED_HZ, Point, VOICED_CONFIDENCE};
use crate::expression::{self, Envelope};
use crate::music::{cents, cents_off_class, median, percentile};
use crate::sung_clip::{MeantNote, SungClip};
use crate::truth::Truth;
use crate::variant::Variant;

/// Past a note's first moments, where any corrector is still arriving, and before its end.
const NOTE_LEAD_MS: f64 = 30.0;
const NOTE_TAIL_MS: f64 = 10.0;
/// Trained listeners hear a note as in tune between these, sharp being the easier error: the range
/// BERT-APC's perceptual tolerance rate takes from the intonation literature.
const IN_TUNE_CENTS: (f64, f64) = (-10.0, 15.0);

/// Start offsets, in frames, that a live slot could have: none of them is a change anyone would
/// hear, and together they say how far a row's median moves on the hop's phase alone.
const START_OFFSETS: [usize; 5] = [0, 37, 74, 111, 200];

/// One run of `clip` through `condition` and `variant`.
pub struct Rendered {
    pub prepared_lag_frames: i64,
    pub take: Take,
}

pub fn render(clip: &SungClip, condition: Condition, guide: Guide, variant: &Variant) -> Rendered {
    let Prepared { input, lag_frames, guide_offset_ms } = condition.prepare(&clip.samples);
    render_prepared(clip, input, lag_frames, guide_offset_ms, guide, variant)
}

fn render_prepared(
    clip: &SungClip,
    input: Input,
    lag_frames: i64,
    guide_offset_ms: f32,
    guide: Guide,
    variant: &Variant,
) -> Rendered {
    let tail = RATE as usize / 2;
    let frames = clip.samples.len() + lag_frames.max(0) as usize + tail;
    let mut setup = Setup::new(input, frames, variant.params());
    setup.guide = Some(guide);
    setup.guide_offset_ms = guide_offset_ms;
    Rendered { prepared_lag_frames: lag_frames, take: run(setup) }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectorScore {
    pub frames: usize,
    /// Reference voiced frames read within 50 cents: raw pitch accuracy.
    pub within_50_cents: f64,
    pub undetected: f64,
    pub octave_errors: f64,
    pub median_cents: f64,
    /// Reference voiced frames the corrector would act on: raw pitch accuracy asks whether a
    /// reading is right; this asks whether the corrector believes it. Voicing recall at its bar.
    pub engaged: f64,
    /// Reference unvoiced frames it would act on anyway: voicing false alarm at its bar.
    pub false_engaged: f64,
}

/// Each reading scored against the f0 at its window's centre, the way the harness scored its
/// element, voiced frames being the reference's.
pub fn score_detector(readings: &[Point], truth: &Truth) -> DetectorScore {
    let annotated =
        readings.iter().filter(|reading| reading.ms >= 0.0 && reading.ms <= truth.end_ms());
    let (mut voiced, mut unvoiced) = (0usize, 0usize);
    let (mut undetected, mut within, mut octave, mut engaged, mut false_engaged) =
        (0usize, 0usize, 0usize, 0usize, 0usize);
    let mut errors = Vec::new();
    for reading in annotated {
        let reference = truth.hz_at(reading.ms);
        let would_act =
            reading.hz >= LOWEST_CORRECTED_HZ && reading.confidence >= VOICED_CONFIDENCE;
        if reference <= 0.0 {
            unvoiced += 1;
            false_engaged += usize::from(would_act);
            continue;
        }
        voiced += 1;
        engaged += usize::from(would_act);
        if reading.hz <= 0.0 {
            undetected += 1;
            continue;
        }
        let error = cents(f64::from(reading.hz), reference).abs();
        errors.push(error);
        within += usize::from(error <= 50.0);
        octave += usize::from(error > 50.0 && (error - 1200.0).abs() <= 50.0);
    }
    let share = |count: usize, of: usize| count as f64 / of.max(1) as f64;
    DetectorScore {
        frames: voiced,
        within_50_cents: share(within, voiced),
        undetected: share(undetected, voiced),
        octave_errors: share(octave, voiced),
        median_cents: median(&errors),
        engaged: share(engaged, voiced),
        false_engaged: share(false_engaged, unvoiced),
    }
}

fn detector_json(score: DetectorScore) -> Value {
    json!({
        "frames": score.frames,
        "within_50_cents_pct": score.within_50_cents * 100.0,
        "undetected_pct": score.undetected * 100.0,
        "octave_errors_pct": score.octave_errors * 100.0,
        "median_cents": score.median_cents,
        "engaged_pct": score.engaged * 100.0,
        "false_engaged_pct": score.false_engaged * 100.0,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IntonationScore {
    pub notes: usize,
    /// The row: each note's median pitch against the note meant, the median of how far.
    pub median_cents: f64,
    /// The same, signed: whether what is left over leans flat or sharp.
    pub signed_median_cents: f64,
    /// How many notes a trained ear would call in tune.
    pub in_tune_pct: f64,
    pub worst_p95_cents: f64,
}

/// Each note's median voiced pitch from just after its start, against the note meant, in whatever
/// octave it was sung. A note with under three voiced readings is not scored.
pub fn score_intonation(points: &[Point], notes: &[MeantNote]) -> IntonationScore {
    let offs: Vec<f64> = notes
        .iter()
        .filter_map(|note| {
            let from = note.start_ms + NOTE_LEAD_MS;
            let to = note.end_ms - NOTE_TAIL_MS;
            let sung: Vec<f64> = points
                .iter()
                .filter(|point| point.voiced() && from <= point.ms && point.ms < to)
                .map(Point::midi)
                .collect();
            (sung.len() >= 3).then(|| cents_off_class(median(&sung), f64::from(note.midi)))
        })
        .collect();
    let magnitudes: Vec<f64> = offs.iter().map(|off| off.abs()).collect();
    let in_tune = offs.iter().filter(|off| (IN_TUNE_CENTS.0..=IN_TUNE_CENTS.1).contains(*off));
    IntonationScore {
        notes: offs.len(),
        median_cents: median(&magnitudes),
        signed_median_cents: median(&offs),
        in_tune_pct: in_tune.count() as f64 / offs.len().max(1) as f64 * 100.0,
        worst_p95_cents: percentile(&magnitudes, 0.95),
    }
}

/// The notes a take's movement is read over: the second annotator's, whose longer notes hold
/// through ornaments, where there is one.
fn sustained_notes(clip: &SungClip) -> &[MeantNote] {
    clip.second_notes.as_deref().unwrap_or(&clip.notes)
}

/// `clip` later by `frames`, its notes with it.
fn started_later(clip: &SungClip, frames: usize) -> SungClip {
    let shift = frames as f64 * 1000.0 / f64::from(RATE);
    let later = |note: &MeantNote| MeantNote {
        start_ms: note.start_ms + shift,
        end_ms: note.end_ms + shift,
        ..*note
    };
    let mut samples = vec![0.0; frames];
    samples.extend_from_slice(&clip.samples);
    SungClip {
        name: clip.name.clone(),
        samples,
        notes: clip.notes.iter().map(later).collect(),
        second_notes: None,
        truth: None,
        key: clip.key.clone(),
    }
}

/// The intonation row's lowest and highest over the start offsets, direct: what a change has to
/// beat before it means anything. A 25-note clip moves by several cents on this alone.
fn start_offset_spread(clip: &SungClip, variant: &Variant) -> Result<(f64, f64), String> {
    let mut medians = Vec::with_capacity(START_OFFSETS.len());
    for frames in START_OFFSETS {
        let later = started_later(clip, frames);
        let rendered = render(&later, Condition::Direct, variant.guide(&later)?, variant);
        let points = contour::read(&rendered.take.voice(), 0);
        medians.push(score_intonation(&points, &later.notes).median_cents);
    }
    let lowest = medians.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = medians.iter().copied().fold(0.0, f64::max);
    Ok((lowest, highest))
}

/// What the movement measures say about one take against the voice that went in.
pub fn expression_of(
    clip: &SungClip,
    rendered: &Rendered,
    dry: &[Point],
) -> expression::Expression {
    let output = rendered.take.voice();
    let wet = contour::read(&output, rendered.prepared_lag_frames);
    expression::compare(
        dry,
        &wet,
        &Envelope::of(&clip.samples, 0),
        &Envelope::of(&output, rendered.prepared_lag_frames),
        sustained_notes(clip),
    )
}

/// Every row for one clip: conditions by variants for the note rows, conditions alone for the
/// detector, since it reads the voice before the shifter.
pub fn clip_rows(
    clip: &SungClip,
    conditions: &[Condition],
    variants: &[Variant],
) -> Result<Value, String> {
    let dry = contour::read(&clip.samples, 0);
    let mut row = Map::new();
    for condition in conditions {
        let mut by_variant = Map::new();
        for variant in variants {
            let rendered = render(clip, *condition, variant.guide(clip)?, variant);
            let output = rendered.take.voice();
            let points = contour::read(&output, rendered.prepared_lag_frames);
            let score = score_intonation(&points, &clip.notes);
            let expression = expression_of(clip, &rendered, &dry);
            let second = clip.second_notes.as_ref().map(|notes| score_intonation(&points, notes));
            let spread = match condition {
                Condition::Direct => Some(start_offset_spread(clip, variant)?),
                _ => None,
            };
            by_variant.insert(
                variant.label.clone(),
                json!({
                    "notes": score.notes,
                    "median_cents": score.median_cents,
                    "start_offset_lowest_cents": spread.map(|(lowest, _)| lowest),
                    "start_offset_highest_cents": spread.map(|(_, highest)| highest),
                    "signed_median_cents": score.signed_median_cents,
                    "in_tune_pct": score.in_tune_pct,
                    "worst_p95_cents": score.worst_p95_cents,
                    "annotator_2_median_cents": second.map(|s| s.median_cents),
                    "sustained_notes": expression.sustained_notes,
                    "motion_kept": expression.motion_kept,
                    "motion_added_p95_cents": expression.motion_added_p95,
                    "vibrato_notes": expression.vibrato_notes,
                    "vibrato_kept": expression.vibrato_kept,
                    "envelope_added_p95_db": expression.envelope_added_p95_db,
                    "jitter_buffer": rendered.take.stats.map(|stats| json!({
                        "lost": stats.lost,
                        "late": stats.late,
                        "concealed": stats.concealed,
                        "resyncs": stats.resyncs,
                    })),
                }),
            );
        }
        let mut entry = Map::new();
        entry.insert("variants".to_owned(), Value::Object(by_variant));
        if let Some(truth) = &clip.truth {
            let off = Variant::off();
            let rendered = render(clip, *condition, off.guide(clip)?, &off);
            entry.insert(
                "detector".to_owned(),
                detector_json(score_detector(&rendered.take.readings, truth)),
            );
        }
        row.insert(condition.label(), Value::Object(entry));
    }
    Ok(Value::Object(row))
}

/// The detector alone over a clip's voice at each window: the row #45 compares against, using the
/// kernel's detector with nothing else around it.
pub fn window_rows(clip: &SungClip, truth: &Truth) -> Value {
    let mut row = Map::new();
    for window in [1024usize, 2048] {
        let points = contour::read_with_window(&clip.samples, window, 0);
        row.insert(window.to_string(), detector_json(score_detector(&points, truth)));
    }
    Value::Object(row)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::hz_of;
    use crate::rtp::Impairment;
    use crate::synthetic::harmonic_voice;

    /// A clip that sings each of `notes` `off` cents from the note.
    fn sung(notes: &[MeantNote], off: f64) -> SungClip {
        let end = notes.iter().map(|note| note.end_ms).fold(0.0, f64::max);
        let frames = ((end + 500.0) * 48.0) as usize;
        let samples = harmonic_voice(frames, |ms| {
            notes
                .iter()
                .find(|note| note.start_ms <= ms && ms < note.end_ms)
                .map_or(0.0, |note| hz_of(f64::from(note.midi)) * 2f64.powf(off / 1200.0))
        });
        SungClip {
            name: "known".to_owned(),
            samples,
            notes: notes.to_vec(),
            second_notes: None,
            truth: None,
            key: "C".to_owned(),
        }
    }

    const NOTES: [MeantNote; 3] = [
        MeantNote { midi: 57, start_ms: 200.0, end_ms: 800.0 },
        MeantNote { midi: 60, start_ms: 800.0, end_ms: 1400.0 },
        MeantNote { midi: 64, start_ms: 1400.0, end_ms: 2000.0 },
    ];

    fn median_through(clip: &SungClip, condition: Condition, label: &str) -> IntonationScore {
        let variant =
            Variant::standard().into_iter().find(|variant| variant.label == label).unwrap();
        let rendered = render(clip, condition, variant.guide(clip).unwrap(), &variant);
        let points = contour::read(&rendered.take.voice(), rendered.prepared_lag_frames);
        score_intonation(&points, &clip.notes)
    }

    // The intonation row on a voice with a known miss. Off reads the miss. Strength scales what is
    // left: 0.6 leaves 40 percent of 30 cents and full strength leaves little. The tolerance is the
    // 5 cents the corrector's own tests hold a settled note to.
    #[test]
    fn the_intonation_row_reads_a_known_miss_and_strength_scales_it() {
        let clip = sung(&NOTES, -30.0);
        let off = median_through(&clip, Condition::Direct, "off");
        assert!((off.median_cents - 30.0).abs() < 3.0, "off {off:?}");
        assert!(off.signed_median_cents < 0.0, "a flat voice reads flat: {off:?}");
        assert_eq!(off.in_tune_pct, 0.0);
        let natural = median_through(&clip, Condition::Direct, "melody-0.6");
        assert!((natural.median_cents - 12.0).abs() < 5.0, "natural {natural:?}");
        let full = median_through(&clip, Condition::Direct, "melody-1.0");
        assert!(full.median_cents < 5.0, "full {full:?}");
        assert_eq!(full.in_tune_pct, 100.0);
    }

    // Through Opus and the real jitter buffer the voice keeps its miss, the row reads it at the
    // right time, and the corrector still pulls it in. Harmonic tones survive 32 kb/s well, so the
    // tolerance is the direct row's plus a little for the codec
    #[test]
    fn the_opus_path_keeps_time_and_the_corrector_still_lands() {
        let clip = sung(&NOTES, -30.0);
        let opus = Condition::Opus { impairment: Impairment::NONE, dtx: true };
        let off = median_through(&clip, opus, "off");
        assert_eq!(off.notes, 3, "the lag lines each note up with its readings");
        assert!((off.median_cents - 30.0).abs() < 5.0, "off {off:?}");
        let full = median_through(&clip, opus, "melody-1.0");
        assert!(full.median_cents < 8.0, "full {full:?}");
    }

    // An impaired LAN loses packets and the stream plays through it
    #[test]
    fn a_lossy_lan_is_counted_and_the_stream_plays_on() {
        let clip = sung(&NOTES, 0.0);
        let wifi = Condition::Opus { impairment: Impairment::WIFI, dtx: true };
        let off = Variant::off();
        let rendered = render(&clip, wifi, off.guide(&clip).unwrap(), &off);
        let stats = rendered.take.stats.expect("a packet source reports");
        assert!(stats.lost > 0, "{stats:?}");
        assert_eq!(stats.resyncs, 0, "nothing reset the stream: {stats:?}");
    }

    // Scoring against a known track: one reading right, one an octave off, one a semitone off, one
    // undetected, and one unvoiced frame the corrector would have acted on
    #[test]
    fn the_detector_row_counts_each_kind_of_miss() {
        let truth = Truth::new(vec![
            (0.00, 220.0),
            (0.01, 220.0),
            (0.02, 220.0),
            (0.03, 220.0),
            (0.04, 0.0),
        ]);
        let point = |ms: f64, hz: f32, confidence: f32| Point { ms, hz, confidence };
        let score = score_detector(
            &[
                point(0.0, 220.0, 0.95),
                point(10.0, 440.0, 0.95),
                point(20.0, 233.1, 0.5),
                point(30.0, 0.0, 0.0),
                point(40.0, 220.0, 0.95),
            ],
            &truth,
        );
        assert_eq!(score.frames, 4);
        assert_eq!(score.within_50_cents, 0.25);
        assert_eq!(score.octave_errors, 0.25);
        assert_eq!(score.undetected, 0.25);
        assert_eq!(score.engaged, 0.5);
        assert_eq!(score.false_engaged, 1.0);
    }
}
