use std::time::Duration;

/// What the timer clock waits on. `MonotonicClock` is the OS's; tests drive a synthetic one, so
/// the pacing is proven without wall time.
pub trait Clock {
    /// Time since a fixed epoch. It never goes back.
    fn now(&self) -> Duration;

    /// Returns at `deadline` or after it. It may return early, on a signal; the pacer checks.
    fn sleep_until(&mut self, deadline: Duration);
}
