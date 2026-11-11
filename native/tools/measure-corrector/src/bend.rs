//! The degrader's shifter: Signalsmith Stretch at its best-quality preset, offline, with its
//! latency taken off so the bent voice lines up with the voice sample for sample. It is not the
//! corrector's shifter, so the two cannot share an artefact and cancel it, and its 120 ms blocks
//! would never fit the mic path, which does not matter here.

use assert_no_alloc::assert_no_alloc;
use engine::block::RATE;
use kernels::{Preset, Stretch};

use crate::curve::Curve;

/// How often the transposition is set: finer than the stretcher's own 30 ms hop.
const CHUNK: usize = 240;

/// `samples` moved by `curve`, the same length and on the same clock.
pub fn bend(samples: &[f32], curve: &Curve) -> Vec<f32> {
    let mut stretch = Stretch::new(1, RATE, Preset::Default);
    let latency = stretch.latency_frames();
    // The block being synthesised when a chunk goes in sits half the latency back in the input
    let lead = latency / 2;
    let mut input = samples.to_vec();
    input.resize(samples.len() + latency + CHUNK, 0.0);
    let mut output = vec![0.0f32; input.len()];
    let frames_per_ms = f64::from(RATE) / 1000.0;
    for (start, (from, to)) in
        (0..).step_by(CHUNK).zip(input.chunks(CHUNK).zip(output.chunks_mut(CHUNK)))
    {
        let heard = start as f64 - lead as f64;
        stretch.set_transpose_semitones((curve.at(heard / frames_per_ms) / 100.0) as f32);
        assert_no_alloc(|| stretch.process(&[from], &mut [to]));
    }
    output.drain(..latency);
    output.truncate(samples.len());
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contour;
    use crate::music::cents;
    use crate::synthetic::harmonic_voice;

    fn cents_at(signal: &[f32], hz: f64, ms: f64) -> f64 {
        let points = contour::read(signal, 0);
        let point = points.iter().min_by(|a, b| (a.ms - ms).abs().total_cmp(&(b.ms - ms).abs()));
        cents(f64::from(point.expect("a reading").hz), hz)
    }

    // A held vowel bent by a 40 cent step reads the step within 3 cents and within 20 ms of where
    // it was asked, the stretcher's 30 ms hop. With no bend it comes back sample for sample. Bent
    // and bent back it lands within 4 cents: the stretcher overshoots a shift up by about 5
    // percent and not one down, which is why the rows print the round trip as their floor
    #[test]
    fn a_bend_lands_where_and_when_it_was_asked() {
        let voice = harmonic_voice(RATE as usize * 3, |_| 220.0);
        let step = Curve::sampled(3000.0, |ms| if ms < 1500.0 { 0.0 } else { 40.0 });
        let bent = bend(&voice, &step);
        assert!(cents_at(&bent, 220.0, 1000.0).abs() < 1.0);
        assert!((cents_at(&bent, 220.0, 2000.0) - 40.0).abs() < 3.0);
        let halfway =
            (1400i32..1600).step_by(10).find(|ms| cents_at(&bent, 220.0, f64::from(*ms)) > 20.0);
        assert!(halfway.is_some_and(|ms| (ms - 1500).abs() <= 20), "{halfway:?}");

        let still = bend(&voice, &Curve::sampled(3000.0, |_| 0.0));
        assert!(voice.iter().zip(&still).all(|(a, b)| (a - b).abs() < 1e-4));
        let back = bend(&bent, &step.undone());
        let landed = cents_at(&back, 220.0, 2000.0);
        assert!(landed.abs() < 4.0, "{landed}");
    }
}
