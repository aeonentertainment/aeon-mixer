//! Where a reference recording sits against a replayed take: a reference carries no clock, so the
//! two are lined up by their levels (docs/design.md 8.9), first every 10 ms within 60 s either
//! way, then every millisecond around the best of those.

/// 10 ms of samples, the coarse step.
const COARSE: usize = 480;
/// 1 ms, the fine step and so the line-up's resolution.
const FINE: usize = 48;
/// The furthest either recording may start before the other: 60 s.
const MOST_LAG: i64 = 60 * 48_000;
/// Under this the best lag is as likely chance as the same singing.
const LEAST_CORRELATION: f64 = 0.5;

pub struct LineUp {
    /// Samples into the reference at which the take's first sample falls, negative when the
    /// reference started after the take.
    pub lag: i64,
    pub correlation: f64,
}

impl LineUp {
    /// The lag at which the two recordings' levels correlate best, over an overlap of at least half
    /// the take, or the best correlation found when it is too weak to trust. The lag is the fine
    /// search's and the correlation the coarse one's, the figure `LEAST_CORRELATION` judged: 1 ms
    /// levels follow where each window falls in a pitch period, which two mics' phase responses
    /// or a lossy codec move apart, so they read lower than the line-up is.
    pub fn find(take: &[f32], reference: &[f32]) -> Result<Self, f64> {
        let reach = MOST_LAG / COARSE as i64;
        let coarse = best(take, reference, COARSE, -reach..=reach);
        if coarse.correlation < LEAST_CORRELATION {
            return Err(coarse.correlation);
        }
        let around = coarse.lag / FINE as i64;
        let steps = (COARSE / FINE) as i64;
        let fine = best(take, reference, FINE, around - steps..=around + steps);
        Ok(Self { lag: fine.lag, correlation: coarse.correlation })
    }
}

/// The best of `lags`, counted in `window`s, by the correlation of each window's level in dB.
fn best(
    take: &[f32],
    reference: &[f32],
    window: usize,
    lags: std::ops::RangeInclusive<i64>,
) -> LineUp {
    let (take, reference) = (levels(take, window), levels(reference, window));
    let least = take.len().div_ceil(2).max(1) as i64;
    let mut best = LineUp { lag: 0, correlation: f64::NEG_INFINITY };
    for lag in lags {
        let start = (-lag).max(0);
        let end = (take.len() as i64).min(reference.len() as i64 - lag);
        if end - start < least {
            continue;
        }
        let shifted = (start + lag) as usize..(end + lag) as usize;
        let correlation = pearson(&take[start as usize..end as usize], &reference[shifted]);
        if correlation > best.correlation {
            best = LineUp { lag: lag * window as i64, correlation };
        }
    }
    best
}

/// Each window's level in dB, so a quiet recording lines up as well as a loud one.
fn levels(samples: &[f32], window: usize) -> Vec<f64> {
    let power = |w: &[f32]| w.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / w.len() as f64;
    samples.chunks_exact(window).map(|w| 10.0 * (power(w) + 1e-12).log10()).collect()
}

fn pearson(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len() as f64;
    let (mean_a, mean_b) = (a.iter().sum::<f64>() / n, b.iter().sum::<f64>() / n);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x - mean_a, y - mean_b);
        ab += x * y;
        aa += x * x;
        bb += y * y;
    }
    if aa == 0.0 || bb == 0.0 { 0.0 } else { ab / (aa * bb).sqrt() }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Two sides whose fine structure is unrelated, as a lossy codec or another phase response makes
    // it: on each, every millisecond's level jumps by up to 10 dB on its own, over one shared
    // envelope that moves by up to 8 dB every 100 ms. Their 1 ms levels correlate under the bar
    // and their 10 ms levels well over it, and the figure reported is the one the bar judged
    #[test]
    fn the_correlation_reported_is_the_one_the_bar_judged() {
        let mut seed = 3u32;
        let mut uniform = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / (1 << 23) as f32 - 1.0
        };
        let envelope: Vec<f32> = (0..60).map(|_| 8.0 * uniform()).collect();
        let mut side = |at: usize| -> Vec<f32> {
            let mut out = vec![0.0; at];
            for envelope_db in &envelope {
                for _ in 0..100 {
                    let gain = 10f32.powf((envelope_db + 10.0 * uniform()) / 20.0);
                    out.extend((0..FINE).map(|_| 0.01 * gain * uniform()));
                }
            }
            out
        };
        let lead = 14_400;
        let (take, reference) = (side(0), side(lead));
        let found = LineUp::find(&take, &reference).unwrap();
        let reach = MOST_LAG / COARSE as i64;
        let coarse = best(&take, &reference, COARSE, -reach..=reach);
        let fine = best(
            &take,
            &reference,
            FINE,
            coarse.lag / FINE as i64 - 10..=coarse.lag / FINE as i64 + 10,
        );
        assert!(fine.correlation < LEAST_CORRELATION, "fine {:.2}", fine.correlation);
        assert!(coarse.correlation > LEAST_CORRELATION, "coarse {:.2}", coarse.correlation);
        assert_eq!(found.correlation, coarse.correlation);
        assert_eq!(found.lag, lead as i64);
    }
}
