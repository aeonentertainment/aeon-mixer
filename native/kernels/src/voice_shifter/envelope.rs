/// Poles in the model. Speech's rule of thumb is one pair per kHz of band, which at 48 kHz is the
/// order that resolves a vocal tract's formants. Decision 0004 has the sweep.
pub const ORDER: usize = 48;
/// A window of about this many periods holds the voice's spectrum steady from one reading to
/// the next, without reaching further back than the note needs.
const WINDOW_PERIODS: f64 = 3.5;
const LONGEST_WINDOW_SECONDS: f64 = 2048.0 / 48_000.0;
const SHORTEST_WINDOW_SECONDS: f64 = 768.0 / 48_000.0;
/// Below this mean square in the window (-90 dBFS, after the first difference) there is no
/// envelope to move.
const SILENCE: f64 = 1e-9;
/// A floor under the model, -20 dB against the voice. The valleys of a real voice are breath and
/// room, and a model free to follow them down has a filter that lifts them by 20 dB or more. This
/// is the setting that takes a singer's periodicity back to where it came in: decision 0004 has
/// the sweep.
const NOISE_FLOOR: f64 = 1e-2;
/// The lag window's width in periods. Only lags well under a period carry the envelope: a longer
/// reach sees the harmonics, and the model then rings on them, which is LPC's known failure on a
/// high voice.
const LAG_WINDOW_PERIODS: f64 = 0.7;
/// A voice at 48 kHz is nearly all under 1 kHz, so the samples alike from one to the next and the
/// first reflection close to 1: at that point Levinson-Durbin has nothing left to divide by. The
/// usual first difference flattens it, and what it leaves behind is a tilt under 250 Hz a level
/// match takes out of the rest.
const PRE_EMPHASIS: f32 = 0.97;
/// How much of the last reading each reading keeps, a time constant of about three hops. One
/// click or lost packet in a short window flattens that window's spectrum, and without this the
/// filter would swing with it.
const KEEP_LAST_READING: f64 = 0.7;
const STABLE: f64 = 0.9995;
const MOST_LAGS: usize = 2 * ORDER + 4;

/// Eight running sums rather than one, because a compiler may not reorder a float sum and so will
/// not vectorise one: this is the difference between the envelope costing a quarter and a whole
/// percent of a core.
fn dot(a: &[f32], b: &[f32]) -> f32 {
    let mut lanes = [0.0; 8];
    let (a_chunks, a_rest) = a.as_chunks::<8>();
    let (b_chunks, b_rest) = b.as_chunks::<8>();
    for (a, b) in a_chunks.iter().zip(b_chunks) {
        for ((lane, x), y) in lanes.iter_mut().zip(a).zip(b) {
            *lane += x * y;
        }
    }
    let rest: f32 = a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum();
    lanes.iter().sum::<f32>() + rest
}

/// A set of reflection coefficients: the model in the form that stays stable while it is
/// interpolated.
pub type Reflections = [f32; ORDER];

/// Reads the voice's spectral envelope as an autocorrelation, and states it again as if the voice
/// had been resampled. Scaling a spectrum's frequency axis by `factor` is reading its
/// autocorrelation at `factor` times the lag, so moving an envelope costs one interpolation per
/// lag and no transform.
pub struct Envelope {
    hann: Vec<f32>,
    autocorrelation: [f64; MOST_LAGS],
    /// The running reading, before the lag window: a window that follows the pitch goes on after.
    held: [f64; MOST_LAGS],
    shortest: usize,
    longest: usize,
}

