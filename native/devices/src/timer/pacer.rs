use std::sync::Arc;
use std::time::Duration;

use engine::block::{BLOCK, RATE};

use super::clock::Clock;
use crate::feed::Feed;
use crate::stats::Stats;

/// The most blocks one wake renders to catch up: a second. A longer stall is a suspended
/// process or a paused VM, and the ingest jitter buffer starts a new stream after a second
/// without a packet anyway, so replaying it would only spin the CPU for its length.
pub const MAX_CATCH_UP: u64 = (RATE as usize / BLOCK) as u64;

const NANOS_PER_SECOND: u128 = 1_000_000_000;

/// The timer clock's schedule. Block `n` falls due `n` periods after the origin, worked out
/// from the origin each time, so a period of 2 666 666.67 ns never rounds into drift. After each
/// wake it renders every block that has fallen due: one when on time, more after a late wake,
/// so the engine's output clock keeps to wall time and the jitter buffers never see a late
/// wake as late packets. A stall past `MAX_CATCH_UP` renders the newest block only and counts
/// one underrun.
pub struct Pacer<C> {
    clock: C,
    origin: Duration,
    /// The next block on the schedule. Blocks rendered and blocks skipped both advance it.
    next: u64,
    stats: Arc<Stats>,
}

impl<C: Clock> Pacer<C> {
    /// Block zero is due now, so the first `tick` renders at once.
    pub fn new(clock: C, stats: Arc<Stats>) -> Self {
        let origin = clock.now();
        Self { clock, origin, next: 0, stats }
    }

    /// Sleeps to the next deadline and renders what has fallen due; returns how many blocks.
    /// Allocates nothing. Each wake that renders is one callback in `Stats`, so its timing
    /// reads the same as a device's.
    pub fn tick(&mut self, feed: &mut Feed) -> u64 {
        self.clock.sleep_until(self.due(self.next));
        let now = self.clock.now();
        let reached = self.reached(now);
        if reached <= self.next {
            return 0;
        }
        self.stats.callback_at(now);
        if reached - self.next > MAX_CATCH_UP {
            self.stats.underrun();
            self.next = reached - 1;
        }
        let blocks = reached - self.next;
        for _ in 0..blocks {
            feed.take(BLOCK, |_, _, _| {});
        }
        self.next = reached;
        blocks
    }

    /// The first moment block `n` is due, rounded up so a wake at it always finds it due.
    fn due(&self, n: u64) -> Duration {
        let nanos = (u128::from(n) * BLOCK as u128 * NANOS_PER_SECOND).div_ceil(u128::from(RATE));
        self.origin + Duration::from_nanos(nanos as u64)
    }

    /// How many blocks are due by `now`.
    fn reached(&self, now: Duration) -> u64 {
        let elapsed = now.saturating_sub(self.origin).as_nanos();
        (elapsed * u128::from(RATE) / (BLOCK as u128 * NANOS_PER_SECOND)) as u64 + 1
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::Relaxed;

    use assert_no_alloc::assert_no_alloc;
    use engine::block::Stereo;
    use engine::driver::Render;

    use super::*;

    const PERIOD_NS: f64 = 1e9 * BLOCK as f64 / RATE as f64;

    /// A clock that jumps to each deadline, plus however late the next wake is meant to be.
    struct Synthetic {
        now: Duration,
        lateness: fn(u64) -> Duration,
        wakes: u64,
    }

    impl Synthetic {
        fn new(lateness: fn(u64) -> Duration) -> Self {
            Self { now: Duration::from_secs(1000), lateness, wakes: 0 }
        }
    }

    impl Clock for Synthetic {
        fn now(&self) -> Duration {
            self.now
        }

        fn sleep_until(&mut self, deadline: Duration) {
            self.now = self.now.max(deadline) + (self.lateness)(self.wakes);
            self.wakes += 1;
        }
    }

    fn counting() -> (Render, Arc<AtomicU64>) {
        let rendered = Arc::new(AtomicU64::new(0));
        let render: Render = Box::new({
            let rendered = Arc::clone(&rendered);
            move |mut out: Stereo<'_>| {
                out.silence();
                rendered.fetch_add(1, Relaxed);
            }
        });
        (render, rendered)
    }

    fn pacer(lateness: fn(u64) -> Duration) -> (Pacer<Synthetic>, Feed, Arc<AtomicU64>) {
        let stats = Arc::new(Stats::new());
        let (render, rendered) = counting();
        let feed = Feed::new(render, Arc::clone(&stats));
        (Pacer::new(Synthetic::new(lateness), stats), feed, rendered)
    }

