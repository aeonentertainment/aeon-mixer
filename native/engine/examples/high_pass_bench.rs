//! For #217 only, never merged: CPU of cleanup's high-pass alone, on, sixty seconds of a
//! voice-like tone in 128-frame blocks, best of five.

use std::f64::consts::TAU;
use std::hint::black_box;
use std::time::Instant;

use engine::block::{BLOCK, RATE};
use engine::params::CleanupParams;
use engine::slot::effects::HighPass;

const SECONDS: f64 = 60.0;

fn main() {
    let input: Vec<f32> = (0..(SECONDS * f64::from(RATE)) as usize)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let voice: f64 =
                (1..=12).map(|h| (TAU * 180.0 * f64::from(h) * t).sin() / f64::from(h)).sum();
            (0.1 * voice + 0.2 * (TAU * 30.0 * t).sin()) as f32
        })
        .collect();
    let on = CleanupParams { enabled: true };
    let mut runs = Vec::new();
    for _ in 0..5 {
        let mut filter = HighPass::new();
        let mut block = [0.0; BLOCK];
        let start = Instant::now();
        for run in input.as_chunks::<BLOCK>().0 {
            block.copy_from_slice(run);
            filter.process(&on, black_box(&mut block));
        }
        runs.push(start.elapsed().as_secs_f64());
    }
    let best = runs.iter().copied().fold(f64::MAX, f64::min);
    let ns_per_block = best * 1e9 / input.as_chunks::<BLOCK>().0.len() as f64;
    println!(
        "high-pass on: {:.4} % of one core, {ns_per_block:.1} ns a block",
        100.0 * best / SECONDS
    );
}
