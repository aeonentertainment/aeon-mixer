/// Runs a trail keeps. A run is at most a block and usually one or two make a block, so this
/// reaches back past a reading's centre, 21 ms before the reading, unless a guide puts more than
/// twenty note edges in that time.
const RUNS: usize = 32;

/// The transposition each of the corrector's recent runs came out with, by the input sample it
/// started on, so a reading can say what the voice at its centre came out at. Fixed in size,
/// so the audio thread allocates nothing to keep it.
pub struct ShiftTrail {
    /// Each run's first sample and its shift, oldest overwritten first.
    runs: [(u64, f32); RUNS],
    next: usize,
}

impl ShiftTrail {
    pub fn new() -> Self {
        Self { runs: [(0, 0.0); RUNS], next: 0 }
    }

    /// A run that started on input sample `first` came out shifted by `semitones`.
    pub fn push(&mut self, first: u64, semitones: f32) {
        self.runs[self.next] = (first, semitones);
        self.next = (self.next + 1) % RUNS;
    }

    /// The shift input sample `sample` came out with, or the oldest the trail holds when it
    /// does not reach back that far.
    pub fn at(&self, sample: u64) -> f32 {
        let newest_first = (1..=RUNS).map(|back| self.runs[(self.next + RUNS - back) % RUNS]);
        let mut oldest = 0.0;
        for (first, semitones) in newest_first {
            if first <= sample {
                return semitones;
            }
            oldest = semitones;
        }
        oldest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sample_takes_the_shift_of_the_run_it_was_in() {
        let mut trail = ShiftTrail::new();
        for (run, first) in [0u64, 100, 228, 300].into_iter().enumerate() {
            trail.push(first, run as f32);
        }
        assert_eq!(
            [0, 99, 100, 250, 300, 5_000].map(|s| trail.at(s)),
            [0.0, 0.0, 1.0, 2.0, 3.0, 3.0]
        );
    }

    #[test]
    fn a_sample_older_than_the_trail_takes_its_oldest_shift() {
        let mut trail = ShiftTrail::new();
        for run in 0..RUNS as u64 + 4 {
            trail.push(1_000 + run * 10, run as f32);
        }
        assert_eq!(trail.at(0), 4.0);
    }
}