impl Envelope {
    pub fn new(rate: f64) -> Self {
        let longest = (rate * LONGEST_WINDOW_SECONDS).round() as usize;
        let hann = (0..=longest)
            .map(|i| {
                let phase = i as f64 / longest as f64;
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * phase).cos()) as f32
            })
            .collect();
        Self {
            hann,
            autocorrelation: [0.0; MOST_LAGS],
            held: [0.0; MOST_LAGS],
            shortest: (rate * SHORTEST_WINDOW_SECONDS).round() as usize,
            longest,
        }
    }

    /// The samples to hand `measure` for a voice of this period.
    pub fn window_samples(&self, period: f64) -> usize {
        ((WINDOW_PERIODS * period) as usize).clamp(self.shortest, self.longest)
    }

    pub fn longest_window(&self) -> usize {
        self.longest
    }

    /// Reads `samples`, oldest first and `window_samples` long, windowing them in place. The lags
    /// measured reach `ORDER` times the largest factor about to be asked of `model_at`. False in
    /// silence, when there is no model to take.
    pub fn measure(&mut self, samples: &mut [f32], period: f64, largest_factor: f64) -> bool {
        let count = samples.len();
        let mut previous = 0.0;
        for (i, sample) in samples.iter_mut().enumerate() {
            let emphasised = *sample - PRE_EMPHASIS * previous;
            previous = *sample;
            *sample = emphasised * self.hann[i * self.longest / count.max(1)];
        }
        let lags = ((ORDER as f64 * largest_factor).ceil() as usize + 4).min(MOST_LAGS);
        for lag in 0..lags {
            self.autocorrelation[lag] = f64::from(dot(&samples[..count - lag], &samples[lag..]));
        }
        // Hann's mean square is 3/8. The first difference of a voice is well under the voice, so
        // this silence is measured in a tilted spectrum, and is quieter than it reads.
        let power = self.autocorrelation[0] / (0.375 * count as f64);
        if power < SILENCE {
            self.held = [0.0; MOST_LAGS];
            return false;
        }
        // Per sample, so a reading from a long window weighs no more than one from a short one
        let scale = 1.0 / count as f64;
        let width = LAG_WINDOW_PERIODS * period;
        for (lag, (value, held)) in
            self.autocorrelation[..lags].iter_mut().zip(&mut self.held).enumerate()
        {
            *held = KEEP_LAST_READING * *held + (1.0 - KEEP_LAST_READING) * scale * *value;
            let at = lag as f64 / width;
            *value = *held * (-0.5 * at * at).exp();
        }
        self.autocorrelation[lags..].fill(0.0);
        true
    }

    /// The model of the measured envelope with its frequency axis scaled by `factor`, from
    /// Levinson-Durbin. It stops at the first coefficient that would not be stable, which leaves
    /// a lower-order model that is.
    pub fn model_at(&self, factor: f64) -> Reflections {
        let mut warped = [0.0; ORDER + 1];
        for (lag, value) in warped.iter_mut().enumerate() {
            *value = self.autocorrelation_at(lag as f64 * factor);
        }
        warped[0] *= 1.0 + NOISE_FLOOR;

        let mut reflections = [0.0; ORDER];
        let mut prediction = [0.0; ORDER + 1];
        let mut previous = [0.0; ORDER + 1];
        prediction[0] = 1.0;
        let mut error = warped[0];
        for order in 1..=ORDER {
            let mut sum = warped[order];
            for j in 1..order {
                sum += prediction[j] * warped[order - j];
            }
            let reflection = -sum / error;
            if reflection.abs() >= STABLE || reflection.is_nan() {
                break;
            }
            reflections[order - 1] = reflection as f32;
            previous[..order].copy_from_slice(&prediction[..order]);
            for j in 1..order {
                prediction[j] = previous[j] + reflection * previous[order - j];
            }
            prediction[order] = reflection;
            error *= 1.0 - reflection * reflection;
        }
        reflections
    }

    /// Catmull-Rom over the measured lags; the autocorrelation is even, so lag -1 is lag 1.
    fn autocorrelation_at(&self, lag: f64) -> f64 {
        let floor = lag.floor();
        let i = (floor as usize).min(MOST_LAGS - 3);
        let t = lag - floor;
        let y0 = self.autocorrelation[i.abs_diff(1)];
        let (y1, y2, y3) =
            (self.autocorrelation[i], self.autocorrelation[i + 1], self.autocorrelation[i + 2]);
        y1 + 0.5
            * t
            * (y2 - y0
                + t * (2.0 * y0 - 5.0 * y1 + 4.0 * y2 - y3 + t * (3.0 * (y1 - y2) + y3 - y0)))
    }
}
