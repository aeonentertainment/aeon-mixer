/// Freeverb's Schroeder allpass: diffuses the combs' echoes into a wash without colouring it.
pub struct Allpass {
    buffer: Vec<f32>,
    index: usize,
}

const FEEDBACK: f32 = 0.5;

impl Allpass {
    pub fn new(length: usize) -> Self {
        Self { buffer: vec![0.0; length], index: 0 }
    }

    pub fn process(&mut self, input: f32) -> f32 {
        let delayed = self.buffer[self.index];
        self.buffer[self.index] = input + delayed * FEEDBACK;
        self.index = (self.index + 1) % self.buffer.len();
        delayed - input
    }

    pub fn clear(&mut self) {
        self.buffer.fill(0.0);
    }
}
