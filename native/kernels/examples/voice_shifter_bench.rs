//! CPU and delay of the voice shifter per slot on one core, beside the corrector it would sit
//! with: sixty seconds of a voice-like tone in the engine's 128-frame blocks, best of three, so a
//! busy machine shows up as little as it can. Opens no device.
//!
//! `cargo run --release -p kernels --example voice_shifter_bench` from `native/`.

use std::f64::consts::PI;
use std::time::Instant;

use kernels::{EVERY_PITCH_CLASS, PitchCorrector, VoiceShifter};

const RATE: u32 = 48_000;
const BLOCK: usize = 128;
const SECONDS: f64 = 60.0;
const RUNS: usize = 3;

/// The corrector's own bench signal, so the corrector's row reads against Aeon Karaoke's: a buzzy
/// tone with 5.5 Hz vibrato and a little noise. `hz` is 180 for that row and 82 for a low voice.
fn voice_like(hz: f64) -> Vec<f32> {
    let mut seed = 7_u32;
    let mut phase = 0.0;
    (0..(SECONDS * f64::from(RATE)) as usize)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            phase += 2.0 * PI * hz * 2f64.powf(0.03 * (2.0 * PI * 5.5 * t).sin()) / f64::from(RATE);
            let voice: f64 = (1..=12).map(|h| (f64::from(h) * phase).sin() / f64::from(h)).sum();
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = f64::from(seed) / 4_294_967_296.0 - 0.5;
            (0.3 * voice + 0.02 * noise) as f32
        })
        .collect()
}

struct Measured {
    cpu_percent: f64,
    mean_ms: f64,
    most_ms: f64,
}

/// `process` over `input`, reading `latency` after every block.
fn measure(
    input: &[f32],
    mut process: impl FnMut(&[f32], &mut [f32]),
    latency: impl Fn() -> usize,
) -> Measured {
    let mut output = [0.0; BLOCK];
    let mut best = f64::MAX;
    let (mut sum, mut count, mut most) = (0, 0, 0);
    for _ in 0..RUNS {
        let start = Instant::now();
        for block in input.chunks_exact(BLOCK) {
            process(block, &mut output);
        }
        best = best.min(start.elapsed().as_secs_f64());
    }
    for block in input[..(5 * RATE) as usize].chunks_exact(BLOCK) {
        process(block, &mut output);
        let samples = latency();
        sum += samples;
        count += 1;
        most = most.max(samples);
    }
    let to_ms = 1000.0 / f64::from(RATE);
    Measured {
        cpu_percent: 100.0 * best / SECONDS,
        mean_ms: sum as f64 / count as f64 * to_ms,
        most_ms: most as f64 * to_ms,
    }
}

fn row(label: &str, measured: &Measured) {
    println!(
        "| {label} | {:.2} ms | {:.2} ms | {:.2} % |",
        measured.mean_ms, measured.most_ms, measured.cpu_percent
    );
}

fn shifter_row(label: &str, input: &[f32], semitones: f32, factor: f32) {
    let shifter = std::cell::RefCell::new(VoiceShifter::new(RATE));
    shifter.borrow_mut().set(semitones, factor);
    let measured = measure(
        input,
        |block, output| shifter.borrow_mut().process(block, output),
        || shifter.borrow().latency_samples(),
    );
    row(label, &measured);
}

fn main() {
    let voice = voice_like(180.0);
    let low = voice_like(82.0);
    println!("| 48 kHz mono, 128-frame blocks | Mean delay | Most delay | CPU, one core |");
    println!("|---|---|---|---|");

    let corrector = std::cell::RefCell::new(PitchCorrector::new(RATE));
    corrector.borrow_mut().set_target(EVERY_PITCH_CLASS, 1.0, 40.0);
    let measured = measure(
        &voice,
        |block, output| corrector.borrow_mut().process(block, output),
        || corrector.borrow().latency_samples(),
    );
    row("corrector, correcting (180 Hz)", &measured);

    shifter_row("voice shifter +6, formants held (180 Hz)", &voice, 6.0, 1.0);
    shifter_row("voice shifter -6, formants held (180 Hz)", &voice, -6.0, 1.0);
    shifter_row("voice shifter +6, formants x1.2 (180 Hz)", &voice, 6.0, 1.2);
    shifter_row("voice shifter 0, formants x1.2 (180 Hz)", &voice, 0.0, 1.2);
    shifter_row(
        "voice shifter +2, formants move with it (180 Hz)",
        &voice,
        2.0,
        2f32.powf(2.0 / 12.0),
    );
    shifter_row("voice shifter +6, formants held (82 Hz)", &low, 6.0, 1.0);
    shifter_row("voice shifter -6, formants held (82 Hz)", &low, -6.0, 1.0);
    shifter_row("voice shifter off", &voice, 0.0, 1.0);
}
