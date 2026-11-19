use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::Duration;

/// Each bucket's width: a block's render on a laptop is a few hundred microseconds.
const BUCKET: Duration = Duration::from_micros(10);
/// 10 ms of buckets; the last holds everything longer.
const BUCKETS: usize = 1000;

/// How long each block's render took, counted by the audio thread with no allocation and read
/// by the control thread.
pub struct RenderTimes {
    counts: [AtomicU32; BUCKETS],
    longest_ns: AtomicU64,
}

impl Default for RenderTimes {
    fn default() -> Self {
        Self { counts: std::array::from_fn(|_| AtomicU32::new(0)), longest_ns: AtomicU64::new(0) }
    }
}

impl RenderTimes {
    pub fn record(&self, took: Duration) {
        let bucket = (took.as_nanos() / BUCKET.as_nanos()) as usize;
        self.counts[bucket.min(BUCKETS - 1)].fetch_add(1, Relaxed);
        self.longest_ns.fetch_max(took.as_nanos() as u64, Relaxed);
    }

    pub fn longest(&self) -> Duration {
        Duration::from_nanos(self.longest_ns.load(Relaxed))
    }

    /// The render time `fraction` of blocks stayed within, to the next bucket edge and never past
    /// the longest. Zero before any block.
    pub fn percentile(&self, fraction: f64) -> Duration {
        let counts: Vec<u64> =
            self.counts.iter().map(|count| u64::from(count.load(Relaxed))).collect();
        let rank = ((fraction * counts.iter().sum::<u64>() as f64).ceil() as u64).max(1);
        let mut seen = 0;
        let Some(bucket) = counts.iter().position(|count| {
            seen += count;
            seen >= rank
        }) else {
            return Duration::ZERO;
        };
        (BUCKET * (bucket as u32 + 1)).min(self.longest())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_percentile_reads_to_the_bucket_and_never_past_the_longest() {
        let times = RenderTimes::default();
        assert_eq!(times.percentile(0.99), Duration::ZERO);
        for _ in 0..99 {
            times.record(Duration::from_micros(101));
        }
        times.record(Duration::from_micros(2_345));
        assert_eq!(times.percentile(0.99), Duration::from_micros(110));
        assert_eq!(times.percentile(1.0), Duration::from_micros(2_345));
        assert_eq!(times.longest(), Duration::from_micros(2_345));
    }
}
