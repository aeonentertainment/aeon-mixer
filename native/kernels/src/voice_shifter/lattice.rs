use super::envelope::{ORDER, Reflections};

/// One all-zero or all-pole lattice, with its coefficients on their way to a target. Both
/// directions share one state: `remove` and `add` are inverses, so a lattice is used for one.
pub struct Lattice {
    reflections: Reflections,
    step: Reflections,
    target: Reflections,
    state: [f32; ORDER + 1],
}

impl Lattice {
    pub fn new() -> Self {
        Self {
            reflections: [0.0; ORDER],
            step: [0.0; ORDER],
            target: [0.0; ORDER],
            state: [0.0; ORDER + 1],
        }
    }

    pub fn glide_to(&mut self, target: &Reflections, steps: usize) {
        self.target = *target;
        for ((step, to), from) in self.step.iter_mut().zip(target).zip(&self.reflections) {
            *step = (to - from) / steps as f32;
        }
    }

    pub fn step(&mut self) {
        for (reflection, step) in self.reflections.iter_mut().zip(&self.step) {
            *reflection += step;
        }
    }

    pub fn land(&mut self) {
        self.reflections = self.target;
        self.step = [0.0; ORDER];
    }

    pub fn is_flat(&self) -> bool {
        self.reflections.iter().all(|k| *k == 0.0) && self.target.iter().all(|k| *k == 0.0)
    }

    /// The all-zero direction: the model's envelope comes off.
    pub fn remove(&mut self, input: f32) -> f32 {
        let mut forward = input;
        let mut delayed_in = input;
        for (reflection, delayed) in self.reflections.iter().zip(&mut self.state) {
            let backward = *delayed + reflection * forward;
            forward += reflection * *delayed;
            *delayed = delayed_in;
            delayed_in = backward;
        }
        forward
    }

    /// The all-pole direction: the model's envelope goes on.
    pub fn add(&mut self, input: f32) -> f32 {
        let mut forward = input;
        for (i, reflection) in self.reflections.iter().enumerate().rev() {
            let delayed = self.state[i];
            forward -= reflection * delayed;
            // A resonance decaying through silence ends in denormals
            self.state[i + 1] = flush_denormal(delayed + reflection * forward);
        }
        self.state[0] = flush_denormal(forward);
        forward
    }
}

fn flush_denormal(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}
