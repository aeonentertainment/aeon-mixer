//! The network thread of docs/design.md 8.4 and 8.7. One thread owns every UDP socket, polls
//! them with a timeout, and runs the tasks that use them: a `Receiver` parses RTP into a slot's
//! packet ring, and a `send::Sender` sends the return branch of 8.7 on the same loop. A receiver
//! can also record what it reads, through `record`. Sockets live here so the engine keeps none;
//! `mio` does the readiness poll, so this crate has no `unsafe`.
#![forbid(unsafe_code)]

pub mod packetizer;
pub mod receive;
pub mod record;
pub mod rtp;
pub mod send;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use mio::{Events, Poll, Registry, Token, Waker};

/// The longest a poll waits when no socket is ready, so every task's `tick` runs at least this
/// often: half a return-branch frame.
pub const TICK: Duration = Duration::from_millis(5);

const WAKE: Token = Token(usize::MAX);

// Tests that pump the engine wrap the pump in `assert_no_alloc`; this allocator makes it panic.
#[cfg(test)]
#[global_allocator]
static ALLOCATOR: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

/// Something the network thread runs. It owns one socket, registered under its token, and is
/// driven by that socket's readiness and by the tick. Built on the control thread, then moved.
pub trait Task: Send + 'static {
    fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()>;
    fn deregister(&mut self, registry: &Registry) -> io::Result<()>;
    /// The socket is ready, or the task has just joined the thread: service it until it would
    /// block, since readiness is edge-triggered and will not repeat.
    fn ready(&mut self);
    /// After every poll, at least every `TICK`. A receiver needs nothing here; a sender drains
    /// its return ring.
    fn tick(&mut self) {}
}

enum Job {
    Add(Token, Box<dyn Task>),
    Remove(Token),
    Stop,
}

/// The control thread's handle on the network thread. Dropping it stops the thread, which
/// closes every socket still on it.
pub struct Network {
    jobs: mpsc::Sender<Job>,
    waker: Arc<Waker>,
    registry: Registry,
    next_token: AtomicUsize,
    thread: Option<JoinHandle<()>>,
}

impl Network {
    pub fn spawn() -> io::Result<Self> {
        let poll = Poll::new()?;
        let waker = Arc::new(Waker::new(poll.registry(), WAKE)?);
        let registry = poll.registry().try_clone()?;
        let (jobs, inbox) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("aeon-mixer-network".into())
            .spawn(move || run(poll, inbox))?;
        Ok(Self { jobs, waker, registry, next_token: AtomicUsize::new(0), thread: Some(thread) })
    }

    /// Registers the task's socket here, so a failure is the caller's to see, then hands the
    /// task to the thread. Dropping the handle takes it off again.
    pub fn add(&self, mut task: impl Task) -> io::Result<TaskHandle> {
        let token = Token(self.next_token.fetch_add(1, Ordering::Relaxed));
        task.register(&self.registry, token)?;
        self.jobs.send(Job::Add(token, Box::new(task))).map_err(|_| gone())?;
        self.waker.wake()?;
        Ok(TaskHandle { token, jobs: self.jobs.clone(), waker: Arc::clone(&self.waker) })
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Stop);
        let _ = self.waker.wake();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct TaskHandle {
    token: Token,
    jobs: mpsc::Sender<Job>,
    waker: Arc<Waker>,
}

impl Drop for TaskHandle {
    fn drop(&mut self) {
        let _ = self.jobs.send(Job::Remove(self.token));
        let _ = self.waker.wake();
    }
}

fn gone() -> io::Error {
    io::Error::other("the network thread has stopped")
}

fn run(mut poll: Poll, inbox: mpsc::Receiver<Job>) {
    let mut events = Events::with_capacity(64);
    let mut tasks: HashMap<Token, Box<dyn Task>> = HashMap::new();
    loop {
        if let Err(error) = poll.poll(&mut events, Some(TICK))
            && error.kind() != io::ErrorKind::Interrupted
        {
            return;
        }
        for event in &events {
            if let Some(task) = tasks.get_mut(&event.token()) {
                task.ready();
            }
        }
        loop {
            match inbox.try_recv() {
                Ok(Job::Add(token, mut task)) => {
                    task.ready();
                    tasks.insert(token, task);
                }
                Ok(Job::Remove(token)) => {
                    if let Some(mut task) = tasks.remove(&token) {
                        let _ = task.deregister(poll.registry());
                    }
                }
                Ok(Job::Stop) | Err(TryRecvError::Disconnected) => return,
                Err(TryRecvError::Empty) => break,
            }
        }
        for task in tasks.values_mut() {
            task.tick();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Receiver, Sender, channel};

    use super::*;

    /// No socket: it reports what the thread does with it.
    struct Probe {
        report: Sender<&'static str>,
    }

    impl Task for Probe {
        fn register(&mut self, _: &Registry, _: Token) -> io::Result<()> {
            self.report.send("registered").unwrap();
            Ok(())
        }

        fn deregister(&mut self, _: &Registry) -> io::Result<()> {
            self.report.send("deregistered").unwrap();
            Ok(())
        }

        fn ready(&mut self) {
            self.report.send("ready").unwrap();
        }

        fn tick(&mut self) {
            let _ = self.report.send("tick");
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            let _ = self.report.send("dropped");
        }
    }

    fn next_other_than_tick(reports: &Receiver<&'static str>) -> &'static str {
        loop {
            let report = reports.recv().unwrap();
            if report != "tick" {
                return report;
            }
        }
    }

    #[test]
    fn a_task_joins_the_thread_is_serviced_and_leaves_when_its_handle_drops() {
        let network = Network::spawn().unwrap();
        let (report, reports) = channel();
        let handle = network.add(Probe { report }).unwrap();
        assert_eq!(
            reports.recv().unwrap(),
            "registered",
            "on the control thread, before add returns"
        );
        assert_eq!(reports.recv().unwrap(), "ready", "serviced once on joining, to arm the edge");
        assert_eq!(reports.recv().unwrap(), "tick", "then ticked");
        drop(handle);
        assert_eq!(next_other_than_tick(&reports), "deregistered");
        assert_eq!(next_other_than_tick(&reports), "dropped");
    }

    #[test]
    fn dropping_the_network_stops_the_thread_and_drops_its_tasks() {
        let network = Network::spawn().unwrap();
        let (report, reports) = channel();
        let _handle = network.add(Probe { report }).unwrap();
        drop(network);
        assert!(reports.iter().any(|r| r == "dropped"));
    }
}
