/// What the room has been played: the master, folded to mono, newest last. The master pushes
/// every block it sends to the device, and every slot's canceller reads it at the delays it
/// needs. One per graph, built on the control thread; `push` and `read` allocate nothing.
pub struct EchoHistory {
    samples: Vec<f32>,
    /// Frames ever pushed. The next frame the master renders is this one.
    pushed: u64,
}

/// 1.37 s at 48 kHz: the longest echo path the canceller searches, the longest delay a slot
/// can report and its filter's tail, with room to spare.
const CAPACITY: usize = 1 << 16;

impl EchoHistory {
    pub fn new() -> Self {
        Self { samples: vec![0.0; CAPACITY], pushed: 0 }
    }

    /// One block of the stereo master, as the device gets it.
    pub fn push(&mut self, left: &[f32], right: &[f32]) {
        for (l, r) in left.iter().zip(right) {
            self.samples[self.pushed as usize % CAPACITY] = 0.5 * (l + r);
            self.pushed += 1;
        }
    }

    /// The `out.len()` frames that end `lag` frames before the next frame to be rendered:
    /// `read(0, out)` is the newest. Frames from before the first push, or older than the
    /// history holds, read as silence.
    pub fn read(&self, lag: usize, out: &mut [f32]) {
        let end = self.pushed as i64 - lag as i64;
        let start = end - out.len() as i64;
        let oldest = self.pushed as i64 - CAPACITY as i64;
        for (offset, sample) in out.iter_mut().enumerate() {
            let at = start + offset as i64;
            *sample = if at < 0 || at < oldest || at >= self.pushed as i64 {
                0.0
            } else {
                self.samples[at as usize % CAPACITY]
            };
        }
    }

    /// The longest `lag + len` a read can reach and still find what was played.
    pub const fn reach() -> usize {
        CAPACITY
    }
}

impl Default for EchoHistory {
    fn default() -> Self {
        Self::new()
    }
}
