use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use engine::block::{BLOCK, RATE};
use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};

use super::monotonic::MonotonicClock;
use super::pacer::Pacer;
use super::priority;
use crate::feed::Feed;
use crate::stats::Stats;

/// The engine's clock on a box with no sound card: Walter, CI, a container. Its own thread
/// renders a block at each deadline on the OS's monotonic clock, so a stalled Node event loop
/// no longer stalls the mix. Nothing plays the output; the mix leaves through a mixer's return
/// stream, so the latency it reports is zero, as the offline clock's is.
///
/// The thread's one OS call is the wait, made between blocks as a device driver's thread waits
/// on its device. Render runs inside the same `Feed` as every backend's.
pub struct TimerDriver {
    stats: Arc<Stats>,
}

impl TimerDriver {
    pub fn new(stats: Arc<Stats>) -> Self {
        Self { stats }
    }
}

impl Driver for TimerDriver {
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let Self { stats } = *self;
        stats.set_opened(format!("timer, {}, a block of {BLOCK} frames", MonotonicClock::NAME));
        let info =
            StreamInfo { sample_rate: RATE, period_frames: BLOCK as u32, output_latency_frames: 0 };
        let stop = Arc::new(AtomicBool::new(false));
        let mut feed = Feed::new(render, Arc::clone(&stats));
        let audio = thread::Builder::new()
            .name("aeon-audio".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || {
                    stats.set_realtime(priority::promote());
                    let mut pacer = Pacer::new(MonotonicClock::new(), stats);
                    while !stop.load(Ordering::Acquire) {
                        pacer.tick(&mut feed);
                    }
                    feed
                }
            })
            .map_err(|error| DriverError::Backend(format!("timer thread: {error}")))?;

        Ok(Running::new(info, move || {
            stop.store(true, Ordering::Release);
            // The feed comes back so render, and whatever it holds, drops here and not on the
            // timer's thread. The join waits at most one period and a render.
            drop(audio.join());
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU64;
    use std::sync::atomic::Ordering::Relaxed;
    use std::sync::mpsc;

    use engine::block::Stereo;

    use super::*;

    #[test]
    fn it_renders_on_its_own_thread_until_stopped() {
        let rendered = Arc::new(AtomicU64::new(0));
        let (reached, milestone) = mpsc::sync_channel(1);
        let render: Render = Box::new({
            let rendered = Arc::clone(&rendered);
            move |mut out: Stereo<'_>| {
                out.silence();
                if rendered.fetch_add(1, Relaxed) == 8 {
                    let _ = reached.try_send(());
                }
            }
        });
        let stats = Arc::new(Stats::new());
        let running = Box::new(TimerDriver::new(Arc::clone(&stats))).start(render).unwrap();
        assert_eq!(running.info.output_latency_frames, 0);
        assert_eq!(running.info.period_frames, 128);

        // The test thread blocks here, as a stalled event loop would, and the blocks still come
        milestone.recv().unwrap();
        drop(running);
        // Stopping joined the thread and dropped render with the sender it holds
        assert!(milestone.recv().is_err());
        assert_eq!(Arc::strong_count(&rendered), 1);
        assert!(rendered.load(Relaxed) >= 9);
        assert!(!stats.report().failed);
    }
}
