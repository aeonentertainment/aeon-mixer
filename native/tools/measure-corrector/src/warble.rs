//! What correction does to a voice's movement in the band a wobble is heard in, read over each
//! stretch the singer held a voiced sound for 300 ms or more, the corrected voice set back onto the
//! input's clock by the delay the slot reported.
//!
//! - **Pitch moved**: the output's pitch less the input's, in 3 to 12 Hz. Correction that follows
//!   the singer moves it by design, so this is the size of what correction does there.
//! - **Pitch movement kept**: the output's own movement in that band against the singer's, as a
//!   ratio. Under 1, correction took some of the singer's movement out; over 1, it put in movement
//!   the singer did not sing.
//!
//! They are measured against known voices, not against listeners. README.md beside this crate
//! says what they cannot tell.

use engine::block::RATE;
use serde_json::{Value, json};

use crate::contour::{self, Point};
use crate::expression::{WARBLE_BAND, band_rms};
use crate::music::{median, percentile};
use crate::take_delay::TakeDelay;

/// Short enough to keep 90 percent of a 12 Hz movement, long enough for an 82 Hz voice.
const PITCH_WINDOW: usize = 1024;
const SHORTEST_MS: f64 = 300.0;
const LEAD_MS: f64 = 60.0;
const TAIL_MS: f64 = 30.0;
/// A reading this far from the stretch's usual shift is the detector's octave or a reading
/// across a splice, not something the corrector did; it is bridged.
const MOST_SHIFT_CENTS: f64 = 100.0;
/// Less movement in the band than this, and the singer held the note still.
const STILLEST_CENTS: f64 = 1.0;

/// Every warble row for a take: `dry` sung, `wet` what the slot made of it, `delays` per block.
pub fn measure(dry: &[f32], wet: &[f32], delays: &[u32]) -> Value {
    let delay = TakeDelay::new(delays);
    let sung = contour::read_with_window(dry, PITCH_WINDOW, 0);
    let mut heard = delay.onto_input(&contour::read_with_window(wet, PITCH_WINDOW, 0));
    heard.sort_by(|a, b| a.ms.total_cmp(&b.ms));
    let stretches = stretches(&sung);

    let (mut moved, mut kept) = (Vec::new(), Vec::new());
    for stretch in &stretches {
        let Some((dry_cents, shift)) = shift_over(&sung[stretch.clone()], &heard) else {
            continue;
        };
        moved.push(band_rms(&shift, 100.0, WARBLE_BAND.0, WARBLE_BAND.1));
        let out: Vec<f64> = dry_cents.iter().zip(&shift).map(|(d, s)| d + s).collect();
        let sung = band_rms(&dry_cents, 100.0, WARBLE_BAND.0, WARBLE_BAND.1);
        // A voice held still has no movement of its own to keep
        if sung >= STILLEST_CENTS {
            kept.push(band_rms(&out, 100.0, WARBLE_BAND.0, WARBLE_BAND.1) / sung);
        }
    }

    let spread =
        |values: &[f64]| json!({ "median": median(values), "p95": percentile(values, 0.95) });
    json!({
        "stretches": moved.len(),
        "pitch_moved_cents": spread(&moved),
        "pitch_movement_kept": spread(&kept),
    })
}

/// Runs of voiced readings at least `SHORTEST_MS` long, less their attack and release.
fn stretches(points: &[Point]) -> Vec<std::ops::Range<usize>> {
    let hop_ms = 1000.0 * contour::HOP as f64 / f64::from(RATE);
    let (lead, tail) = ((LEAD_MS / hop_ms) as usize, (TAIL_MS / hop_ms) as usize);
    let mut runs = Vec::new();
    let mut start = None;
    for (index, point) in
        points.iter().chain([&Point { ms: 0.0, hz: 0.0, confidence: 0.0 }]).enumerate()
    {
        match (point.voiced(), start) {
            (true, None) => start = Some(index),
            (false, Some(from)) => {
                if (index - from) as f64 * hop_ms >= SHORTEST_MS {
                    runs.push(from + lead..index - tail);
                }
                start = None;
            }
            _ => {}
        }
    }
    runs
}

/// The sung pitch over `sung`, and the corrected pitch less it, both in cents, every reading;
/// `None` when under four fifths of it can be read in the output.
fn shift_over(sung: &[Point], heard: &[Point]) -> Option<(Vec<f64>, Vec<f64>)> {
    let dry: Vec<f64> = sung.iter().map(|point| 100.0 * point.midi()).collect();
    let raw: Vec<Option<f64>> = sung
        .iter()
        .zip(&dry)
        .map(|(point, cents)| Some(100.0 * at(heard, point.ms)? - cents))
        .collect();
    let known: Vec<f64> = raw.iter().flatten().copied().collect();
    let usual = median(&known);
    let shift: Vec<Option<f64>> =
        raw.iter().map(|value| value.filter(|s| (s - usual).abs() < MOST_SHIFT_CENTS)).collect();
    let read = shift.iter().flatten().count();
    if (read as f64) < 0.8 * shift.len() as f64 {
        return None;
    }
    Some((dry, bridged(&shift)))
}

