//! The added latency row. A click has no pitch, so it reads the shifter at rest; a real vocal reads
//! how far it trails while it corrects, the way the GStreamer harness sampled its elements.

use engine::block::{BLOCK, RATE};
use engine::params::{CorrectorParams, ReverbParams};
use engine::slot::guide::Guide;
use serde_json::{Value, json};

use crate::chain::{Input, Setup, run};
use crate::music::{Lcg, key_mask, percentile};
use crate::synthetic::across_the_range;

const ON: CorrectorParams = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 40.0 };
const OFF: CorrectorParams = CorrectorParams { enabled: false, strength: 0.0, retune_ms: 0.0 };
/// The reverb the harness ran with: `CHANNEL_EFFECTS_OFF`'s mix, on.
const REVERB: ReverbParams = ReverbParams { enabled: true, room_size: 0.5, mix: 0.25 };

/// A 1 ms noise burst one second into two seconds of silence.
fn click() -> Vec<f32> {
    let mut rng = Lcg(3);
    let mut samples = vec![0.0; 2 * RATE as usize];
    for sample in &mut samples[RATE as usize..RATE as usize + 48] {
        *sample = rng.signed() * 0.5;
    }
    samples
}

/// How long after the input's click the output's arrives, in frames, read at 20 percent of its
/// peak as the harness did.
pub fn click_delay_frames(corrector: CorrectorParams, reverb: ReverbParams) -> i64 {
    let samples = click();
    let frames = samples.len();
    let mut setup = Setup::new(Input::Samples(samples), frames, corrector);
    setup.reverb = reverb;
    setup.guide = Some(Guide { notes: Vec::new(), key_mask: key_mask("C").expect("a key") });
    let voice = run(setup).voice();
    let peak = voice.iter().fold(0.0f32, |peak, s| peak.max(s.abs()));
    let onset = voice.iter().position(|s| s.abs() > peak * 0.2);
    onset.expect("the click comes out") as i64 - i64::from(RATE)
}

/// The delay every 50 ms between `from` and `to` seconds, as the harness sampled it, in key mode
/// at full strength as its four slots sang, in milliseconds.
fn delays_over_vocal(samples: &[f32], key: &str, from: f64, to: f64) -> Vec<f64> {
    let mut setup = Setup::new(Input::Samples(samples.to_vec()), samples.len(), ON);
    setup.guide = Some(Guide { notes: Vec::new(), key_mask: key_mask(key).expect("a known key") });
    let delays = run(setup).delays;
    let sample_every = RATE as usize / 20;
    (0..)
        .map(|k| k * sample_every)
        .take_while(|frame| frame / BLOCK < delays.len())
        .filter(|frame| {
            let seconds = *frame as f64 / f64::from(RATE);
            from <= seconds && seconds < to
        })
        .map(|frame| f64::from(delays[frame / BLOCK]) * 1000.0 / f64::from(RATE))
        .collect()
}

/// The vocal's delay in `key`, at start offsets a live slot could have. The delay is a sawtooth,
/// so a few milliseconds' start offset moves its p95 by more than a change worth quoting; the row
/// is the range, and the first offset alone is what a diff follows.
pub fn delay_over_vocal(samples: &[f32], key: &str) -> Value {
    let runs: Vec<Vec<f64>> = START_OFFSETS
        .iter()
        .map(|offset| delays_over_vocal(&samples[*offset..], key, 3.0, 13.0))
        .collect();
    let stat = |read: &dyn Fn(&[f64]) -> f64| -> Vec<f64> {
        runs.iter().map(|delays| read(delays)).collect()
    };
    let span = |values: Vec<f64>| {
        json!({
            "first": values[0],
            "lowest": values.iter().copied().fold(f64::INFINITY, f64::min),
            "highest": values.iter().copied().fold(0.0, f64::max),
        })
    };
    json!({
        "mean_ms": span(stat(&|d| d.iter().sum::<f64>() / d.len().max(1) as f64)),
        "p95_ms": span(stat(&|d| percentile(d, 0.95))),
        "max_ms": span(stat(&|d| d.iter().copied().fold(0.0, f64::max))),
    })
}

/// A live slot starts wherever its first packet lands; these cover a 10 ms hop's worth and more.
const START_OFFSETS: [usize; 4] = [0, 600, 1200, 1800];

fn ms(frames: i64) -> f64 {
    frames as f64 * 1000.0 / f64::from(RATE)
}

pub fn rows(golden_vocal: Option<&[f32]>) -> Value {
    let chain_off = click_delay_frames(OFF, ReverbParams::default());
    let at_rest = click_delay_frames(ON, ReverbParams::default());
    let whole_chain = click_delay_frames(ON, REVERB);
    let mut range = serde_json::Map::new();
    for hz in [82.5, 110.0, 220.0, 440.0, 880.0] {
        let (landed, frames, delay) = across_the_range(hz);
        range.insert(
            format!("{hz}hz"),
            json!({ "landed_cents": landed, "max_delay_frames": frames, "max_delay_ms": delay }),
        );
    }
    json!({
        "off_ms": ms(chain_off),
        "at_rest_ms": ms(at_rest),
        "whole_chain_at_rest_ms": ms(whole_chain),
        "golden_vocal": golden_vocal.map(|vocal| json!({
            "key_c": delay_over_vocal(vocal, "C"),
            "song_key_g": delay_over_vocal(vocal, "G"),
            "chromatic": delay_over_vocal(vocal, "chromatic"),
        })),
        "range": range,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Off is the input on its sample. On at rest is the shifter's floor, 59 frames: room for a
    // jump's crossfade at six semitones, whatever the reverb does after it
    #[test]
    fn off_adds_nothing_and_on_at_rest_is_the_shifters_floor() {
        assert_eq!(click_delay_frames(OFF, ReverbParams::default()), 0);
        let floor = click_delay_frames(ON, ReverbParams::default());
        assert!((55..=64).contains(&floor), "at rest, {floor} frames");
        assert_eq!(click_delay_frames(ON, REVERB), floor, "the reverb has no lookahead");
    }
}
