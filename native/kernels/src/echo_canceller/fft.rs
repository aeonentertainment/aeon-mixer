use std::sync::Arc;

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use super::{BINS, WINDOW};

/// One window's spectrum: DC to Nyquist.
pub type Spectrum = [Complex32; BINS];

pub const SILENT: Spectrum = [Complex32::new(0.0, 0.0); BINS];

/// The real FFT of one window, planned on the control thread. The forward transform scales by
/// 1 / WINDOW and the inverse does not, the convention mdf.c's arithmetic assumes.
pub struct Fft {
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    time: [f32; WINDOW],
    frequency: Spectrum,
    scratch: Vec<Complex32>,
}

impl Fft {
    pub fn new() -> Self {
        let mut planner = RealFftPlanner::new();
        let forward = planner.plan_fft_forward(WINDOW);
        let inverse = planner.plan_fft_inverse(WINDOW);
        let scratch_len = forward.get_scratch_len().max(inverse.get_scratch_len());
        Self {
            forward,
            inverse,
            time: [0.0; WINDOW],
            frequency: SILENT,
            scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
        }
    }

    pub fn forward(&mut self, input: &[f32; WINDOW], output: &mut Spectrum) {
        self.time = *input;
        // Lengths are fixed at construction, so the transform cannot fail
        let _ = self.forward.process_with_scratch(&mut self.time, output, &mut self.scratch);
        let scale = 1.0 / WINDOW as f32;
        output.iter_mut().for_each(|bin| *bin *= scale);
    }

    pub fn inverse(&mut self, input: &Spectrum, output: &mut [f32; WINDOW]) {
        self.frequency = *input;
        // A real signal's DC and Nyquist bins are real; rounding must not make them otherwise
        self.frequency[0].im = 0.0;
        self.frequency[BINS - 1].im = 0.0;
        let _ = self.inverse.process_with_scratch(&mut self.frequency, output, &mut self.scratch);
    }
}
