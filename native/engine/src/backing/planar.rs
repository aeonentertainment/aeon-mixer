/// One stem's stereo audio at 48 kHz, a vector per side, both the same length.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Planar {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

impl Planar {
    pub fn silence(frames: usize) -> Self {
        Self { left: vec![0.0; frames], right: vec![0.0; frames] }
    }

    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// The left and right samples at `frame`, silence past the end. Allocation-free.
    pub fn at(&self, frame: usize) -> [f32; 2] {
        match (self.left.get(frame), self.right.get(frame)) {
            (Some(left), Some(right)) => [*left, *right],
            _ => [0.0, 0.0],
        }
    }

    /// Pads the end with silence up to `frames`; never shortens.
    pub fn pad_to(&mut self, frames: usize) {
        if frames > self.frames() {
            self.left.resize(frames, 0.0);
            self.right.resize(frames, 0.0);
        }
    }
}
