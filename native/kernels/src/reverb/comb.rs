/// Freeverb's lowpass-feedback comb: the tank's decay, damped a little more each pass.
pub struct Comb {
    buffer: Vec<f32>,
    index: usize,
    store: f32,
}

impl Comb {
    pub fn new(length: usize) -> Self {
        Self { buffer: vec![0.0; length], index: 0, store: 0.0 }
    }

    pub fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let output = self.buffer[self.index];
        // A decaying tail ends in denormals, which cost x86 a hundred times a normal float
        self.store = flush_denormal(output * (1.0 - damp) + self.store * damp);
        self.buffer[self.index] = input + self.store * feedback;
        self.index = (self.index + 1) % self.buffer.len();
        output
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
        self.store = 0.0;
    }
}

fn flush_denormal(x: f32) -> f32 {
    if x.abs() < 1e-20 { 0.0 } else { x }
}
