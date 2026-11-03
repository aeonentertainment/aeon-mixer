use std::time::Duration;

use crate::stats::{BUCKET, BUCKETS};

/// A stream's `Stats` read at one moment, on the control thread.
pub struct Report {
    pub callbacks: u64,
    pub underruns: u64,
    pub realtime: bool,
    pub failed: bool,
    pub shortest: Option<Duration>,
    pub longest: Option<Duration>,
    pub(crate) intervals: [u32; BUCKETS],
}

impl Report {
    /// The callback interval that `fraction` of intervals stay within, to the next bucket edge.
    pub fn percentile(&self, fraction: f64) -> Option<Duration> {
        let total: u64 = self.intervals.iter().map(|count| u64::from(*count)).sum();
        let rank = ((fraction * total as f64).ceil() as u64).max(1);
        let mut seen = 0;
        let bucket = self.intervals.iter().position(|count| {
            seen += u64::from(*count);
            seen >= rank
        })?;
        let edge = BUCKET * (bucket as u32 + 1);
        Some(if bucket == BUCKETS - 1 { self.longest? } else { edge.min(self.longest?) })
    }

    /// Callback intervals of `threshold` or more, to the bucket.
    pub fn at_least(&self, threshold: Duration) -> u64 {
        let first = threshold.as_nanos().div_ceil(BUCKET.as_nanos()) as usize;
        self.intervals.iter().skip(first).map(|count| u64::from(*count)).sum()
    }
}
