//! Known voices for the rows that need an answer they can check, and the corrector's response to
//! them: vibrato it should keep, a scoop it should land, a voice on the fence between two notes.

use std::f64::consts::TAU;

use engine::block::RATE;
use engine::params::CorrectorParams;
use engine::slot::guide::{Guide, Note};
use kernels::EVERY_PITCH_CLASS;
use serde_json::{Value, json};

use crate::chain::{Input, Setup, frames_of_ms, run};
use crate::contour;
use crate::expression::band_rms;
use crate::music::{cents, median};

const A3: f64 = 220.0;

/// Twelve harmonics falling as 1/h, the buzz of a sung vowel, following `hz_at(ms)`. Silent where
/// that says 0.
pub fn harmonic_voice(frames: usize, hz_at: impl Fn(f64) -> f64) -> Vec<f32> {
    let mut phase = 0.0f64;
    (0..frames)
        .map(|i| {
            let hz = hz_at(i as f64 * 1000.0 / f64::from(RATE));
            phase = (phase + hz / f64::from(RATE)) % 1.0;
            if hz <= 0.0 {
                return 0.0;
            }
            let sample: f64 = (1..=12).map(|h| (TAU * h as f64 * phase).sin() / h as f64).sum();
            (0.25 * sample) as f32
        })
        .collect()
}

/// A corrector setting worth reading on a known voice.
fn settings() -> [(&'static str, f32, f32); 3] {
    [("natural-0.6-40ms", 0.6, 40.0), ("full-1.0-40ms", 1.0, 40.0), ("hard-1.0-0ms", 1.0, 0.0)]
}

fn pulled_to_a3(frames: usize) -> Guide {
    let note = Note { midi: 57, start_frame: 0, end_frame: frames as u64 };
    Guide { notes: vec![note], key_mask: EVERY_PITCH_CLASS }
}

fn through(voice: &[f32], strength: f32, retune_ms: f32, guide: Guide) -> Vec<f32> {
    let frames = voice.len();
    let params = CorrectorParams { enabled: true, strength, retune_ms };
    let mut setup = Setup::new(Input::Samples(voice.to_vec()), frames, params);
    setup.guide = Some(guide);
    run(setup).voice()
}

/// Cents from A3 at every reading between `from_ms` and `to_ms`, 100 per second.
fn cents_from_a3(signal: &[f32], from_ms: f64, to_ms: f64) -> Vec<f64> {
    contour::read(signal, 0)
        .iter()
        .filter(|point| point.voiced() && from_ms <= point.ms && point.ms < to_ms)
        .map(|point| cents(f64::from(point.hz), A3))
        .collect()
}

/// #46's gate as a number: a voice `flat` cents under A3 with vibrato of `depth` cents at `rate`.
/// The depth kept is how much of the vibrato's motion comes out, by the band around its rate.
pub fn vibrato(rate: f64, depth: f64, flat: f64, strength: f32, retune_ms: f32) -> (f64, f64) {
    let frames = RATE as usize * 4;
    let voice = harmonic_voice(frames, |ms| {
        A3 * 2f64.powf((-flat + depth * (TAU * rate * ms / 1000.0).sin()) / 1200.0)
    });
    let output = through(&voice, strength, retune_ms, pulled_to_a3(frames));
    let dry = cents_from_a3(&voice, 1000.0, 3900.0);
    let wet = cents_from_a3(&output, 1000.0, 3900.0);
    let band = |series: &[f64]| band_rms(series, 100.0, rate - 1.5, rate + 1.5);
    (wet.iter().sum::<f64>() / wet.len() as f64, band(&wet) / band(&dry))
}

/// What a scooped voice does before its scoop starts, half a second in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approach {
    /// Holds steady below the note.
    Held,
    /// Holds below the note with 50 cents of vibrato at 5.5 Hz, which fades out over the scoop: a
    /// glide out of a note the corrector is holding.
    HeldWithVibrato,
    /// Is silent: the scoop is how the note starts.
    FromSilence,
}

