use std::f32::consts::PI;

/// A fourth-order Butterworth low-pass, two biquads in series, run before the delay finder
/// takes one sample in `DECIMATION`. The mic and the reference each get one with the same
/// coefficients, so its delay is the same on both and leaves their lag where it was.
pub struct Lowpass {
    sections: [Biquad; 2],
}

#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 2],
    z: [f32; 2],
}

impl Lowpass {
    pub fn new(rate: u32, cutoff_hz: f32) -> Self {
        // A fourth-order Butterworth's two pole pairs
        let qs = [0.541_196_1, 1.306_563];
        Self { sections: qs.map(|q| Biquad::low_pass(rate, cutoff_hz, q)) }
    }

    pub fn process(&mut self, sample: f32) -> f32 {
        self.sections.iter_mut().fold(sample, |x, section| section.process(x))
    }

    pub fn reset(&mut self) {
        self.sections.iter_mut().for_each(|section| section.z = [0.0; 2]);
    }
}

impl Biquad {
    /// The RBJ cookbook's low-pass.
    fn low_pass(rate: u32, cutoff_hz: f32, q: f32) -> Self {
        let w = 2.0 * PI * cutoff_hz / rate as f32;
        let alpha = w.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        let b1 = (1.0 - w.cos()) / a0;
        Self {
            b: [b1 / 2.0, b1, b1 / 2.0],
            a: [-2.0 * w.cos() / a0, (1.0 - alpha) / a0],
            z: [0.0; 2],
        }
    }

    /// Transposed direct form II.
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}
