use super::envelope::Reflections;
use super::lattice::Lattice;

/// How often the coefficients take a step towards their target.
const STEP_SAMPLES: usize = 16;
/// How long the level match looks back. Longer than a low voice's period, so it does not pump on
/// the glottal pulses, and short enough to catch a consonant's change of spectrum.
const LEVEL_MEMORY_SECONDS: f64 = 0.020;
const QUIETEST_GAIN: f32 = 0.25;
const LOUDEST_GAIN: f32 = 4.0;
/// Under this power the level match goes back to unity rather than dividing noise by noise, so
/// a voice coming back from silence never starts at the gain the silence left.
const NO_LEVEL: f32 = 1e-8;
/// How far over the input's recent peak the output may go, +24 dB, before the filter is called
/// broken and starts again from flat. A moved formant can lift one harmonic a long way, and the
/// level match then brings it back; this is for what the level match would be too slow to save
/// the room from.
const BROKEN: f32 = 16.0;
/// How fast the input's peak falls, per sample: about a 50 ms time constant at 48 kHz.
const PEAK_KEEP: f32 = 0.9996;
/// Quiet tails after the input stops are not a burst.
const NOT_BROKEN_BELOW: f32 = 1e-3;

/// Takes the envelope the delay line left off the voice and puts on the one asked for, sample by
/// sample with no delay: a lattice whitens with the first model, a lattice colours with the
/// second. A lattice stays stable under any coefficients inside the unit interval, and a straight
/// line between two such sets stays inside it, so a target can change at any moment without a
/// click. With both models the same the two lattices cancel to the input.
///
/// Two models fitted separately do not agree on a level, so the output's is matched to the
/// input's by measuring both. A singer who turns brighter does not turn louder.
pub struct FormantFilter {
    whiten: Lattice,
    colour: Lattice,
    steps_left: usize,
    until_step: usize,
    input_power: f32,
    output_power: f32,
    power_keep: f32,
    input_peak: f32,
    gain: f32,
    gain_step: f32,
}

impl FormantFilter {
    pub fn new(rate: f64) -> Self {
        Self {
            whiten: Lattice::new(),
            colour: Lattice::new(),
            steps_left: 0,
            until_step: 0,
            input_power: 0.0,
            output_power: 0.0,
            power_keep: (-1.0 / (rate * LEVEL_MEMORY_SECONDS)).exp() as f32,
            input_peak: 0.0,
            gain: 1.0,
            gain_step: 0.0,
        }
    }

    /// A new target for each model, arriving over `samples`.
    pub fn glide_to(&mut self, whiten: &Reflections, colour: &Reflections, samples: usize) {
        // Asked to do nothing while already doing nothing: stay transparent for the caller to skip
        if self.is_transparent() && whiten == colour && whiten.iter().all(|k| *k == 0.0) {
            return;
        }
        let steps = (samples / STEP_SAMPLES).max(1);
        self.whiten.glide_to(whiten, steps);
        self.colour.glide_to(colour, steps);
        self.steps_left = steps;
    }

    /// True once the filter has settled on doing nothing, so the caller can skip it.
    pub fn is_transparent(&self) -> bool {
        self.steps_left == 0 && self.whiten.is_flat() && self.colour.is_flat()
    }

    /// Forgets the voice. A filter at rest has nothing to fade.
    pub fn clear(&mut self) {
        self.whiten = Lattice::new();
        self.colour = Lattice::new();
        self.steps_left = 0;
        self.until_step = 0;
        self.input_power = 0.0;
        self.output_power = 0.0;
        self.input_peak = 0.0;
        self.gain = 1.0;
        self.gain_step = 0.0;
    }

    pub fn process(&mut self, input: f32) -> f32 {
        if self.until_step == 0 {
            self.until_step = STEP_SAMPLES;
            self.step();
        }
        self.until_step -= 1;

        let residual = self.whiten.remove(input);
        let output = self.colour.add(residual);
        self.input_peak = (self.input_peak * PEAK_KEEP).max(input.abs());
        self.input_power =
            self.power_keep * self.input_power + (1.0 - self.power_keep) * input * input;
        self.output_power =
            self.power_keep * self.output_power + (1.0 - self.power_keep) * output * output;
        self.gain += self.gain_step;
        let levelled = output * self.gain;
        // Infinity or NaN would stay in the lattices for good, and a burst would reach the room
        if levelled.is_nan() || levelled.abs() > BROKEN * self.input_peak + NOT_BROKEN_BELOW {
            self.clear();
            return if input.is_finite() { input } else { 0.0 };
        }
        levelled
    }

    fn step(&mut self) {
        if self.steps_left > 0 {
            self.whiten.step();
            self.colour.step();
            self.steps_left -= 1;
            if self.steps_left == 0 {
                self.whiten.land();
                self.colour.land();
            }
        }
        // An idle filter must come out exactly at unity, or the bypass would step
        if self.is_transparent() {
            self.gain = 1.0;
            self.gain_step = 0.0;
            return;
        }
        let target = if self.input_power < NO_LEVEL || self.output_power < NO_LEVEL {
            1.0
        } else {
            (self.input_power / self.output_power).sqrt().clamp(QUIETEST_GAIN, LOUDEST_GAIN)
        };
        self.gain_step = (target - self.gain) / STEP_SAMPLES as f32;
    }
}