    /// The blocks wall time has reached since the origin, rendered or not.
    fn wall_blocks(pacer: &Pacer<Synthetic>) -> u64 {
        let elapsed = (pacer.clock.now - pacer.origin).as_nanos();
        (elapsed * 48_000 / 128_000_000_000) as u64 + 1
    }

    #[test]
    fn on_time_wakes_render_one_block_a_period_without_allocating() {
        let (mut pacer, mut feed, rendered) = pacer(|_| Duration::ZERO);
        let blocks = assert_no_alloc(|| (0..3750).map(|_| pacer.tick(&mut feed)).sum::<u64>());
        assert_eq!(blocks, 3750);
        assert_eq!(rendered.load(Relaxed), 3750);

        let report = pacer.stats.report();
        assert_eq!(report.callbacks, 3750);
        assert_eq!(report.underruns, 0);
        // Ten seconds of deadlines a period apart, to the nanosecond the rounding allows
        let shortest = report.shortest.unwrap().as_nanos() as f64;
        let longest = report.longest.unwrap().as_nanos() as f64;
        assert!((shortest - PERIOD_NS).abs() <= 1.0 && (longest - PERIOD_NS).abs() <= 1.0);
        assert_eq!(pacer.clock.now - pacer.origin, Duration::from_nanos(9_997_333_334));
    }

    #[test]
    fn a_late_wake_catches_up_and_the_next_deadline_stays_on_the_grid() {
        // The fourth wake comes 2.5 periods late
        let (mut pacer, mut feed, rendered) =
            pacer(|wake| if wake == 3 { Duration::from_nanos(6_666_667) } else { Duration::ZERO });
        let ticks: Vec<u64> = (0..6).map(|_| pacer.tick(&mut feed)).collect();
        assert_eq!(ticks, [1, 1, 1, 3, 1, 1]);
        assert_eq!(rendered.load(Relaxed), 8);
        assert_eq!(pacer.stats.report().underruns, 0);
        // The sixth wake is on block 7's own deadline, not 2.5 periods after it
        assert_eq!(pacer.clock.now, pacer.due(7));
    }

    #[test]
    fn jittery_wakes_never_drift_from_wall_time() {
        // Up to 1.3 periods late, from a hash of the wake count: a loaded box, deterministically
        let (mut pacer, mut feed, rendered) = pacer(|wake| {
            Duration::from_nanos(wake.wrapping_mul(0x9E37_79B9_7F4A_7C15) % 3_466_667)
        });
        for _ in 0..100_000 {
            pacer.tick(&mut feed);
        }
        assert_eq!(rendered.load(Relaxed), wall_blocks(&pacer));
        let report = pacer.stats.report();
        assert_eq!(report.underruns, 0);
        assert!(report.callbacks < rendered.load(Relaxed), "some wakes caught up");
    }

    #[test]
    fn a_stall_past_a_second_renders_only_the_newest_block_and_counts_an_underrun() {
        let (mut pacer, mut feed, rendered) =
            pacer(|wake| if wake == 1 { Duration::from_secs(3) } else { Duration::ZERO });
        assert_eq!(pacer.tick(&mut feed), 1);
        assert_eq!(pacer.tick(&mut feed), 1);
        assert_eq!(pacer.stats.report().underruns, 1);
        // From there it keeps the schedule from the origin, a block a period
        assert_eq!(pacer.tick(&mut feed), 1);
        assert_eq!(rendered.load(Relaxed), 3);
        assert_eq!(pacer.next, wall_blocks(&pacer));
    }

    #[test]
    fn an_early_return_renders_nothing() {
        /// Returns from every other sleep at once, as a signal would wake it
        struct Interrupted {
            inner: Synthetic,
            woken: bool,
        }
        impl Clock for Interrupted {
            fn now(&self) -> Duration {
                self.inner.now
            }
            fn sleep_until(&mut self, deadline: Duration) {
                self.woken = !self.woken;
                if self.woken {
                    self.inner.sleep_until(deadline);
                }
            }
        }
        let stats = Arc::new(Stats::new());
        let (render, rendered) = counting();
        let mut feed = Feed::new(render, Arc::clone(&stats));
        let clock = Interrupted { inner: Synthetic::new(|_| Duration::ZERO), woken: false };
        let mut pacer = Pacer::new(clock, Arc::clone(&stats));
        let ticks: Vec<u64> = (0..5).map(|_| pacer.tick(&mut feed)).collect();
        assert_eq!(ticks, [1, 0, 1, 0, 1]);
        assert_eq!(rendered.load(Relaxed), 3);
        assert_eq!(stats.report().callbacks, 3);
    }
}
