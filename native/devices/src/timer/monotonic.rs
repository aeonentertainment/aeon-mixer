use std::time::Duration;

use super::clock::Clock;
#[cfg(target_os = "macos")]
use super::mach::Timebase;

/// The OS's monotonic clock and an absolute wait on it, so a late wake never moves a later
/// deadline. Reading it stays in userspace: the vDSO on Linux, the commpage on macOS,
/// QueryPerformanceCounter on Windows.
pub struct MonotonicClock {
    #[cfg(target_os = "macos")]
    timebase: Timebase,
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    epoch: std::time::Instant,
}

impl Default for MonotonicClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl MonotonicClock {
    /// What the clock is, in words, for `Stats::opened`.
    pub const NAME: &str = "CLOCK_MONOTONIC, clock_nanosleep to an absolute deadline";

    pub fn new() -> Self {
        Self {}
    }
}

#[cfg(target_os = "linux")]
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        let mut time = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: writes the timespec it is handed; CLOCK_MONOTONIC always exists.
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) };
        Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
    }

    fn sleep_until(&mut self, deadline: Duration) {
        let time = libc::timespec {
            tv_sec: deadline.as_secs() as libc::time_t,
            tv_nsec: deadline.subsec_nanos() as _,
        };
        // SAFETY: reads the timespec; with TIMER_ABSTIME no remainder is written, so it may be
        // null. A signal returns EINTR early, which the pacer treats as nothing due.
        unsafe {
            libc::clock_nanosleep(
                libc::CLOCK_MONOTONIC,
                libc::TIMER_ABSTIME,
                &time,
                std::ptr::null_mut(),
            )
        };
    }
}

#[cfg(target_os = "macos")]
impl MonotonicClock {
    pub const NAME: &str = "mach_absolute_time, mach_wait_until an absolute deadline";

    pub fn new() -> Self {
        Self { timebase: Timebase::get() }
    }
}

#[cfg(target_os = "macos")]
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.timebase.now()
    }

    fn sleep_until(&mut self, deadline: Duration) {
        self.timebase.wait_until(deadline);
    }
}

// Windows and the rest: std's sleep, from the deadline less now. On Windows it is a
// high-resolution waitable timer, about half a millisecond fine, since Rust 1.75.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl MonotonicClock {
    pub const NAME: &str = "std Instant, a sleep to the deadline";

    pub fn new() -> Self {
        Self { epoch: std::time::Instant::now() }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
impl Clock for MonotonicClock {
    fn now(&self) -> Duration {
        self.epoch.elapsed()
    }

    fn sleep_until(&mut self, deadline: Duration) {
        if let Some(wait) = deadline.checked_sub(self.now()) {
            std::thread::sleep(wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wait_ends_at_or_after_its_deadline() {
        let mut clock = MonotonicClock::new();
        let deadline = clock.now() + Duration::from_millis(2);
        // A signal may end a wait early; a second wait finishes it
        clock.sleep_until(deadline);
        clock.sleep_until(deadline);
        assert!(clock.now() >= deadline);
    }
}
