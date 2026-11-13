//! The events thread of docs/design.md section 2: every 5 ms it drains the event ring, drops
//! what the retired ring holds, reads the device's stats, the follower's moves and the render
//! jobs' progress, and calls one threadsafe function with the batch. It is the one consumer of both rings, so the
//! audio thread never shares an index with anyone else.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use devices::{Current, Stats};
use engine::command::{EventConsumer, Retired, RetiredConsumer};
use napi::Status;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};

use crate::backing::{SharedRenderJob, lock};
use crate::engine::OutputInfo;
use crate::event::NativeEvent;

pub const DRAIN_INTERVAL: Duration = Duration::from_millis(5);

pub type Deliver = ThreadsafeFunction<Vec<NativeEvent>, (), Vec<NativeEvent>, Status, false>;

pub struct EventsThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    watches: Sender<RenderWatch>,
}

/// A render job the thread reads on each drain: its fraction when it moves, its end once.
struct RenderWatch {
    id: u32,
    job: SharedRenderJob,
    last: f32,
}

impl EventsThread {
    /// `stats` is the device stream's record, read for underruns and failure, and `current` the
    /// follower's, read for moves; the offline clock has neither.
    pub fn spawn(
        events: EventConsumer,
        retired: RetiredConsumer,
        stats: Option<Arc<Stats>>,
        current: Option<Current>,
        deliver: Deliver,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let (watches, new_watches) = mpsc::channel();
        let mut drain = Drain {
            events,
            retired,
            stats,
            underruns_seen: 0,
            failed_seen: false,
            current,
            moves_seen: 0,
            new_watches,
            renders: Vec::new(),
        };
        let handle = thread::Builder::new()
            .name("aeon-mixer-events".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || {
                    while !stop.load(Ordering::Relaxed) {
                        let batch = drain.collect();
                        if !batch.is_empty() {
                            deliver.call(batch, ThreadsafeFunctionCallMode::NonBlocking);
                        }
                        thread::sleep(DRAIN_INTERVAL);
                    }
                }
            })
            .expect("the events thread spawns");
        Self { stop, handle: Some(handle), watches }
    }

    /// Reports `job`'s progress under `id` until it finishes or its stems are taken.
    pub fn watch(&self, id: u32, job: SharedRenderJob) {
        let _ = self.watches.send(RenderWatch { id, job, last: -1.0 });
    }

    /// Ends the loop and waits for it, at most one interval; the threadsafe function is released
    /// with the thread, so no batch arrives after this returns.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

struct Drain {
    events: EventConsumer,
    retired: RetiredConsumer,
    stats: Option<Arc<Stats>>,
    underruns_seen: u64,
    failed_seen: bool,
    current: Option<Current>,
    moves_seen: u32,
    new_watches: Receiver<RenderWatch>,
    renders: Vec<RenderWatch>,
}

impl Drain {
    fn collect(&mut self) -> Vec<NativeEvent> {
        let mut batch = Vec::new();
        while let Some(event) = self.events.pop() {
            batch.push(NativeEvent::from(event));
        }
        while let Some(retired) = self.retired.pop() {
            match retired {
                Retired::Graph(graph) => {
                    drop(graph);
                    batch.push(NativeEvent::Retired);
                }
                Retired::Guide(guide) => drop(guide),
            }
        }
        if let Some(stats) = &self.stats {
            let report = stats.report();
            batch.extend((self.underruns_seen..report.underruns).map(|_| NativeEvent::Underrun));
            self.underruns_seen = report.underruns;
            if report.failed && !self.failed_seen {
                self.failed_seen = true;
                batch.push(NativeEvent::Fault);
            }
        }
        if let Some(playing) = self.current.as_ref().map(Current::get)
            && playing.moves != self.moves_seen
        {
            self.moves_seen = playing.moves;
            batch.push(NativeEvent::OutputChanged {
                gap_ms: playing.gap.as_secs_f64() * 1000.0,
                output: OutputInfo::new(playing.info, Some(playing)),
            });
        }
        self.renders.extend(self.new_watches.try_iter());
        self.renders.retain_mut(|watch| {
            let guard = lock(&watch.job);
            let Some(job) = guard.as_ref() else { return false };
            // Finished is read first, so the fraction beside it is the worker's last word; and
            // once it reads true, the join `installBacking` does returns without blocking
            let finished = job.is_finished();
            let fraction = job.progress();
            if fraction != watch.last {
                watch.last = fraction;
                batch.push(NativeEvent::RenderProgress {
                    job: watch.id,
                    fraction: f64::from(fraction),
                });
            }
            if finished {
                batch.push(NativeEvent::RenderFinished { job: watch.id });
            }
            !finished
        });
        batch
    }
}
