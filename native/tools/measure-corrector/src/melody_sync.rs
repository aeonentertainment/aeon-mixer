//! The melody sync row, two ways. In the audio: a steady tone corrected to a guide that steps, the
//! way the harness read it back. To the sample: the same voice rendered with and without one edge
//! of a guide. Everything is deterministic, so the two takes are bit-identical until the corrector
//! acts on that edge, and the first sample they differ on is when it did.

use engine::block::RATE;
use engine::params::CorrectorParams;
use engine::slot::guide::{Guide, Note};
use kernels::{PitchClassMask, pitch_class_of};
use serde_json::{Value, json};

use crate::chain::{Input, Setup, frames_of_ms, run};
use crate::contour::{self, Point};
use crate::music::{cents, hz_of, key_mask, midi_of};
use crate::synthetic::harmonic_voice;

/// The harness's input and guide: A3, stepping two semitones each way, with no retune time.
const INPUT_HZ: f64 = 220.0;
const STEPS: [(u8, f64, f64); 4] =
    [(59, 500.0, 1000.0), (57, 1000.0, 1500.0), (55, 1500.0, 2000.0), (57, 2000.0, 2500.0)];

fn stepping_guide() -> Guide {
    let notes = STEPS.iter().map(|&(midi, start, end)| Note {
        midi,
        start_frame: frames_of_ms(start),
        end_frame: frames_of_ms(end),
    });
    Guide { notes: notes.collect(), key_mask: key_mask("C").expect("a key") }
}

/// Where a reading's pitch crosses halfway from one target to the next, in milliseconds after the
/// edge, between the two readings either side of it in proportion to the cents.
fn crossing_after(points: &[Point], edge_ms: f64, before: f64, after: f64) -> Option<f64> {
    let (before, after) = (cents(before, INPUT_HZ), cents(after, INPUT_HZ));
    let halfway = 0.5 * (before + after);
    let crossed = |point: &Point| {
        let at = cents(f64::from(point.hz), INPUT_HZ);
        if after > before { at >= halfway } else { at <= halfway }
    };
    let index = points.iter().position(|p| p.hz > 0.0 && p.ms > edge_ms - 40.0 && crossed(p))?;
    let (at, prior) = (points.get(index)?, points.get(index.checked_sub(1)?)?);
    let (at_cents, prior_cents) =
        (cents(f64::from(at.hz), INPUT_HZ), cents(f64::from(prior.hz), INPUT_HZ));
    let fraction = (halfway - prior_cents) / (at_cents - prior_cents);
    Some(prior.ms + fraction * (at.ms - prior.ms) - edge_ms)
}

/// How long after each audible step the output's pitch crosses halfway, in milliseconds.
pub fn crossings() -> Vec<f64> {
    let frames = 3 * RATE as usize;
    let voice = harmonic_voice(frames, |_| INPUT_HZ);
    let hard = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };
    let mut setup = Setup::new(Input::Samples(voice), frames, hard);
    setup.guide = Some(stepping_guide());
    let points = contour::read(&run(setup).voice(), 0);
    let hz_at = |ms: f64| {
        let note = STEPS.iter().find(|(_, start, end)| *start <= ms && ms < *end);
        note.map_or(INPUT_HZ, |(midi, ..)| hz_of(f64::from(*midi)))
    };
    STEPS
        .iter()
        .map(|(_, start, _)| *start)
        .filter(|edge| (hz_at(*edge - 1.0) - hz_at(*edge)).abs() > 1e-9)
        .filter_map(|edge| crossing_after(&points, edge, hz_at(edge - 1.0), hz_at(edge)))
        .collect()
}

/// `guide` as if nothing happened from `edge` on: notes sounding across it sound on, and nothing
/// after it starts. `u64::MAX` is the guide itself.
fn without_edge_on(guide: &Guide, edge: u64) -> Guide {
    let notes = guide.notes.iter().filter(|note| note.start_frame < edge).map(|note| Note {
        end_frame: if note.end_frame >= edge && edge != u64::MAX {
            u64::MAX
        } else {
            note.end_frame
        },
        ..*note
    });
    Guide { notes: notes.collect(), key_mask: guide.key_mask }
}

/// With no retune time, so an edge with no target change leaves the corrector's state exactly as
/// it was: an eased shift splits its easing at the edge, and rounding would show up later.
fn output_following(voice: &[f32], guide: Guide, until: usize) -> Vec<f32> {
    let hard = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };
    let mut setup = Setup::new(Input::Samples(voice[..until].to_vec()), until, hard);
    setup.guide = Some(guide);
    run(setup).voice()
}

