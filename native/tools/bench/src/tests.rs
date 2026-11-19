use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use assert_no_alloc::assert_no_alloc;
use devices::{Clock as TimerClock, Feed, Pacer, Stats};
use engine::block::{BLOCK, RATE, Stereo};
use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};
use replay::bundle::report::{self, Options as Replay};

use crate::cli::{Args, Clock};
use crate::play::play;

/// The timer clock's time, made up: each wait returns at once at its deadline, and every
/// seventh oversleeps by three periods, so the pacer catches up as it does after a late wake.
struct Synthetic {
    now: Duration,
    waits: u32,
}

impl TimerClock for Synthetic {
    fn now(&self) -> Duration {
        self.now
    }

    fn sleep_until(&mut self, deadline: Duration) {
        self.waits += 1;
        let late = if self.waits.is_multiple_of(7) { 3 * BLOCK as u32 } else { 0 };
        let late = Duration::from_secs_f64(f64::from(late) / f64::from(RATE));
        self.now = self.now.max(deadline) + late;
    }
}

/// A fake output on the timer clock's own pacer and the synthetic clock: it renders as fast as
/// the machine allows, every block under the allocation guard, and keeps what it played.
struct FakeOutput {
    stats: Arc<Stats>,
    heard: Arc<Mutex<Vec<f32>>>,
}

impl Driver for FakeOutput {
    fn start(self: Box<Self>, mut render: Render) -> Result<Running, DriverError> {
        let Self { stats, heard } = *self;
        stats.set_opened("a fake output".into());
        // The first lock may set the lock up, which allocates, so it is taken here and not in render
        drop(heard.lock());
        let keep: Render = Box::new(move |out: Stereo<'_>| {
            let Stereo { left, right } = out;
            render(Stereo { left: &mut *left, right: &mut *right });
            let mut heard = heard.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            for (l, r) in left.iter().zip(right.iter()) {
                if heard.len() < heard.capacity() {
                    heard.extend_from_slice(&[*l, *r]);
                }
            }
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread = thread::spawn({
            let stop = Arc::clone(&stop);
            move || {
                let mut feed = Feed::new(keep, Arc::clone(&stats));
                let mut pacer = Pacer::new(Synthetic { now: Duration::ZERO, waits: 0 }, stats);
                while !stop.load(Ordering::Acquire) {
                    assert_no_alloc(|| pacer.tick(&mut feed));
                }
            }
        });
        let info =
            StreamInfo { sample_rate: RATE, period_frames: BLOCK as u32, output_latency_frames: 0 };
        Ok(Running::new(info, move || {
            stop.store(true, Ordering::Release);
            thread.join().expect("the fake output's thread");
        }))
    }
}

#[test]
fn a_run_renders_what_replay_renders_and_its_bundle_replays_to_the_same_samples() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scorecard/fixtures");
    let replay = Replay { profile: None, backing: Some(fixtures.join("stems")) };
    let duet = fixtures.join("bundles/duet");
    let (_, replayed) = report::play(&duet, &replay).unwrap();
    let out = std::env::temp_dir().join(format!("aeon-bench-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);

    let heard = Arc::new(Mutex::new(Vec::with_capacity(replayed.mix.len())));
    let stats = Arc::new(Stats::new());
    let output = FakeOutput { stats: Arc::clone(&stats), heard: Arc::clone(&heard) };
    let args = Args {
        bundle: duet,
        replay: Replay { profile: None, backing: Some(fixtures.join("stems")) },
        out: Some(out.clone()),
        baseline: None,
        write: None,
        clock: Clock::Timer,
    };
    play(&args, Box::new(output), stats).unwrap();

    let heard = heard.lock().unwrap();
    assert_eq!(heard.len(), replayed.mix.len());
    assert!(*heard == replayed.mix, "the run played other samples than replay");
    let (_, again) = report::play(&out, &replay).unwrap();
    assert!(again.mix == replayed.mix, "the run's bundle replays to other samples");
    std::fs::remove_dir_all(&out).unwrap();
}
