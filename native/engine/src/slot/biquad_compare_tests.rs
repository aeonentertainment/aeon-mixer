//! For #217 only, never merged: cleanup's high-pass before and after it moved onto the shared
//! biquad, fed one known buffer.

use std::f64::consts::{FRAC_1_SQRT_2, TAU};

use assert_no_alloc::assert_no_alloc;

use super::*;
use crate::command::event_ring;
use crate::driver::OfflineDriver;
use crate::graph::Graph;
use crate::params::{CleanupParams, Params};

/// `high_pass.rs` as it was on main at c4944fa, verbatim but for the name.
struct Before {
    b: [f64; 3],
    a: [f64; 2],
    state: [f64; 2],
    wet: f32,
}

impl Before {
    fn new() -> Self {
        let w0 = TAU * 80.0 / f64::from(RATE);
        let (sin, cos) = w0.sin_cos();
        let alpha = sin / (2.0 * FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        let b = [(1.0 + cos) / 2.0 / a0, -(1.0 + cos) / a0, (1.0 + cos) / 2.0 / a0];
        Self { b, a: [-2.0 * cos / a0, (1.0 - alpha) / a0], state: [0.0; 2], wet: 0.0 }
    }

    fn process(&mut self, params: &CleanupParams, block: &mut [f32]) {
        let target = if params.enabled { 1.0 } else { 0.0 };
        if target == 0.0 && self.wet == 0.0 {
            return;
        }
        let step = (target - self.wet) / block.len().max(1) as f32;
        let [b0, b1, b2] = self.b;
        let [a1, a2] = self.a;
        for sample in block {
            let dry = f64::from(*sample);
            let filtered = b0 * dry + self.state[0];
            self.state[0] = b1 * dry - a1 * filtered + self.state[1];
            self.state[1] = b2 * dry - a2 * filtered;
            self.wet += step;
            *sample += self.wet * (filtered as f32 - *sample);
        }
        self.wet = target;
        if target == 0.0 {
            self.state = [0.0; 2];
        }
    }
}

/// Six seconds: twelve harmonics of 110 Hz, 25 Hz rumble, a click every quarter second and
/// white noise, peaking near full scale.
fn known() -> Vec<f32> {
    let mut seed = 11_u32;
    (0..6 * RATE as usize)
        .map(|i| {
            let t = i as f64 / f64::from(RATE);
            let voice: f64 =
                (1..=12).map(|h| (TAU * 110.0 * f64::from(h) * t).sin() / f64::from(h)).sum();
            let rumble = 0.3 * (TAU * 25.0 * t).sin();
            let click = if i % 12_000 == 777 { 0.6 } else { 0.0 };
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let noise = 0.05 * (f64::from(seed) / 4_294_967_296.0 - 0.5);
            (0.2 * voice + rumble + click + noise) as f32
        })
        .collect()
}

/// On from the third block, off for blocks 375 to 399, on again after.
fn enabled(block: usize) -> bool {
    block >= 3 && !(375..400).contains(&block)
}

/// How many f32 steps apart two samples are.
fn ulps(a: f32, b: f32) -> u32 {
    let ordered = |x: f32| {
        let bits = x.to_bits() as i32;
        if bits < 0 { i32::MIN - bits } else { bits }
    };
    ordered(a).abs_diff(ordered(b))
}

#[test]
fn the_high_pass_before_and_after() {
    let input = known();
    let (mut before, mut after) = (Before::new(), HighPass::new());
    let (mut old, mut new) = (input.clone(), input.clone());
    for (block, (old, new)) in old.chunks_mut(BLOCK).zip(new.chunks_mut(BLOCK)).enumerate() {
        let params = CleanupParams { enabled: enabled(block) };
        assert_no_alloc(|| before.process(&params, old));
        assert_no_alloc(|| after.process(&params, new));
    }
    let differing = old.iter().zip(&new).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
    let most_ulps = old.iter().zip(&new).map(|(a, b)| ulps(*a, *b)).max().unwrap();
    let most_abs = old.iter().zip(&new).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    println!(
        "high-pass: {differing} of {} samples differ, at most {most_ulps} ulp, {most_abs:e} absolute",
        old.len()
    );
    assert!(most_ulps <= 1, "{most_ulps} ulp apart");
}

/// The slot's left channel with cleanup on, then off, then on, through a graph on the offline
/// driver, written as raw f32 bits to `$BIQUAD_DUMP` when it is set.
#[test]
fn the_slot_with_cleanup_on_the_offline_driver() {
    let input = known();
    let length = input.len();
    let mut graph = Graph::new(vec![Slot::new(Box::new(Clip { samples: input, at: 0 }))]);
    let (mut events, _drain) = event_ring();
    let mut params = Params::default();
    params.slots[0].pan = -1.0;
    let mut block = 0;
    let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
        params.slots[0].effects.cleanup = CleanupParams { enabled: enabled(block) };
        block += 1;
        graph.render(&params, out, &mut events);
    }));
    let mut left: Vec<f32> = Vec::with_capacity(length);
    let mut interleaved = [0.0; 2 * BLOCK];
    while left.len() < length {
        assert_no_alloc(|| driver.pump_into(&mut interleaved));
        left.extend(interleaved.iter().step_by(2));
    }
    if let Ok(path) = std::env::var("BIQUAD_DUMP") {
        let bytes: Vec<u8> = left.iter().flat_map(|s| s.to_bits().to_le_bytes()).collect();
        std::fs::write(path, bytes).unwrap();
    }
}

struct Clip {
    samples: Vec<f32>,
    at: usize,
}

impl Source for Clip {
    fn pull(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.samples.get(self.at).copied().unwrap_or(0.0);
            self.at += 1;
        }
    }
}

#[test]
fn the_f64_inside() {
    use crate::slot::effects::tone::{Biquad, Section};
    let before = Before::new();
    let after = Biquad::high_pass(80.0);
    println!("b before {:?} after {:?}", before.b, after.b);
    println!("a before {:?} after {:?}", before.a, after.a);
    let input = known();
    let (mut state, mut section) = ([0.0f64; 2], Section::new());
    let [b0, b1, b2] = before.b;
    let [a1, a2] = before.a;
    let mut most = 0.0f64;
    let mut most_rel = 0.0f64;
    for x in input.iter().map(|s| f64::from(*s)) {
        let old = b0 * x + state[0];
        state[0] = b1 * x - a1 * old + state[1];
        state[1] = b2 * x - a2 * old;
        let new = section.tick(&after, x);
        most = most.max((old - new).abs());
        if old != 0.0 {
            most_rel = most_rel.max(((old - new) / old).abs());
        }
    }
    println!("f64 inside: at most {most:e} apart, {most_rel:e} relative");
}
