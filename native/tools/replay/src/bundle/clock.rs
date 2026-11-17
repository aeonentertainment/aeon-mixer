//! The bundle's two clocks tied together (docs/design.md 8.11, decision 0026). Every line holds
//! a pair, the output frames rendered and the wall-clock microseconds, read in one call; a
//! packet holds only its microseconds. Between two pairs a packet's frame is where it falls on
//! the line between them, which follows the output's drift against the wall clock. Outside them
//! it runs at the nominal rate from the nearest pair.

const FRAMES_PER_US: f64 = 0.048;

pub struct Clock {
    /// `(us, frame)`, sorted by `us`.
    pairs: Vec<(f64, f64)>,
}

impl Clock {
    pub fn new(mut pairs: Vec<(f64, f64)>) -> Self {
        pairs.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        Self { pairs }
    }

    /// The output frame at wall-clock `us`, to the nearest frame. A frame is 20.8 µs, so a time
    /// truncated to the microsecond comes back to the frame it was taken on.
    pub fn frame_at(&self, us: f64) -> i64 {
        let after = self.pairs.partition_point(|&(at, _)| at <= us);
        let frame =
            match (after.checked_sub(1).and_then(|i| self.pairs.get(i)), self.pairs.get(after)) {
                (Some(&(u0, f0)), Some(&(u1, f1))) if u1 > u0 => {
                    f0 + (us - u0) * (f1 - f0) / (u1 - u0)
                }
                (Some(&(u0, f0)), _) | (None, Some(&(u0, f0))) => f0 + (us - u0) * FRAMES_PER_US,
                (None, None) => 0.0,
            };
        frame.round() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_between_pairs_follows_the_output_and_past_them_the_nominal_rate() {
        // The output runs 1 % fast against the wall clock between the two pairs
        let clock = Clock::new(vec![(2_000_000.0, 48_480.0), (1_000_000.0, 0.0)]);
        assert_eq!(clock.frame_at(1_500_000.0), 24_240);
        assert_eq!(clock.frame_at(900_000.0), -4_800);
        assert_eq!(clock.frame_at(3_000_000.0), 48_480 + 48_000);
    }
}
