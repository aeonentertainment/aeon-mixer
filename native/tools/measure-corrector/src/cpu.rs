//! The CPU row: thread CPU time over audio time, so a busy machine inflates it less than a wall
//! clock would. It is still this machine's number, on whatever core the thread landed, so it is
//! the one row a diff does not treat as deterministic. Build with `--release`.

use cpu_time::ThreadTime;
use engine::block::RATE;
use engine::params::CorrectorParams;
use engine::slot::guide::Guide;
use kernels::{EVERY_PITCH_CLASS, PitchCorrector};
use serde_json::{Value, json};

use crate::chain::{Input, Setup, run};
use crate::music::{Lcg, median};

const REPEATS: usize = 5;
const ON: CorrectorParams = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 40.0 };
const OFF: CorrectorParams = CorrectorParams { enabled: false, strength: 0.0, retune_ms: 40.0 };

/// A buzzy 180 Hz tone with vibrato and a little noise, sixty seconds of it, which is what the
/// core's bench in Aeon Karaoke times.
fn bench_voice() -> Vec<f32> {
    let frames = 60 * RATE as usize;
    let mut rng = Lcg(7);
    let mut phase = 0.0f64;
    (0..frames)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let hz = 180.0 * 2f64.powf(0.03 * (std::f64::consts::TAU * 5.5 * t).sin());
            phase += std::f64::consts::TAU * hz / f64::from(RATE);
            let buzz: f64 = (1..=12).map(|h| (f64::from(h) * phase).sin() / f64::from(h)).sum();
            0.3 * buzz as f32 + 0.01 * rng.signed()
        })
        .collect()
}

fn percent_of_core(seconds: f64, frames: usize) -> f64 {
    seconds / (frames as f64 / f64::from(RATE)) * 100.0
}

/// The core with nothing around it, in the mixer's 10 ms runs, as the bench ran it.
fn core_alone(voice: &[f32], strength: f32) -> f64 {
    let mut corrector = PitchCorrector::new(RATE);
    corrector.set_target(EVERY_PITCH_CLASS, strength, 40.0);
    let mut output = vec![0.0; 480];
    let started = ThreadTime::now();
    for input in voice.chunks_exact(480) {
        corrector.process(input, &mut output);
    }
    percent_of_core(started.elapsed().as_secs_f64(), voice.len())
}

/// `voices` copies of `voice` through engaged slots, all into one graph.
fn slots(voice: &[f32], voices: usize, params: CorrectorParams) -> f64 {
    let mut setup = Setup::new(Input::Samples(voice.to_vec()), voice.len(), params);
    setup.guide = Some(Guide { notes: Vec::new(), key_mask: EVERY_PITCH_CLASS });
    setup.extra_voices = vec![voice.to_vec(); voices - 1];
    percent_of_core(run(setup).cpu_seconds, voice.len())
}

fn repeated(mut measure: impl FnMut() -> f64) -> Value {
    let runs: Vec<f64> = (0..REPEATS).map(|_| measure()).collect();
    json!({ "median": median(&runs), "min": runs.iter().copied().fold(f64::INFINITY, f64::min) })
}

pub fn rows(vocal: &[f32]) -> Value {
    let voice = bench_voice();
    json!({
        "build": if cfg!(debug_assertions) { "debug, not a number to quote" } else { "release" },
        "core_correcting": repeated(|| core_alone(&voice, 1.0)),
        "core_bypassed": repeated(|| core_alone(&voice, 0.0)),
        "slot_correcting": repeated(|| slots(vocal, 1, ON)),
        "slot_bypassed": repeated(|| slots(vocal, 1, OFF)),
        "four_slots_correcting": repeated(|| slots(vocal, 4, ON)),
    })
}