/// #46's other gate: a voice that scoops `from` cents below A3 over `glide_ms` and holds. How long
/// after the scoop ends the output stays within 10 cents of A3, in milliseconds; `None` if it never
/// does in the second that follows.
pub fn scoop_settle_ms(
    approach: Approach,
    from: f64,
    glide_ms: f64,
    strength: f32,
    retune_ms: f32,
) -> Option<f64> {
    let start = 500.0;
    let frames = RATE as usize * 2;
    let voice = harmonic_voice(frames, |ms| {
        let done = ((ms - start) / glide_ms).clamp(0.0, 1.0);
        let vibrato = match approach {
            Approach::HeldWithVibrato => 50.0 * (1.0 - done) * (TAU * 5.5 * ms / 1000.0).sin(),
            Approach::Held => 0.0,
            Approach::FromSilence if ms < start => return 0.0,
            Approach::FromSilence => 0.0,
        };
        A3 * 2f64.powf((from * (done - 1.0) + vibrato) / 1200.0)
    });
    let output = through(&voice, strength, retune_ms, pulled_to_a3(frames));
    let landed = start + glide_ms;
    let points = contour::read(&output, 0);
    let hold = 10;
    points
        .windows(hold)
        .find(|run| {
            run[0].ms >= start
                && run.iter().all(|p| p.voiced() && cents(f64::from(p.hz), A3).abs() <= 10.0)
        })
        .map(|run| run[0].ms - landed)
}

/// A voice sitting on the fence between A3 and B flat, with vibrato that crosses the midpoint: the
/// classic warble, a target that flips. The returned motion is how far the output moves in the
/// warble band against how far the voice did, in cents RMS.
pub fn on_the_fence(depth: f64, strength: f32, retune_ms: f32) -> (f64, f64) {
    let frames = RATE as usize * 4;
    let voice = harmonic_voice(frames, |ms| {
        A3 * 2f64.powf((50.0 + depth * (TAU * 5.5 * ms / 1000.0).sin()) / 1200.0)
    });
    let chromatic = Guide { notes: Vec::new(), key_mask: EVERY_PITCH_CLASS };
    let output = through(&voice, strength, retune_ms, chromatic);
    let dry = cents_from_a3(&voice, 1000.0, 3900.0);
    let wet = cents_from_a3(&output, 1000.0, 3900.0);
    (band_rms(&dry, 100.0, 3.0, 12.0), band_rms(&wet, 100.0, 3.0, 12.0))
}

/// The bound row: a voice held 30 cents sharp of `hz`, corrected in full, so the shifter reads
/// behind and drifts away from the voice until it jumps back. What it lands at, and the most it
/// trailed by, in frames and milliseconds.
pub fn across_the_range(hz: f64) -> (f64, f64, f64) {
    let frames = RATE as usize * 2;
    let voice = harmonic_voice(frames, |_| hz * 2f64.powf(30.0 / 1200.0));
    let params = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };
    let mut setup = Setup::new(Input::Samples(voice), frames, params);
    setup.guide = Some(Guide { notes: Vec::new(), key_mask: EVERY_PITCH_CLASS });
    let take = run(setup);
    let output = take.voice();
    let settled = frames_of_ms(1000.0) as usize;
    let landed: Vec<f64> = contour::read(&output[settled..], 0)
        .iter()
        .filter(|point| point.voiced())
        .map(|point| cents(f64::from(point.hz), hz))
        .collect();
    let most = take.delays.iter().copied().max().unwrap_or(0);
    let delay_ms = f64::from(most) * 1000.0 / f64::from(RATE);
    (median(&landed), f64::from(most), delay_ms)
}

