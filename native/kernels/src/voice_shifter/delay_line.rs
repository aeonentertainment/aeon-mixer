/// The crossfade a jump hides under, at 48 kHz: 128 samples, 2.7 ms.
const CROSSFADE_SECONDS: f64 = 128.0 / 48_000.0;
/// Cubic interpolation reads two samples ahead of the read point, and a sample is written before
/// it is read.
const READ_REACH: f64 = 3.0;
/// Below this correlation a jump of one period has not found the waveform again, so the reading
/// may be an octave high and a jump of two periods is tried too.
const LOST_THE_WAVEFORM: f32 = 0.6;

/// The crossfade for this voice and ratio: the longest there is, unless the read point would
/// drift a whole period during it. A high voice shifted far would then have to jump again before
/// the fade was over, and the delay would run away.
fn crossfade_samples(ratio: f64, period: f64, longest: f64) -> f64 {
    let drift = (ratio - 1.0).abs();
    if drift * longest <= period { longest } else { (period / drift).floor() }
}

/// How far behind the newest sample the read point may close: room for its reach, plus what it
/// gains on the input during a jump's crossfade when reading faster than time.
fn floor_samples(ratio: f64, period: f64, longest: f64) -> f64 {
    let crossfade = crossfade_samples(ratio, period, longest);
    READ_REACH + 2.0 + ((ratio - 1.0).max(0.0) * crossfade).ceil()
}

/// How far a jump's correlation may stretch it: an eighth of a period, at least two samples.
fn jump_reach(period: f64) -> f64 {
    (period / 8.0).max(2.0)
}

/// The most the read point trails, in samples, for a voice of this period.
pub fn bound_samples(ratio: f64, period: f64, rate: f64) -> f64 {
    let longest = (rate * CROSSFADE_SECONDS).round();
    floor_samples(ratio, period, longest) + period + jump_reach(period) + 2.0
}

/// Reads the input back at the transposition's rate. When the read point has drifted a period
/// it jumps back or ahead by about one period, the lag the correlation likes best, under a short
/// crossfade. The pitch out is the pitch in times the ratio, whatever the period reads: a period
/// that reads wrong costs a worse splice, not a wrong note. This is `PitchShifter`'s method, with
/// a floor that follows the ratio rather than holding room for the largest shift, and jumps finer
/// than a sample.
pub struct DelayLine {
    ring: Vec<f32>,
    mask: i64,
    written: i64,
    read: f64,
    fade_from: f64,
    fade_left: usize,
    fade_length: usize,
    fade_in: Vec<f32>,
    longest_period: f64,
    /// The last jump's length. A jump of two periods must not read as a period's worth of drift,
    /// or the next jump would undo it.
    last_lag: f64,
}

impl DelayLine {
    /// Holds `reach` samples of history for whoever reads it, on top of its own needs. A jump
    /// never goes further than `longest_period`.
    pub fn new(rate: f64, reach: usize, longest_period: f64) -> Self {
        let crossfade = (rate * CROSSFADE_SECONDS).round() as usize;
        let fade_in = (0..crossfade)
            .map(|i| {
                let phase = (i as f64 + 0.5) / crossfade as f64;
                (0.5 - 0.5 * (std::f64::consts::PI * phase).cos()) as f32
            })
            .collect();
        let size = (reach + 4 * crossfade).next_power_of_two();
        Self {
            ring: vec![0.0; size],
            mask: size as i64 - 1,
            written: 0,
            read: 0.0,
            fade_from: 0.0,
            fade_left: 0,
            fade_length: crossfade,
            fade_in,
            longest_period,
            last_lag: 0.0,
        }
    }

    pub fn write(&mut self, sample: f32) {
        self.ring[(self.written & self.mask) as usize] = sample;
        self.written += 1;
    }

    /// Starts reading at the floor, for a line that has been idle.
    pub fn start(&mut self, ratio: f64, period: f64) {
        self.read = self.written as f64 - floor_samples(ratio, period, self.longest_fade());
        self.fade_left = 0;
        self.last_lag = 0.0;
    }

    /// The next output sample, with the newest input already written.
    pub fn read(&mut self, ratio: f64, period: f64) -> f32 {
        if self.fade_left == 0 {
            self.keep_in_range(ratio, period);
        }
        let mut output = self.sample_at(self.read);
        if self.fade_left > 0 {
            let along = (self.fade_length - self.fade_left) * self.fade_in.len() / self.fade_length;
            let gain = self.fade_in[along];
            output = gain * output + (1.0 - gain) * self.sample_at(self.fade_from);
            // A ratio raised mid-fade must not carry the old read point past the newest sample
            self.fade_from = (self.fade_from + ratio).min(self.written as f64 - READ_REACH);
            self.fade_left -= 1;
        }
        self.read = (self.read + ratio).min(self.written as f64 - READ_REACH);
        output
    }

    /// How far the output trails the input now, in samples.
    pub fn delay(&self) -> f64 {
        self.written as f64 - self.read
    }

