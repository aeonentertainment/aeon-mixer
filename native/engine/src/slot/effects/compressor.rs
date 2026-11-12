use crate::params::CleanupParams;
use crate::slot::effects::level::{amplitude, fade_to_unity, one_pole, power_db};

/// The static curve: over -30 dBFS RMS, 3 dB in becomes 1 dB out, through a 12 dB soft knee.
const THRESHOLD_DB: f32 = -30.0;
const RATIO: f32 = 3.0;
const KNEE_DB: f32 = 12.0;
/// The most the stage ever raises the voice. A compressor's makeup is gain on the mic, which is
/// what feeds back; capping it caps how much closer to feedback the stage can bring a slot. With
/// it, a voice at -21 dBFS RMS passes at unity, quieter ones gain up to this, louder ones lose.
const MAKEUP_DB: f32 = 6.0;
/// Two one-poles in a row smooth the level, so its ripple on an 82 Hz voice is 58 dB under the
/// voice instead of the 38 a single fast one leaves.
const DETECTOR_MS: f32 = 2.0;
/// No lookahead: an onset 30 dB over the threshold passes about 12 dB too loud for its first
/// 6 ms. The master's soft clipper is what stands behind that.
const ATTACK_MS: f32 = 1.0;
/// Program-dependent release: the fast branch lets go of a peak in 60 ms, the slow branch charges
/// only over a sustained phrase and lets go over 600 ms, and the stage takes the larger, so
/// between the syllables of a long phrase the gain holds still instead of pumping.
const RELEASE_MS: f32 = 60.0;
const SLOW_ATTACK_MS: f32 = 300.0;
const SLOW_RELEASE_MS: f32 = 600.0;
/// How quickly the makeup comes in when the stage is turned on.
const MAKEUP_MS: f32 = 50.0;

/// The slot's compressor: it narrows the gap between a phone at the lips and one held a foot
/// away, and between a singer's whisper and their belt. Feedforward with no lookahead, so it adds
/// no samples.
pub struct Compressor {
    smoothed: [f32; 2],
    fast_db: f32,
    slow_db: f32,
    makeup_db: f32,
    /// The linear gain the last sample got, which turning the stage off fades from.
    applied: f32,
    coefficients: Coefficients,
}

struct Coefficients {
    detector: f32,
    attack: f32,
    release: f32,
    slow_attack: f32,
    slow_release: f32,
    makeup: f32,
}

impl Compressor {
    pub fn new() -> Self {
        Self {
            smoothed: [0.0; 2],
            fast_db: 0.0,
            slow_db: 0.0,
            makeup_db: 0.0,
            applied: 1.0,
            coefficients: Coefficients {
                detector: one_pole(DETECTOR_MS),
                attack: one_pole(ATTACK_MS),
                release: one_pole(RELEASE_MS),
                slow_attack: one_pole(SLOW_ATTACK_MS),
                slow_release: one_pole(SLOW_RELEASE_MS),
                makeup: one_pole(MAKEUP_MS),
            },
        }
    }