/// The output's pitch at `ms` on the input's clock, from the two readings around it, as MIDI.
fn at(heard: &[Point], ms: f64) -> Option<f64> {
    let after = heard.partition_point(|point| point.ms < ms);
    let (before, next) = (heard.get(after.checked_sub(1)?)?, heard.get(after)?);
    if !before.voiced() || !next.voiced() || next.ms - before.ms > 15.0 {
        return None;
    }
    let t = (ms - before.ms) / (next.ms - before.ms).max(1e-9);
    Some(before.midi() + t * (next.midi() - before.midi()))
}

/// Holes filled with a line between the readings either side; the ends hold the nearest.
fn bridged(series: &[Option<f64>]) -> Vec<f64> {
    let known: Vec<(usize, f64)> =
        series.iter().enumerate().filter_map(|(i, v)| v.map(|v| (i, v))).collect();
    (0..series.len())
        .map(|i| {
            let after = known.partition_point(|(k, _)| *k < i);
            match (after.checked_sub(1).map(|b| known[b]), known.get(after)) {
                (_, Some(&(k, v))) if k == i => v,
                (Some((a, va)), Some(&(b, vb))) => va + (vb - va) * (i - a) as f64 / (b - a) as f64,
                (Some((_, v)), None) | (None, Some(&(_, v))) => v,
                (None, None) => 0.0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;
    use crate::synthetic::harmonic_voice;

    const A3: f64 = 220.0;
    const FRAMES: usize = 3 * RATE as usize;

    /// A3, `flat` cents under, with `depth` cents of vibrato at 5 Hz starting `late_ms` later,
    /// and `wobble` cents of a 9 Hz wobble nobody sang.
    fn voice(depth: f64, late_ms: f64, wobble: f64) -> Vec<f32> {
        harmonic_voice(FRAMES, |ms| {
            let vibrato = depth * (TAU * 5.0 * (ms - late_ms) / 1000.0).sin();
            A3 * 2f64.powf((-20.0 + vibrato + wobble * (TAU * 9.0 * ms / 1000.0).sin()) / 1200.0)
        })
    }

    fn median_of(row: &Value, name: &str) -> f64 {
        row[name]["median"].as_f64().unwrap()
    }

    // The voice through unchanged moves nothing and keeps all its movement
    #[test]
    fn a_voice_left_alone_has_no_warble() {
        let dry = voice(30.0, 0.0, 0.0);
        let row = measure(&dry, &dry, &[]);
        assert_eq!(row["stretches"], 1);
        assert!(median_of(&row, "pitch_moved_cents") < 0.05, "{row}");
        assert!((median_of(&row, "pitch_movement_kept") - 1.0).abs() < 0.01, "{row}");
    }

    // Half the vibrato, 20 ms late, moves the voice by the difference of the two vibratos and
    // keeps half the singer's movement
    #[test]
    fn half_the_vibrato_late_keeps_half() {
        let row = measure(&voice(30.0, 0.0, 0.0), &voice(15.0, 20.0, 0.0), &[]);
        let late = TAU * 5.0 * 0.020;
        let difference = (15.0 * late.cos() - 30.0).hypot(15.0 * late.sin());
        let moved = median_of(&row, "pitch_moved_cents");
        assert!((moved - difference / 2f64.sqrt()).abs() < 1.0, "{row}");
        assert!((median_of(&row, "pitch_movement_kept") - 0.5).abs() < 0.03, "{row}");
    }

    // A 9 Hz wobble of 20 cents the singer did not sing, over their 10 cents of vibrato, moves the
    // voice by its own RMS and more than doubles the movement in the band
    #[test]
    fn a_wobble_the_singer_did_not_sing_is_kept_over_one() {
        let row = measure(&voice(10.0, 0.0, 0.0), &voice(10.0, 0.0, 20.0), &[]);
        let moved = median_of(&row, "pitch_moved_cents");
        assert!((moved - 20.0 / 2f64.sqrt()).abs() < 0.15 * 20.0 / 2f64.sqrt(), "{row}");
        let kept = median_of(&row, "pitch_movement_kept");
        assert!((kept - 5f64.sqrt()).abs() < 0.15 * 5f64.sqrt(), "{row}");
    }
}
