use engine::slot::source::Source;

/// A clip the slot plays once, then silence. It says it has no latency: the guide clock reads the
/// clip's own time.
pub struct ClipSource {
    samples: Vec<f32>,
    at: usize,
}

impl ClipSource {
    pub fn new(samples: Vec<f32>) -> Self {
        Self { samples, at: 0 }
    }
}

impl Source for ClipSource {
    fn pull(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.samples.get(self.at).copied().unwrap_or(0.0);
            self.at += 1;
        }
    }
}