    /// The newest `window.len()` samples, oldest first.
    pub fn copy_newest(&self, window: &mut [f32]) {
        let start = self.written - window.len() as i64;
        for (offset, sample) in window.iter_mut().enumerate() {
            *sample = self.at(start + offset as i64);
        }
    }

    pub fn clear(&mut self) {
        self.ring.fill(0.0);
        self.written = 0;
        self.read = 0.0;
        self.fade_left = 0;
        self.last_lag = 0.0;
    }

    fn at(&self, index: i64) -> f32 {
        self.ring[(index & self.mask) as usize]
    }

    /// Catmull-Rom between the two samples around `position`.
    fn sample_at(&self, position: f64) -> f32 {
        let floor = position.floor();
        let i = floor as i64;
        let t = (position - floor) as f32;
        let (y0, y1, y2, y3) = (self.at(i - 1), self.at(i), self.at(i + 1), self.at(i + 2));
        y1 + 0.5
            * t
            * (y2 - y0
                + t * (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3 + t * (3.0 * (y1 - y2) + y3 - y0)))
    }

    fn longest_fade(&self) -> f64 {
        self.fade_in.len() as f64
    }

    fn keep_in_range(&mut self, ratio: f64, period: f64) {
        let floor = floor_samples(ratio, period, self.longest_fade());
        let delay = self.delay();
        let longest_stay = (period + jump_reach(period)).max(self.last_lag);
        let direction = if delay < floor {
            -1
        } else if delay > floor + longest_stay + 1.0 {
            1
        } else {
            return;
        };
        let Some(lag) = self.best_jump(direction, period, floor) else {
            // Two periods would pass the newest sample; stay until they would not
            self.last_lag = 2.0 * period + jump_reach(period);
            return;
        };
        self.last_lag = lag;
        self.fade_from = self.read;
        self.read += direction as f64 * lag;
        self.fade_length = crossfade_samples(ratio, period, self.longest_fade()) as usize;
        self.fade_left = self.fade_length;
    }

    /// The lag near one period that best continues the waveform, or near two when one period
    /// loses it: a reading an octave high lands on a half-period otherwise, and that is the one
    /// reading that would be heard. None when two periods are wanted and the read point is not yet
    /// far enough behind to take them.
    fn best_jump(&self, direction: i64, period: f64, floor: f64) -> Option<f64> {
        // A voice that has just gone up leaves the read point several of its new periods behind
        let periods = if direction > 0 {
            ((self.delay() - floor - jump_reach(period)) / period).floor().max(1.0)
        } else {
            1.0
        };
        let (lag, score) = self.search_near(direction, periods * period, period, floor);
        if score >= LOST_THE_WAVEFORM || 2.0 * period > self.longest_period {
            return Some(lag);
        }
        let furthest = 2.0 * period + jump_reach(period);
        if direction > 0 && self.delay() - furthest < floor {
            return None;
        }
        let (double_lag, double_score) = self.search_near(direction, 2.0 * period, period, floor);
        Some(if double_score >= LOST_THE_WAVEFORM && double_score > score {
            double_lag
        } else {
            lag
        })
    }

    /// The best lag within a jump's reach of `nominal`, for a voice of `period`, and its correlation. The lag is finer than a sample:
    /// a voice's period is not a whole number of samples, and whole-sample jumps add up to an
    /// audible error in pitch at the top of the range, where jumps are short and frequent.
    fn search_near(&self, direction: i64, nominal: f64, period: f64, floor: f64) -> (f64, f32) {
        let nominal = nominal.round() as i64;
        let reach = jump_reach(period) as i64;
        let mut best = (nominal, -2.0);
        for lag in nominal - reach..=nominal + reach {
            let to = self.read.floor() as i64 + direction * lag;
            if direction > 0 && ((self.written - to) as f64) < floor {
                break;
            }
            let score = self.correlation(direction, lag);
            if score > best.1 {
                best = (lag, score);
            }
        }
        let before = self.correlation(direction, best.0 - 1);
        let after = self.correlation(direction, best.0 + 1);
        let curvature = before - 2.0 * best.1 + after;
        let finer = if curvature < 0.0 {
            f64::from((0.5 * (before - after) / curvature).clamp(-0.5, 0.5))
        } else {
            0.0
        };
        (best.0 as f64 + finer, best.1)
    }

    /// How alike the stretch behind the read point and the one behind where a jump would land.
    fn correlation(&self, direction: i64, lag: i64) -> f32 {
        let from = self.read.floor() as i64;
        let to = from + direction * lag;
        let (mut cross, mut energy_from, mut energy_to) = (0.0, 0.0, 0.0);
        for k in 1..=self.fade_in.len() as i64 {
            let (a, b) = (self.at(from - k), self.at(to - k));
            cross += a * b;
            energy_from += a * a;
            energy_to += b * b;
        }
        cross / (energy_from * energy_to + 1e-12_f32).sqrt()
    }
}