    /// One run of the slot's voice, in place. Turned off, it fades to unity across one block and
    /// then costs nothing.
    pub fn process(&mut self, params: &CleanupParams, block: &mut [f32]) {
        if !params.enabled {
            if self.applied != 1.0 {
                fade_to_unity(self.applied, block);
                *self = Self::new();
            }
            return;
        }
        let c = &self.coefficients;
        for sample in block {
            let [first, second] = &mut self.smoothed;
            *first += c.detector * (*sample * *sample - *first);
            *second += c.detector * (*first - *second);
            let reduction = reduction_db(power_db(*second));

            let toward = if reduction > self.fast_db { c.attack } else { c.release };
            self.fast_db += toward * (reduction - self.fast_db);
            let toward = if reduction > self.slow_db { c.slow_attack } else { c.slow_release };
            self.slow_db += toward * (reduction - self.slow_db);
            self.makeup_db += c.makeup * (MAKEUP_DB - self.makeup_db);

            self.applied = amplitude(self.makeup_db - self.fast_db.max(self.slow_db));
            *sample *= self.applied;
        }
    }
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

/// How many dB the static curve takes off a voice at `level_db`.
fn reduction_db(level_db: f32) -> f32 {
    let over = level_db - THRESHOLD_DB;
    let slope = 1.0 - 1.0 / RATIO;
    if over <= -KNEE_DB / 2.0 {
        0.0
    } else if over >= KNEE_DB / 2.0 {
        slope * over
    } else {
        slope * (over + KNEE_DB / 2.0).powi(2) / (2.0 * KNEE_DB)
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::TAU;

    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::block::{BLOCK, RATE};

    const ON: CleanupParams = CleanupParams { enabled: true };
    const MS: usize = RATE as usize / 1000;

    /// A sine at `hz` and `db` dBFS RMS, `ms` long.
    fn sine(hz: f64, db: f32, ms: usize) -> Vec<f32> {
        let peak = f64::from(amplitude(db)) * 2f64.sqrt();
        (0..ms * MS)
            .map(|i| (peak * (TAU * hz * i as f64 / f64::from(RATE)).sin()) as f32)
            .collect()
    }

    fn through(compressor: &mut Compressor, mut signal: Vec<f32>) -> Vec<f32> {
        assert_no_alloc(|| {
            signal.chunks_mut(BLOCK).for_each(|block| compressor.process(&ON, block))
        });
        signal
    }

    fn rms_db(samples: &[f32]) -> f32 {
        power_db(samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32)
    }

    /// The steady gain on a 220 Hz sine at `db`, read over its last 200 ms.
    fn steady_gain_db(db: f32) -> f32 {
        let output = through(&mut Compressor::new(), sine(220.0, db, 3_000));
        rms_db(&output[2_800 * MS..]) - db
    }

    /// The power at `hz` in `signal`, by Goertzel over a whole number of its cycles.
    fn power_at(signal: &[f32], hz: f64) -> f64 {
        let cycles = (signal.len() as f64 * hz / f64::from(RATE)).floor();
        let frames = (cycles * f64::from(RATE) / hz).round() as usize;
        let coefficient = 2.0 * (TAU * hz / f64::from(RATE)).cos();
        let (mut s1, mut s2) = (0.0f64, 0.0f64);
        for sample in &signal[..frames] {
            let s0 = f64::from(*sample) + coefficient * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        s1 * s1 + s2 * s2 - coefficient * s1 * s2
    }

    // The done-when: a phone a foot away and one at the lips, 12 dB apart, come out 5 dB apart
    #[test]
    fn a_12_db_swing_comes_out_5_db_wide() {
        let width = steady_gain_db(-18.0) + 12.0 - steady_gain_db(-30.0);
        assert!((width - 5.0).abs() < 0.3, "12 dB in came out {width:.2} dB wide");
    }

    // Feedback safety: at no level does the stage raise the voice by more than its makeup
    #[test]
    fn it_never_raises_a_voice_more_than_6_db() {
        for db in (-90..=0).step_by(6) {
            let gain = steady_gain_db(db as f32);
            assert!(gain <= MAKEUP_DB + 0.05, "{db} dBFS gained {gain:.2} dB");
        }
        assert!((steady_gain_db(-60.0) - MAKEUP_DB).abs() < 0.05);
    }

    // An 82 Hz sine, the lowest corrected voice, 18 dB over the threshold: the gain's ripple on
    // it puts every harmonic together at least 50 dB under the fundamental
    #[test]
    fn a_low_voice_under_heavy_compression_stays_clean() {
        let output = through(&mut Compressor::new(), sine(82.0, -12.0, 2_000));
        let settled = &output[RATE as usize..];
        let harmonics: f64 = (2..=10).map(|h| power_at(settled, 82.0 * f64::from(h))).sum();
        let distortion = 10.0 * (harmonics / power_at(settled, 82.0)).log10();
        assert!(distortion < -50.0, "harmonics at {distortion:.1} dB");
    }

    // A 20 ms shout lets go within 250 ms; after a 3 s phrase the gain is still holding then, so a
    // breath in a long phrase does not pump the room up
    #[test]
    fn release_follows_how_long_the_loud_part_lasted() {
        let still_reduced_db = |loud_ms: usize| {
            let mut signal = sine(220.0, -40.0, 1_000);
            signal.extend(sine(220.0, -10.0, loud_ms));
            signal.extend(sine(220.0, -40.0, 400));
            let output = through(&mut Compressor::new(), signal);
            let at = (1_000 + loud_ms + 250) * MS;
            MAKEUP_DB - (rms_db(&output[at..at + 10 * MS]) + 40.0)
        };
        let after_shout = still_reduced_db(20);
        let after_phrase = still_reduced_db(3_000);
        assert!(after_shout < 1.0, "a shout still held {after_shout:.1} dB");
        assert!(after_phrase > 4.0, "a phrase held only {after_phrase:.1} dB");
    }

    // Turned off while it holds a quiet voice up by 6 dB, the gain walks back to unity across one
    // block in even steps, and from the next block the voice is untouched
    #[test]
    fn turning_it_off_lets_go_without_a_step() {
        let mut stage = Compressor::new();
        through(&mut stage, vec![0.01; 500 * MS]);
        let off = CleanupParams { enabled: false };
        let mut blocks = [0.01f32; 2 * BLOCK];
        assert_no_alloc(|| blocks.chunks_mut(BLOCK).for_each(|block| stage.process(&off, block)));
        let gains: Vec<f32> = blocks.iter().map(|s| s / 0.01).collect();
        let start = amplitude(MAKEUP_DB);
        let even = (start - 1.0) / BLOCK as f32;
        assert!((gains[0] - (start - even)).abs() < 1e-3, "first sample at {}", gains[0]);
        for pair in gains[..BLOCK].windows(2) {
            assert!((pair[0] - pair[1] - even).abs() < 1e-4, "step {} to {}", pair[0], pair[1]);
        }
        assert!(gains[BLOCK..].iter().all(|gain| (gain - 1.0).abs() < 1e-6));
    }
}
