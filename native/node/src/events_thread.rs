//! The events thread of docs/design.md section 2: every 5 ms it drains the event ring, drops
//! what the retired ring holds, reads the device's stats, and calls one threadsafe function
//! with the batch. It is the one consumer of both rings, so the audio thread never shares an
//! index with anyone else.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use devices::Stats;
use engine::command::{EventConsumer, Retired, RetiredConsumer};
use napi::Status;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};

use crate::event::NativeEvent;

pub const DRAIN_INTERVAL: Duration = Duration::from_millis(5);

pub type Deliver = ThreadsafeFunction<Vec<NativeEvent>, (), Vec<NativeEvent>, Status, false>;

pub struct EventsThread {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EventsThread {
    /// `stats` is the device stream's record, read for underruns and failure; the offline clock
    /// has none.
    pub fn spawn(
        events: EventConsumer,
        retired: RetiredConsumer,
        stats: Option<Arc<Stats>>,
        deliver: Deliver,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let mut drain = Drain { events, retired, stats, underruns_seen: 0, failed_seen: false };
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
        Self { stop, handle: Some(handle) }
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
        batch
    }
}
