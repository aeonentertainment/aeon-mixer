//! Synthetic signals for testing and measuring the echo stages, the same in every crate that
//! asks for them: noise, a voice-shaped talker, a room and a mic on its own clock. Built with the
//! `fixtures` feature.

use std::f64::consts::TAU;

/// A repeatable uniform source in -1 to 1, the generator Aeon Karaoke's `aec:measure` uses.
pub struct Lcg(u32);

impl Lcg {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    pub fn sample(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (f64::from(self.0) / 4_294_967_296.0 * 2.0 - 1.0) as f32
    }
}

/// Paul Kellet's pink noise, as `aec:measure` makes its backing bed.
pub fn pink_noise(frames: usize, seed: u32) -> Vec<f32> {
    let mut rng = Lcg::new(seed);
    let mut b = [0.0f32; 7];
    (0..frames)
        .map(|_| {
            let w = rng.sample();
            b[0] = 0.99886 * b[0] + w * 0.055_517_9;
            b[1] = 0.99332 * b[1] + w * 0.075_075_9;
            b[2] = 0.969 * b[2] + w * 0.153_852;
            b[3] = 0.8665 * b[3] + w * 0.310_485_6;
            b[4] = 0.55 * b[4] + w * 0.532_952_2;
            b[5] = -0.7616 * b[5] - w * 0.016_898;
            let out = (b.iter().sum::<f32>() + w * 0.5362) * 0.11;
            b[6] = w * 0.115_926;
            out
        })
        .collect()
}

/// A voice-shaped talker: twelve harmonics falling as 1/h on a pitch that wanders between
/// `low_hz` and twice it, in syllables of 120 to 400 ms with gaps between them. It is not
/// speech, but it has speech's line spectrum, its gaps and its moving pitch.
pub fn talker(frames: usize, low_hz: f64, seed: u32) -> Vec<f32> {
    let rate = 48_000.0;
    let mut rng = Lcg::new(seed);
    let mut out = vec![0.0f32; frames];
    let mut at = 0usize;
    let mut phase = 0.0f64;
    while at < frames {
        let syllable = ((0.12 + 0.28 * f64::from(rng.sample().abs())) * rate) as usize;
        let gap = ((0.03 + 0.15 * f64::from(rng.sample().abs())) * rate) as usize;
        let start_hz = low_hz * (1.0 + f64::from(rng.sample().abs()));
        let end_hz = low_hz * (1.0 + f64::from(rng.sample().abs()));
        for i in 0..syllable.min(frames - at) {
            let progress = i as f64 / syllable as f64;
            let hz = start_hz + (end_hz - start_hz) * progress;
            phase = (phase + TAU * hz / rate) % (TAU * 1000.0);
            let envelope = (std::f64::consts::PI * progress).sin();
            let sample: f64 = (1..=12).map(|h| (phase * h as f64).sin() / h as f64).sum();
            out[at + i] = (0.25 * envelope * sample) as f32;
        }
        at += syllable + gap;
    }
    out
}

/// A room as a sparse impulse response: the direct path `delay` frames after the speaker at
/// `gain`, then reflections arriving at random under an exponential decay of `rt60` seconds,
/// for `length` seconds. Pairs of (lag in frames, gain).
pub fn room(delay: usize, gain: f32, rt60: f64, length: f64, seed: u32) -> Vec<(usize, f32)> {
    let rate = 48_000.0;
    let mut rng = Lcg::new(seed);
    let mut taps = vec![(delay, gain)];
    // A reflection a millisecond on average, as dense as a small room's early part
    let reflections = (length * 1000.0) as usize;
    for _ in 0..reflections {
        let after = f64::from(rng.sample().abs()) * length;
        let decay = 10f64.powf(-3.0 * after / rt60);
        let lag = delay + (after * rate) as usize + 24;
        taps.push((lag, gain * 0.35 * (decay as f32) * rng.sample()));
    }
    taps
}

/// `signal` through `room`.
pub fn through(signal: &[f32], room: &[(usize, f32)]) -> Vec<f32> {
    let mut out = vec![0.0f32; signal.len()];
    for &(lag, gain) in room {
        for (o, s) in out[lag.min(signal.len())..].iter_mut().zip(signal) {
            *o += gain * s;
        }
    }
    out
}

/// Scales `signal` to an RMS of `db` dBFS.
pub fn at_db(mut signal: Vec<f32>, db: f32) -> Vec<f32> {
    let gain = 10f32.powf(db / 20.0) / rms(&signal).max(1e-12);
    signal.iter_mut().for_each(|s| *s *= gain);
    signal
}

pub fn rms(signal: &[f32]) -> f32 {
    (signal.iter().map(|s| s * s).sum::<f32>() / signal.len().max(1) as f32).sqrt()
}

pub fn db(level: f32) -> f32 {
    20.0 * level.max(1e-12).log10()
}

/// `signal` at a moment `t` frames in, between its frames, as a converter on another clock
/// samples it: a Hann-windowed sinc 32 frames wide. Linear interpolation would add a lowpass that
/// sweeps with the two clocks' phase, which no real mic has. Silence outside the signal.
pub fn between(signal: &[f32], t: f64) -> f32 {
    const HALF: i64 = 16;
    let whole = t.floor() as i64;
    let fraction = t - whole as f64;
    (1 - HALF..=HALF)
        .filter_map(|m| {
            let sample = signal.get(usize::try_from(whole + m).ok()?)?;
            let x = m as f64 - fraction;
            let window = 0.5 + 0.5 * (std::f64::consts::PI * x / HALF as f64).cos();
            let angle = std::f64::consts::PI * x;
            let sinc = if x == 0.0 { 1.0 } else { angle.sin() / angle };
            Some(f64::from(*sample) * sinc * window)
        })
        .sum::<f64>() as f32
}

/// `signal` as a mic on a clock `ppm` fast or slow hears it: its frame `k` falls at
/// `k / (1 + ppm / 1e6)` on the signal's clock.
pub fn on_clock(signal: &[f32], ppm: f64) -> Vec<f32> {
    (0..signal.len()).map(|k| between(signal, k as f64 / (1.0 + ppm * 1e-6))).collect()
}
