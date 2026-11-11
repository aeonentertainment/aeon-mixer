//! Clicks found the way audio restoration finds them (Vaseghi and Rayner 1990; Godsill and Rayner,
//! Digital Audio Restoration, 1998): fit an autoregressive model to each block, and call a sample
//! a click where the model's prediction misses by many times the block's typical miss. A voice is
//! predictable from its last few milliseconds; a discontinuity is not. A singer's own mouth clicks
//! and plosives miss too, 7 to 20 a minute on vocadito, so a row counts only the clicks in the
//! output with none in the input beside them.

use crate::music::median;

const BLOCK: usize = 1024;
const ORDER: usize = 32;
/// A miss this many robust deviations out: a step of a hundredth of full scale in a held vowel
/// reads, and a harmonic vowel with vibrato reads nothing.
const THRESHOLD: f64 = 12.0;
/// Flagged samples this close together are one click.
const ONE_CLICK: usize = 96;

/// Where the clicks in `signal` are, as sample indices.
pub fn find(signal: &[f32]) -> Vec<usize> {
    let mut clicks = Vec::new();
    for start in (ORDER..signal.len().saturating_sub(BLOCK)).step_by(BLOCK) {
        let block = &signal[start..start + BLOCK];
        let Some(coefficients) = predictor(block) else { continue };
        let residual: Vec<f64> = (start..start + BLOCK)
            .map(|n| {
                let predicted: f64 = coefficients
                    .iter()
                    .enumerate()
                    .map(|(k, a)| a * f64::from(signal[n - k - 1]))
                    .sum();
                f64::from(signal[n]) - predicted
            })
            .collect();
        let magnitudes: Vec<f64> = residual.iter().map(|e| e.abs()).collect();
        let spread = 1.4826 * median(&magnitudes);
        if spread <= 1e-7 {
            continue;
        }
        for (offset, miss) in magnitudes.iter().enumerate() {
            let at = start + offset;
            if *miss > THRESHOLD * spread && clicks.last().is_none_or(|last| at - last > ONE_CLICK)
            {
                clicks.push(at);
            }
        }
    }
    clicks
}

/// Clicks in `wet` with no click in `dry` within this, once `wet` is on `dry`'s clock.
const BESIDE: usize = 240;

/// Clicks in `wet` that `dry` did not have, given where each of `wet`'s samples sits in `dry`.
pub fn added(dry: &[f32], wet: &[f32], on_dry_clock: impl Fn(usize) -> usize) -> usize {
    let heard = find(dry);
    find(wet)
        .into_iter()
        .map(on_dry_clock)
        .filter(|at| {
            let next = heard.partition_point(|click| click < at);
            let near = |index: Option<usize>| {
                index.and_then(|i| heard.get(i)).is_some_and(|c| c.abs_diff(*at) <= BESIDE)
            };
            !near(next.checked_sub(1)) && !near(Some(next))
        })
        .count()
}

/// The block's linear predictor by autocorrelation and Levinson-Durbin, windowed; `None` for a
/// block too quiet to model.
fn predictor(block: &[f32]) -> Option<[f64; ORDER]> {
    let n = block.len();
    let windowed: Vec<f64> = block
        .iter()
        .enumerate()
        .map(|(i, s)| {
            f64::from(*s) * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())
        })
        .collect();
    let r: Vec<f64> = (0..=ORDER)
        .map(|lag| windowed.iter().zip(&windowed[lag..]).map(|(a, b)| a * b).sum())
        .collect();
    if r[0] <= 1e-9 {
        return None;
    }
    let mut a = [0.0f64; ORDER];
    let mut error = r[0] * (1.0 + 1e-9);
    for i in 0..ORDER {
        let reflection = (r[i + 1] - (0..i).map(|j| a[j] * r[i - j]).sum::<f64>()) / error;
        let previous = a;
        a[i] = reflection;
        for j in 0..i {
            a[j] = previous[j] - reflection * previous[i - 1 - j];
        }
        error *= 1.0 - reflection * reflection;
        if error <= 0.0 {
            return None;
        }
    }
    Some(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synthetic::harmonic_voice;

    // A held vowel with steps of a hundredth of full scale every 300 ms: every one found, at its
    // sample within a millisecond, and nothing else
    #[test]
    fn a_step_in_a_held_vowel_is_found_and_the_vowel_is_not() {
        let clean = harmonic_voice(48_000 * 3, |ms| 180.0 + 20.0 * (ms / 300.0).sin());
        assert!(find(&clean).is_empty(), "a clean vowel read {:?}", find(&clean));
        let steps: Vec<usize> = (1..10).map(|i| i * 14_400 + 77).collect();
        let mut clicked = clean.clone();
        for &at in &steps {
            clicked[at..].iter_mut().for_each(|s| *s += 0.01);
        }
        let found = find(&clicked);
        assert_eq!(found.len(), steps.len(), "found {found:?}");
        assert!(found.iter().zip(&steps).all(|(f, s)| f.abs_diff(*s) <= 48), "{found:?}");
        // Only the clicks the input lacks are added: here the input had the first five
        let mut half = clean.clone();
        for &at in &steps[..5] {
            half[at..].iter_mut().for_each(|s| *s += 0.01);
        }
        assert_eq!(added(&half, &clicked, |at| at), 4);
        assert_eq!(added(&clicked, &half, |at| at), 0);
    }
}
