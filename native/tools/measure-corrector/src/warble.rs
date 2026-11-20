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
use crate::expression::{SLIP_CENTS, WARBLE_BAND, band_rms, bridged};
use crate::music::{median, percentile};
use crate::take_delay::TakeDelay;

/// Short enough to keep 90 percent of a 12 Hz movement, long enough for an 82 Hz voice.
const PITCH_WINDOW: usize = 1024;
/// Each stretch is read for three cycles of the band's lowest rate, a second, so every stretch
/// resolves the band it is labelled with, as `expression` needs for a note.
const READ_MS: f64 = 3000.0 / WARBLE_BAND.0;
/// A run of the singer's sound carries through an unsure stretch this short, which is the
/// detector's doubt, not a rest.
const GAP_MS: f64 = 50.0;
/// A p95 needs this many stretches; with fewer, the most a stretch read is given instead.
const FOR_A_P95: usize = 20;
const LEAD_MS: f64 = 60.0;
const TAIL_MS: f64 = 30.0;
/// A shift this far from the stretch's usual one is the detector slipping on the output, not
/// something the corrector did.
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

    let spread = |values: &[f64]| {
        let p95 = (values.len() >= FOR_A_P95).then(|| percentile(values, 0.95));
        let most = values.iter().copied().fold(f64::NAN, f64::max);
        json!({ "median": median(values), "p95": p95, "most": most })
    };
    json!({
        "stretches": moved.len(),
        "pitch_moved_cents": spread(&moved),
        "pitch_movement_kept": spread(&kept),
    })
}

/// Runs of voiced readings that carry through a gap of `GAP_MS` or less, less their attack and
/// release, that leave `READ_MS` to read. A gap's readings are bridged in the run.
fn stretches(points: &[Point]) -> Vec<std::ops::Range<usize>> {
    let hop_ms = 1000.0 * contour::HOP as f64 / f64::from(RATE);
    let (lead, tail) = ((LEAD_MS / hop_ms) as usize, (TAIL_MS / hop_ms) as usize);
    let gap = (GAP_MS / hop_ms) as usize;
    let mut runs = Vec::new();
    let mut run: Option<(usize, usize)> = None;
    let mut close = |(from, to): (usize, usize)| {
        if (to - from).saturating_sub(lead + tail) as f64 * hop_ms >= READ_MS {
            runs.push(from + lead..to - tail);
        }
    };
    for (index, point) in points.iter().enumerate() {
        if !point.voiced() {
            continue;
        }
        run = match run {
            Some((from, to)) if index - to <= gap => Some((from, index + 1)),
            Some(done) => {
                close(done);
                Some((index, index + 1))
            }
            None => Some((index, index + 1)),
        };
    }
    run.into_iter().for_each(&mut close);
    runs
}

/// The sung pitch over `sung`, and the corrected pitch less it, both in cents, every reading,
/// each with the detector's slips taken out and bridged; `None` when under four fifths of either
/// can be read.
fn shift_over(sung: &[Point], heard: &[Point]) -> Option<(Vec<f64>, Vec<f64>)> {
    let raw: Vec<Option<f64>> =
        sung.iter().map(|point| point.voiced().then(|| 100.0 * point.midi())).collect();
    let shift: Vec<Option<f64>> = sung
        .iter()
        .map(|point| Some(100.0 * (at(heard, point.ms).filter(|_| point.voiced())? - point.midi())))
        .collect();
    Some((bridged(&raw, SLIP_CENTS)?, bridged(&shift, MOST_SHIFT_CENTS)?))
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

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use super::*;
    use kernels::fixtures::BUZZ;

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

    // A sung voice whose fundamental and odd harmonics fade for 60 ms, which the detector reads
    // an octave up and sure of it, in the input and so in the output: the slip is bridged, not
    // read as movement, and half the vibrato, 20 ms late, still keeps half
    #[test]
    fn a_slip_of_the_detector_on_the_sung_voice_is_bridged() {
        let slipped = |depth: f64, late_ms: f64| {
            let hz = |i: usize| {
                let ms = i as f64 * 1000.0 / f64::from(RATE);
                A3 * 2f64
                    .powf((-20.0 + depth * (TAU * 5.0 * (ms - late_ms) / 1000.0).sin()) / 1200.0)
            };
            let whole = kernels::fixtures::harmonic_voice(FRAMES, &BUZZ, hz);
            let even: Vec<f64> =
                BUZZ.iter().enumerate().map(|(h, l)| if h % 2 == 1 { *l } else { 0.0 }).collect();
            let octave = kernels::fixtures::harmonic_voice(FRAMES, &even, hz);
            // Odd harmonics fade out over 40 ms, stay out 60 ms and come back over 40 ms
            let odd = |i: usize| {
                let ms = i as f64 * 1000.0 / f64::from(RATE);
                ((ms - 1460.0).min(1600.0 - ms) / 40.0).clamp(0.0, 1.0) as f32
            };
            whole
                .iter()
                .zip(&octave)
                .enumerate()
                .map(|(i, (w, o))| w + odd(i) * (o - w))
                .collect::<Vec<f32>>()
        };
        let (dry, wet) = (slipped(30.0, 0.0), slipped(15.0, 20.0));
        let sung = contour::read_with_window(&dry, PITCH_WINDOW, 0);
        let octave_up = sung
            .iter()
            .filter(|p| p.voiced() && (1460.0..1600.0).contains(&p.ms) && p.hz > 400.0)
            .count();
        assert!(octave_up >= 3, "the detector slipped on {octave_up} readings");
        let row = measure(&dry, &wet, &[]);
        assert_eq!(row["stretches"], 1, "{row}");
        assert!((median_of(&row, "pitch_movement_kept") - 0.5).abs() < 0.05, "{row}");
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