/// The nearest of `mask`'s notes to `midi`, as the corrector searches for one.
fn nearest(mask: PitchClassMask, midi: f64) -> Option<i32> {
    let base = midi.round() as i32;
    (-6..=6)
        .map(|step| base + step)
        .filter(|note| mask & pitch_class_of(*note) != 0)
        .min_by(|a, b| (f64::from(*a) - midi).abs().total_cmp(&(f64::from(*b) - midi).abs()))
}

/// What a guide pulls to at `frame`: the note sounding, newest first, else its key.
fn mask_at(guide: &Guide, frame: u64) -> PitchClassMask {
    let sounding = guide
        .notes
        .iter()
        .filter(|note| note.start_frame <= frame && frame < note.end_frame)
        .max_by_key(|note| note.start_frame);
    sounding.map_or(guide.key_mask, |note| pitch_class_of(i32::from(note.midi)))
}

/// Every distinct edge of `guide` within `seconds`, where the voice's nearest target changes: how
/// many frames after it the output first differs from the one that never saw it. An edge with no
/// change is left out. It still splits the corrector's run, and the shifter's jump can land a few
/// samples differently for it, but nothing turned.
pub fn edge_lateness(guide: &Guide, seconds: usize) -> Vec<Option<i64>> {
    let frames = seconds * RATE as usize;
    let voice_midi = midi_of(INPUT_HZ) + 0.3;
    let voice = harmonic_voice(frames, |_| hz_of(voice_midi));
    let full = output_following(&voice, without_edge_on(guide, u64::MAX), frames);
    let mut edges: Vec<u64> = guide
        .notes
        .iter()
        .flat_map(|note| [note.start_frame, note.end_frame])
        .filter(|edge| *edge < frames as u64 && *edge > 0)
        .collect();
    edges.sort_unstable();
    edges.dedup();
    let ends: Vec<usize> =
        edges.iter().skip(1).map(|edge| *edge as usize).chain([frames]).collect();
    edges
        .iter()
        .zip(ends)
        .filter(|(edge, _)| {
            nearest(mask_at(guide, **edge - 1), voice_midi)
                != nearest(mask_at(guide, **edge), voice_midi)
        })
        .map(|(edge, until)| {
            // Only as far as the next edge: a difference after that one is the next edge's
            let without = output_following(&voice, without_edge_on(guide, *edge), until);
            let differs = full.iter().zip(&without).position(|(a, b)| a != b);
            differs.map(|at| at as i64 - *edge as i64)
        })
        .collect()
}

pub fn rows(golden_guide: Option<&Guide>) -> Value {
    let steps = crossings();
    let stepping;
    let (guide, seconds) = match golden_guide {
        Some(guide) => (guide, 13),
        None => {
            stepping = stepping_guide();
            (&stepping, 3)
        }
    };
    let lateness = edge_lateness(guide, seconds);
    let heard: Vec<i64> = lateness.iter().flatten().copied().collect();
    json!({
        "halfway_after_edge_ms": steps,
        "halfway_worst_ms": steps.iter().map(|ms| ms.abs()).fold(0.0, f64::max),
        "target_changes": lateness.len(),
        "target_changes_seen": heard.len(),
        "latest_frames": heard.iter().copied().max(),
        "earliest_frames": heard.iter().copied().min(),
        "latest_ms": heard.iter().copied().max().map(|f| f as f64 * 1000.0 / f64::from(RATE)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The row's claim, on the row's own measure: a corrector that acts on an edge late or early
    // would show here. The tolerance is the sample the read point moves on after the shift changes
    #[test]
    fn the_target_turns_on_the_sample_the_guide_names() {
        let lateness = edge_lateness(&stepping_guide(), 3);
        // Four notes have five edges, but the last hands back to the key, where this voice's
        // nearest note is the one it had
        assert_eq!(lateness.len(), 4);
        let heard: Vec<i64> = lateness.into_iter().flatten().collect();
        assert_eq!(heard.len(), 4, "every target change shows in the output");
        for frames in heard {
            assert!((0..=1).contains(&frames), "the target turned {frames} frames from its edge");
        }
    }

    // The audio side: every step reads back within the 6 ms the baseline reached, which is the
    // shifter's own delay and the 10 ms grid's interpolation
    #[test]
    fn the_output_pitch_crosses_halfway_soon_after_each_edge() {
        let steps = crossings();
        assert_eq!(steps.len(), 4);
        for ms in steps {
            assert!(ms.abs() < 10.0, "halfway {ms:+.1} ms from its edge");
        }
    }
}
