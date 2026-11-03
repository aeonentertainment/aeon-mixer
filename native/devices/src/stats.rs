use std::sync::OnceLock;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64};
use std::time::{Duration, Instant};

use crate::report::Report;

/// The width of one bucket of the callback interval histogram.
pub const BUCKET: Duration = Duration::from_micros(50);

/// Buckets in the histogram. The last one holds every interval of 20 ms or more.
pub const BUCKETS: usize = 400;

const NEVER: u64 = u64::MAX;

/// What a running stream records about itself: its callbacks and their spacing, its underruns,
/// whether it failed. A backend writes it from the audio thread with relaxed atomics and no
/// allocation. Whoever holds the other `Arc` reads a `Report` whenever it likes: measure-sink
/// prints one, and the binding can turn underruns and failure into engine events.
pub struct Stats {
    epoch: Instant,
    last_ns: AtomicU64,
    callbacks: AtomicU64,
    underruns: AtomicU64,
    intervals: [AtomicU32; BUCKETS],
    shortest_ns: AtomicU64,
    longest_ns: AtomicU64,
    realtime: AtomicBool,
    failed: AtomicBool,
    opened: OnceLock<String>,
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

impl Stats {
    pub fn new() -> Self {
        Self {
            epoch: Instant::now(),
            last_ns: AtomicU64::new(NEVER),
            callbacks: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            intervals: [const { AtomicU32::new(0) }; BUCKETS],
            shortest_ns: AtomicU64::new(NEVER),
            longest_ns: AtomicU64::new(0),
            realtime: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            opened: OnceLock::new(),
        }
    }

    /// Marks one device callback, called first thing in it. Returns the time since the last one.
    /// Its clock read stays in userspace: the vDSO on Linux, mach_absolute_time on macOS and
    /// QueryPerformanceCounter on Windows.
    pub fn callback(&self) -> Option<Duration> {
        let now = self.epoch.elapsed().as_nanos() as u64;
        self.callbacks.fetch_add(1, Relaxed);
        let last = self.last_ns.swap(now, Relaxed);
        (last != NEVER).then(|| {
            self.record(now - last);
            Duration::from_nanos(now - last)
        })
    }

    pub fn underrun(&self) {
        self.underruns.fetch_add(1, Relaxed);
    }

    /// The device is gone or render panicked. The stream plays silence or nothing from here.
    pub fn fail(&self) {
        self.failed.store(true, Relaxed);
    }

    /// Whether the audio thread got the OS's realtime scheduling, set once at start.
    pub fn set_realtime(&self, realtime: bool) {
        self.realtime.store(realtime, Relaxed);
    }

    /// What the backend opened, in words, set once at start on the control thread.
    pub fn set_opened(&self, description: String) {
        let _ = self.opened.set(description);
    }

    pub fn opened(&self) -> Option<&str> {
        self.opened.get().map(String::as_str)
    }

    pub fn report(&self) -> Report {
        let nanos = |ns: u64| (ns != NEVER && ns != 0).then(|| Duration::from_nanos(ns));
        Report {
            callbacks: self.callbacks.load(Relaxed),
            underruns: self.underruns.load(Relaxed),
            realtime: self.realtime.load(Relaxed),
            failed: self.failed.load(Relaxed),
            shortest: nanos(self.shortest_ns.load(Relaxed)),
            longest: nanos(self.longest_ns.load(Relaxed)),
            intervals: self.intervals.each_ref().map(|count| count.load(Relaxed)),
        }
    }

    fn record(&self, interval_ns: u64) {
        let bucket = (interval_ns / BUCKET.as_nanos() as u64).min(BUCKETS as u64 - 1);
        self.intervals[bucket as usize].fetch_add(1, Relaxed);
        self.shortest_ns.fetch_min(interval_ns, Relaxed);
        self.longest_ns.fetch_max(interval_ns, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    const MS: u64 = 1_000_000;

    #[test]
    fn callbacks_record_without_allocating() {
        let stats = Stats::new();
        let (first, second) = assert_no_alloc(|| (stats.callback(), stats.callback()));
        assert_eq!(first, None);
        assert!(second.is_some());
        assert_eq!(stats.report().callbacks, 2);
    }

    #[test]
    fn the_report_reads_the_distribution() {
        let stats = Stats::new();
        // A 128-frame period, ten callbacks a period late, one 30 ms stall
        for _ in 0..1000 {
            stats.record(2_667_000);
        }
        for _ in 0..10 {
            stats.record(5 * MS);
        }
        stats.record(30 * MS);
        let report = stats.report();
        let ms = |ms: f64| Some(Duration::from_secs_f64(ms / 1000.0));

        assert_eq!(report.percentile(0.5), ms(2.70));
        assert_eq!(report.percentile(0.999), ms(5.05));
        assert_eq!(report.percentile(1.0), ms(30.0));
        assert_eq!(report.at_least(Duration::from_millis(4)), 11);
        assert_eq!(report.shortest, ms(2.667));
        assert_eq!(report.longest, ms(30.0));
    }
}
