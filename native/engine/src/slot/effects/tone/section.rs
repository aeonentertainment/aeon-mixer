use crate::slot::effects::tone::biquad::Biquad;

/// How near a flat's output must be to its input before the section rests: far under the step an
/// f32 sample can show at any level a voice reaches.
const SETTLED: f64 = 1e-7;

/// One biquad on the voice, in direct form I. Its state is the signal itself, the last two samples
/// in and out, so its coefficients can move under it every sample: a change of bands glides
/// across a block, and the de-esser's shelf follows its cut, with no transient of the filter's own.
pub struct Section {
    now: Biquad,
    input: [f64; 2],
    output: [f64; 2],
}

impl Section {
    pub fn new() -> Self {
        Self { now: Biquad::IDENTITY, input: [0.0; 2], output: [0.0; 2] }
    }

    /// One run in place, gliding from where the last ended to `to` across it. At the identity it
    /// touches no sample, and keeps the run's last two as its state, which is what an identity's
    /// state is. A band arriving or leaving glides from or to its own flat, the same poles with
    /// zeros on them, where a glide straight to the identity runs through filters like neither end
    /// and steps the voice. A flat is unity, but its state still rings with the band it was, so a
    /// leaving band runs as its flat until that ring is under `SETTLED`, and only then rests.
    pub fn process(&mut self, to: Biquad, run: &mut [f32]) {
        if self.now == Biquad::IDENTITY && to == Biquad::IDENTITY {
            self.rest(run);
            return;
        }
        if self.now == to {
            for sample in run {
                *sample = self.tick(&to, f64::from(*sample)) as f32;
            }
            return;
        }
        let from = if self.now == Biquad::IDENTITY { to.flat() } else { self.now };
        let toward = if to == Biquad::IDENTITY { from.flat() } else { to };
        let frames = run.len() as f64;
        for (i, sample) in run.iter_mut().enumerate() {
            let at = from.toward(&toward, (i + 1) as f64 / frames);
            *sample = self.tick(&at, f64::from(*sample)) as f32;
        }
        let ringing = self.output.iter().zip(&self.input).any(|(y, x)| (y - x).abs() > SETTLED);
        if to != Biquad::IDENTITY || !ringing {
            self.now = to;
        }
    }

    /// One sample through `at`, for a part whose coefficients move every sample.
    pub fn tick(&mut self, at: &Biquad, x: f64) -> f64 {
        self.now = *at;
        let [b0, b1, b2] = at.b;
        let [a1, a2] = at.a;
        let y = b0 * x + b1 * self.input[0] + b2 * self.input[1]
            - a1 * self.output[0]
            - a2 * self.output[1];
        self.input = [x, self.input[0]];
        self.output = [y, self.output[0]];
        y
    }

    /// Passes `run` untouched and keeps its end as the state.
    pub fn rest(&mut self, run: &[f32]) {
        self.now = Biquad::IDENTITY;
        self.input = match *run {
            [.., before, last] => [f64::from(last), f64::from(before)],
            [last] => [f64::from(last), self.input[0]],
            [] => self.input,
        };
        self.output = self.input;
    }

    /// `rest` for one sample.
    pub fn pass(&mut self, x: f64) {
        self.now = Biquad::IDENTITY;
        self.input = [x, self.input[0]];
        self.output = self.input;
    }
}

impl Default for Section {
    fn default() -> Self {
        Self::new()
    }
}
