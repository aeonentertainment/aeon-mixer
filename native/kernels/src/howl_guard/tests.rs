use assert_no_alloc::assert_no_alloc;

use super::*;
use crate::fixtures::{at_db, db, rms, talker};
use crate::test_support::RATE;

const BLOCK: usize = 128;

/// `signal` through a guard, block by block under the allocation guard.
fn guarded(mut signal: Vec<f32>) -> (Vec<f32>, HowlGuard) {
    let mut guard = HowlGuard::new(RATE);
    for block in signal.as_chunks_mut::<BLOCK>().0.iter_mut() {
        assert_no_alloc(|| guard.process(block));
    }
    (signal, guard)
}

fn sine(hz: f32, frames: usize, level_db: impl Fn(usize) -> f32) -> Vec<f32> {
    (0..frames)
        .map(|i| {
            let amplitude = 10f32.powf(level_db(i) / 20.0);
            amplitude * (std::f32::consts::TAU * hz * i as f32 / RATE as f32).sin()
        })
        .collect()
}

// A singer is not a howl: a voice-shaped talker at a hot level for ten seconds, its pitch moving
// and its harmonics standing beside each peak, passes bit for bit.
#[test]
fn a_voice_passes_untouched() {
    let voice = at_db(talker(10 * RATE as usize, 160.0, 5), -12.0);
    let (out, guard) = guarded(voice.clone());
    assert_eq!(guard.catches(), 0);
    assert!(out.iter().zip(&voice).all(|(o, v)| o.to_bits() == v.to_bits()));
}

// A steady tone, a whistle or a held pure note, is not running away either. The guard tells a
// howl by its growth, so a tone that holds still at any level is left alone.
#[test]
fn a_steady_tone_is_left_alone() {
    let (_, guard) = guarded(sine(1000.0, 5 * RATE as usize, |_| -10.0));
    assert_eq!(guard.catches(), 0);
}

// A loop 2 dB over unity with an 80 ms round trip grows 25 dB a second. From -50 dBFS it is
// caught before it passes -30, within 400 ms, and the slot comes out 12 dB down. This tone is
// open loop, so the duck cannot stop it, and the guard ducks again each time it runs on: a
// loop the first duck leaves over unity is turned down further.
#[test]
fn a_tone_that_runs_away_is_ducked_and_ducked_again() {
    const GROWTH_DB_PER_SECOND: f32 = 25.0;
    let frames = 2 * RATE as usize;
    let level = |i: usize| -50.0 + GROWTH_DB_PER_SECOND * i as f32 / RATE as f32;
    let tone = sine(1200.0, frames, level);
    let (out, guard) = guarded(tone.clone());
    let caught = out
        .as_chunks::<BLOCK>()
        .0
        .iter()
        .zip(tone.as_chunks::<BLOCK>().0.iter())
        .position(|(o, t)| rms(o) < 0.9 * rms(t))
        .expect("ducked")
        * BLOCK;
    let caught_ms = caught as f32 * 1000.0 / RATE as f32;
    println!("caught at {caught_ms:.0} ms, at {:.1} dBFS", level(caught));
    assert!(caught_ms < 400.0 && level(caught) < -30.0, "caught at {caught_ms:.0} ms");
    let after = caught + RATE as usize / 50..caught + RATE as usize / 10;
    let ducked = db(rms(&out[after.clone()])) - db(rms(&tone[after]));
    assert!((ducked + 12.0).abs() < 0.5, "ducked by {ducked:.1} dB");
    assert!(guard.catches() > 1, "caught {} times", guard.catches());
}

// Ducking, the guard is a gain and nothing more: a click after a catch leaves on the very sample
// it entered, only quieter, so it costs the mic path no delay (docs/design.md 8.9).
#[test]
fn a_click_while_ducked_leaves_on_the_sample_it_entered() {
    let rate = RATE as usize;
    let mut signal = sine(1200.0, rate, |i| -50.0 + 25.0 * i as f32 / RATE as f32);
    let click = rate + rate / 5 + 77;
    signal.resize(click + rate / 10, 0.0);
    signal[click] = 0.5;
    let (out, guard) = guarded(signal);
    assert!(guard.catches() >= 1, "the tone was caught first");
    let loudest = (click..out.len()).max_by(|a, b| out[*a].abs().total_cmp(&out[*b].abs()));
    assert_eq!(loudest, Some(click));
    assert!(out[click] < 0.5 * 0.6, "ducked: the click came out at {}", out[click]);
}
