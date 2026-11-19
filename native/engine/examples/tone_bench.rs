//! CPU of one slot's tone stage on one core: sixty seconds of a voice-like tone in the engine's
//! 128-frame blocks, best of three. The worst row keeps the de-esser cutting, so its shelf takes
//! new coefficients every sample. Opens no device.
//!
//! `cargo run --release -p engine --example tone_bench` from `native/`.

use std::f64::consts::TAU;
use std::time::Instant;

use engine::block::{BLOCK, RATE};
use engine::params::{ToneBand, ToneBands, ToneParams};
use engine::slot::effects::Tone;

const SECONDS: f64 = 60.0;
const RUNS: usize = 3;

/// Twelve harmonics of 180 Hz, and `hiss` of white noise over them.
fn voice_like(hiss: f64) -> Vec<f32> {
    let mut seed = 7_u32;
    (0..(SECONDS * f64::from(RATE)) as usize)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let voice: f64 =
                (1..=12).map(|h| (TAU * 180.0 * f64::from(h) * t).sin() / f64::from(h)).sum();
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(seed) / 4_294_967_296.0 - 0.5;
            (0.1 * voice + hiss * noise) as f32
        })
        .collect()
}

fn cpu_percent(params: &ToneParams, input: &[f32]) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..RUNS {
        let mut tone = Tone::new();
        let mut block = [0.0; BLOCK];
        let start = Instant::now();
        for run in input.as_chunks::<BLOCK>().0 {
            block.copy_from_slice(run);
            tone.process(params, &mut block);
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    100.0 * best / SECONDS
}

fn main() {
    let bands = [
        ToneBand::LowShelf { hz: 200.0, gain_db: 2.0 },
        ToneBand::Peak { hz: 400.0, gain_db: -3.0, q: 1.0 },
        ToneBand::Peak { hz: 1200.0, gain_db: -2.0, q: 2.0 },
        ToneBand::Peak { hz: 3000.0, gain_db: 4.0, q: 1.0 },
        ToneBand::Peak { hz: 5000.0, gain_db: -2.0, q: 3.0 },
        ToneBand::Peak { hz: 7000.0, gain_db: 2.0, q: 2.0 },
        ToneBand::Peak { hz: 9000.0, gain_db: -6.0, q: 4.0 },
        ToneBand::HighShelf { hz: 12_000.0, gain_db: -2.0 },
    ];
    let eight = ToneParams {
        enabled: true,
        bands: ToneBands::given(&bands).expect("eight bands"),
        air_db: 3.0,
        ..ToneParams::default()
    };
    let everything = ToneParams { plosive_db: 12.0, de_ess_db: 12.0, ..eight };
    let (voice, hissing) = (voice_like(0.0), voice_like(0.5));
    println!("| One slot, 48 kHz, 128-frame blocks | CPU, one core |");
    println!("|---|---|");
    for (label, params, input) in [
        ("eight bands and air", &eight, &voice),
        ("every part, the guard and de-esser at rest", &everything, &voice),
        ("every part, the de-esser cutting all the time", &everything, &hissing),
    ] {
        println!("| {label} | {:.2} % |", cpu_percent(params, input));
    }
}
