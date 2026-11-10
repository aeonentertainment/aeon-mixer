//! What correction added that needs no answer key to see, for any take, a phone's included: clicks,
//! octave jumps, a fast warble the voice did not have, and held notes gone still. Each is read on
//! the voice in and the voice out, and the row is the difference, so a singer's own glottal pulses,
//! octave flips and steadiness are not charged to the corrector.

use serde_json::{Value, json};

use crate::clicks;
use crate::contour::{self, Point};
use crate::expression::{band_rms, note_contour};
use crate::music::percentile;
use crate::sung_clip::MeantNote;
use crate::take_delay::TakeDelay;

/// Pitch movement between 8 and 30 Hz: faster than any vibrato, slow enough to be heard as the
/// pitch moving rather than as roughness. Restoration calls the same thing flutter.
pub const FAST_WARBLE_BAND: (f64, f64) = (8.0, 30.0);
/// The detector's 43 ms window would average most of that band away; 1024 samples halves it.
const FAST_WINDOW: usize = 1024;
/// A jump between two readings 10 ms apart this close to an octave is the detector or the shifter
/// slipping, not a singer.
const OCTAVE_CENTS: (f64, f64) = (900.0, 1500.0);
/// A held note that moves less than this, its slope off, sounds machined. A YIN reading of a steady
/// tone wanders by under a cent; vocadito's singers hold a note to 4 to 10 cents RMS.
const STILL_CENTS: f64 = 3.0;
const HELD_NOTE_MS: f64 = 500.0;

/// One take's artefacts against the voice that went in.
pub fn compare(
    dry: &[f32],
    wet: &[f32],
    wet_lag_frames: i64,
    delay: &TakeDelay,
    notes: &[MeantNote],
) -> Value {
    let read = |signal: &[f32], lag: i64, window: usize, delay: &TakeDelay| {
        delay.onto_input(&contour::read_with_window(signal, window, lag))
    };
    let (dry_points, wet_points) = (
        read(dry, 0, contour::WINDOW, &TakeDelay::NONE),
        read(wet, wet_lag_frames, contour::WINDOW, delay),
    );
    let (dry_fast, wet_fast) = (
        read(dry, 0, FAST_WINDOW, &TakeDelay::NONE),
        read(wet, wet_lag_frames, FAST_WINDOW, delay),
    );
    let held: Vec<&MeantNote> =
        notes.iter().filter(|note| note.end_ms - note.start_ms >= HELD_NOTE_MS).collect();

    let mut warble_added = Vec::new();
    for note in &held {
        let (Some(dry), Some(wet)) = (note_contour(&dry_fast, note), note_contour(&wet_fast, note))
        else {
            continue;
        };
        let (low, high) = FAST_WARBLE_BAND;
        warble_added
            .push((band_rms(&wet, 100.0, low, high) - band_rms(&dry, 100.0, low, high)).max(0.0));
    }
    let minutes = dry.len() as f64 / f64::from(engine::block::RATE) / 60.0;
    json!({
        "clicks_added_per_min": clicks::added(dry, wet, |at| {
            let clip_time = at.saturating_sub(usize::try_from(wet_lag_frames).unwrap_or(0));
            clip_time.saturating_sub(delay.frames_at(clip_time))
        }) as f64 / minutes.max(1e-9),
        "octave_jumps_added_per_min":
            (octave_jumps(&wet_points) as f64 - octave_jumps(&dry_points) as f64) / minutes.max(1e-9),
        "fast_warble_added_p95_cents": percentile(&warble_added, 0.95),
        "still_notes_pct": still_share(&wet_points, &held),
        "still_notes_pct_in": still_share(&dry_points, &held),
    })
}

/// Jumps between voiced readings at most 50 ms apart: a detector loses the voice for a few readings
/// while the octave flips.
fn octave_jumps(points: &[Point]) -> usize {
    let voiced: Vec<&Point> = points.iter().filter(|point| point.voiced()).collect();
    voiced
        .windows(2)
        .filter(|pair| {
            let jump = (1200.0 * (f64::from(pair[1].hz) / f64::from(pair[0].hz)).log2()).abs();
            pair[1].ms - pair[0].ms <= 50.0 && (OCTAVE_CENTS.0..=OCTAVE_CENTS.1).contains(&jump)
        })
        .count()
}

/// Of the held notes with a contour, the percent that barely move.
fn still_share(points: &[Point], held: &[&MeantNote]) -> Option<f64> {
    let spreads: Vec<f64> = held
        .iter()
        .filter_map(|note| note_contour(points, note))
        .map(|contour| band_rms(&contour, 100.0, 0.5, 50.0))
        .collect();
    (!spreads.is_empty()).then(|| {
        spreads.iter().filter(|spread| **spread < STILL_CENTS).count() as f64 / spreads.len() as f64
            * 100.0
    })
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;
    use crate::synthetic::harmonic_voice;

    const NOTES: [MeantNote; 2] = [
        MeantNote { midi: 57, start_ms: 100.0, end_ms: 1400.0 },
        MeantNote { midi: 57, start_ms: 1500.0, end_ms: 2800.0 },
    ];

    fn voice(cents_at: impl Fn(f64) -> f64) -> Vec<f32> {
        harmonic_voice(48_000 * 3, |ms| {
            if NOTES.iter().any(|n| n.start_ms <= ms && ms < n.end_ms) {
                220.0 * 2f64.powf(cents_at(ms) / 1200.0)
            } else {
                0.0
            }
        })
    }

    fn sung(ms: f64) -> f64 {
        25.0 * (TAU * 5.5 * ms / 1000.0).sin() + 6.0 * (TAU * 1.1 * ms / 1000.0).sin()
    }

    // The voice against itself adds nothing; a 15 Hz warble of 20 cents injected reads back within
    // the 1024 window's loss at 15 Hz, which takes it to 0.7 to 1.0 of its RMS; a voice pinned
    // still reads as still
    #[test]
    fn an_injected_warble_reads_and_the_voice_itself_does_not() {
        let dry = voice(sung);
        let same = compare(&dry, &dry, 0, &TakeDelay::NONE, &NOTES);
        assert_eq!(same["fast_warble_added_p95_cents"], 0.0);
        assert_eq!(same["octave_jumps_added_per_min"], 0.0);
        assert_eq!(same["clicks_added_per_min"], 0.0);
        assert_eq!(same["still_notes_pct_in"], 0.0);

        let warbled = voice(|ms| sung(ms) + 20.0 * (TAU * 15.0 * ms / 1000.0).sin());
        let read = compare(&dry, &warbled, 0, &TakeDelay::NONE, &NOTES);
        let added = read["fast_warble_added_p95_cents"].as_f64().unwrap();
        let injected = 20.0 / 2f64.sqrt();
        assert!((0.7 * injected..=1.0 * injected).contains(&added), "read {added} of {injected}");

        let pinned = voice(|_| 0.0);
        assert_eq!(compare(&dry, &pinned, 0, &TakeDelay::NONE, &NOTES)["still_notes_pct"], 100.0);
    }

    // A take that flips an octave up for 100 ms and back reads two jumps
    #[test]
    fn an_octave_slip_is_two_jumps() {
        let dry = voice(sung);
        let slipped =
            voice(|ms| sung(ms) + if (600.0..700.0).contains(&ms) { 1200.0 } else { 0.0 });
        let read = compare(&dry, &slipped, 0, &TakeDelay::NONE, &NOTES);
        let per_min = read["octave_jumps_added_per_min"].as_f64().unwrap();
        assert!((per_min - 2.0 / 3.0 * 60.0).abs() < 0.01, "{per_min}");
    }
}