pub fn rows() -> Value {
    let mut vibrato_rows = serde_json::Map::new();
    for (label, strength, retune_ms) in settings() {
        for rate in [4.0, 5.5, 7.0, 10.0] {
            let (mean, kept) = vibrato(rate, 50.0, 30.0, strength, retune_ms);
            vibrato_rows.insert(
                format!("{label}.{rate}hz"),
                json!({ "mean_cents_from_target": mean, "depth_kept": kept }),
            );
        }
    }
    let mut scoop_rows = serde_json::Map::new();
    let mut vibrato_scoop_rows = serde_json::Map::new();
    let mut silent_scoop_rows = serde_json::Map::new();
    let mut fence_rows = serde_json::Map::new();
    for (label, strength, retune_ms) in settings() {
        for (approach, rows) in [
            (Approach::Held, &mut scoop_rows),
            (Approach::HeldWithVibrato, &mut vibrato_scoop_rows),
            (Approach::FromSilence, &mut silent_scoop_rows),
        ] {
            let settle_ms = scoop_settle_ms(approach, 100.0, 150.0, strength, retune_ms);
            rows.insert(label.to_owned(), json!({ "settle_ms": settle_ms }));
        }
        let (dry, wet) = on_the_fence(30.0, strength, retune_ms);
        fence_rows
            .insert(label.to_owned(), json!({ "motion_in_cents": dry, "motion_out_cents": wet }));
    }
    json!({
        "vibrato": vibrato_rows,
        "scoop": scoop_rows,
        "scoop_from_vibrato": vibrato_scoop_rows,
        "scoop_from_silence": silent_scoop_rows,
        "on_the_fence": fence_rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Off is the control: a voice with 50 cents of vibrato reads as that vibrato, 30 cents flat
    #[test]
    fn the_vibrato_row_reads_the_voice_it_was_given() {
        let frames = RATE as usize * 4;
        let voice = harmonic_voice(frames, |ms| {
            A3 * 2f64.powf((-30.0 + 50.0 * (TAU * 5.5 * ms / 1000.0).sin()) / 1200.0)
        });
        let read = cents_from_a3(&voice, 1000.0, 3900.0);
        let mean = read.iter().sum::<f64>() / read.len() as f64;
        assert!((mean + 30.0).abs() < 3.0, "mean {mean}");
        // RMS of a sine is depth over root two; the 43 ms window takes about 4 percent at 5.5 Hz
        let rms = band_rms(&read, 100.0, 4.0, 7.0);
        assert!((rms - 50.0 / 2f64.sqrt()).abs() < 4.0, "rms {rms}");
    }

    // A corrector at full strength and retune 0 pulls a flat voice's centre onto the note within
    // the 5 cents #46 asks, and the setting a person would call natural leaves it partly flat
    #[test]
    fn full_strength_centres_the_note_and_strength_scales_it() {
        let (hard, _) = vibrato(5.5, 50.0, 30.0, 1.0, 0.0);
        assert!(hard.abs() < 5.0, "came out {hard:+.1} cents from the note");
        let (natural, _) = vibrato(5.5, 50.0, 30.0, 0.6, 40.0);
        assert!((natural + 12.0).abs() < 5.0, "came out {natural:+.1} cents from the note");
    }

    // The textbook warble: a voice whose vibrato crosses the midpoint between two notes, which
    // flipped the target every half cycle at a hard retune and moved the output half as much again
    // as the voice. The held note's centre picks the note now, so every setting keeps one note and
    // the voice's own swing, within 10 percent
    #[test]
    fn a_voice_on_the_fence_keeps_one_note_and_its_swing() {
        for (label, strength, retune_ms) in settings() {
            let (dry, wet) = on_the_fence(30.0, strength, retune_ms);
            assert!((dry - 30.0 / 2f64.sqrt()).abs() < 3.0, "the voice moved {dry} cents");
            assert!((wet - dry).abs() < 0.1 * dry, "{label} moved {wet} cents from {dry}");
        }
    }

    // The bound row's own bound: 1.2 ms plus 1.125 periods of the voice
    #[test]
    fn a_held_voice_trails_by_at_most_the_bound_and_lands() {
        for hz in [82.5, 110.0, 220.0, 440.0] {
            let (landed, _, delay_ms) = across_the_range(hz);
            let bound = 1.23 + 1125.0 / hz;
            assert!(delay_ms <= bound + 0.1, "{hz} Hz trailed {delay_ms} ms, bound {bound}");
            assert!(landed.abs() < 5.0, "{hz} Hz landed {landed} cents off");
        }
    }

    // The scoop lands, and sooner the faster the retune
    #[test]
    fn a_scoop_settles_sooner_at_a_faster_retune() {
        let hard = scoop_settle_ms(Approach::Held, 100.0, 150.0, 1.0, 0.0).expect("hard settles");
        let soft = scoop_settle_ms(Approach::Held, 100.0, 150.0, 1.0, 80.0).expect("soft settles");
        assert!(hard < soft, "hard {hard} ms, soft {soft} ms");
    }
}
