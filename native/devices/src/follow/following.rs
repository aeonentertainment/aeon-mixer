use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use engine::driver::{DriverError, Movable, Running};

use super::current::Current;
use super::dock::Dock;
use super::playing::Playing;
use super::system::System;
use crate::output::Output;
use crate::request::Request;
use crate::stats::Stats;

/// How long a lost output may wait for one to move to before the engine faults. A command sent in
/// the gap is answered after it, and the binding gives up on one after a second.
pub const GIVE_UP: Duration = Duration::from_secs(1);

/// How often an output that refused to open is tried again, inside `GIVE_UP`.
const RETRY: Duration = Duration::from_millis(100);

/// The follower thread's state: the stream it runs, or the `Movable` it holds between streams.
pub struct Following<M, S> {
    request: Request,
    system: S,
    stats: Arc<Stats>,
    current: Current,
    state: State<M>,
    _watch: Box<dyn Send>,
}

enum State<M> {
    Playing {
        running: Running,
        dock: Dock<M>,
    },
    /// Stopped since the wake at `since`, with nowhere yet to play.
    Waiting {
        movable: M,
        since: Instant,
    },
    /// Faulted: a torn render, or nothing to play on within `GIVE_UP`.
    Gone,
}

impl<M: Movable, S: System> Following<M, S> {
    /// Opens the first stream where `request` says to play, on the calling thread.
    pub fn open(
        request: Request,
        movable: M,
        system: S,
        stats: Arc<Stats>,
        watch: Box<dyn Send>,
    ) -> Result<Self, DriverError> {
        let outputs = system.outputs()?;
        let output =
            target(&outputs, request.device.as_deref()).ok_or(DriverError::NoDevice)?.clone();
        let (running, dock) =
            start(&system, &request, &stats, &output, movable).map_err(|(error, _)| error)?;
        let current =
            Current::new(Playing { info: running.info, output, moves: 0, gap: Duration::ZERO });
        Ok(Self {
            request,
            system,
            stats,
            current,
            state: State::Playing { running, dock },
            _watch: watch,
        })
    }

    pub fn current(&self) -> Current {
        self.current.clone()
    }

    /// Follows until `stopping` is set and a wake shows it.
    pub fn run(mut self, woken: &Receiver<()>, stopping: &AtomicBool) {
        loop {
            let woke = match &self.state {
                State::Waiting { since, .. } => {
                    let left = GIVE_UP.saturating_sub(since.elapsed());
                    if left.is_zero() {
                        self.give_up();
                        continue;
                    }
                    woken.recv_timeout(left.min(RETRY))
                }
                State::Playing { .. } | State::Gone => {
                    woken.recv().map_err(|_| RecvTimeoutError::Disconnected)
                }
            };
            if stopping.load(Ordering::Acquire) || woke == Err(RecvTimeoutError::Disconnected) {
                return;
            }
            self.reconcile(Instant::now());
        }
    }

    // Plays where the rule says, or does nothing when it already does: a wake says only that
    // something changed, so three of them for one change cost one move.
    fn reconcile(&mut self, woke_at: Instant) {
        let lost = self.stats.take_lost();
        // A list that fails to read leaves a stream that still plays alone; one that has lost its
        // device stops, and waiting retries the list
        let outputs = match self.system.outputs() {
            Ok(outputs) => outputs,
            Err(_) if lost && matches!(self.state, State::Playing { .. }) => Vec::new(),
            Err(_) => return,
        };
        let target = target(&outputs, self.request.device.as_deref()).cloned();

        if let State::Playing { .. } = self.state {
            let playing = self.current.get().output;
            let listed = outputs.iter().any(|output| output.id == playing.id);
            let stays = target.as_ref().is_none_or(|target| target.id == playing.id);
            if listed && stays && !lost {
                return;
            }
            let State::Playing { running, dock } = mem::replace(&mut self.state, State::Gone)
            else {
                return;
            };
            drop(running);
            // A stream may count its own stop as a loss; the move answers it either way
            self.stats.take_lost();
            let Some(mut movable) = dock.take() else { return self.give_up() };
            movable.moved();
            self.state = State::Waiting { movable, since: woke_at };
        }

        let Some(target) = target else { return };
        let State::Waiting { movable, since } = mem::replace(&mut self.state, State::Gone) else {
            return;
        };
        match start(&self.system, &self.request, &self.stats, &target, movable) {
            Ok((running, dock)) => {
                let moves = self.current.get().moves + 1;
                self.current.set(Playing {
                    info: running.info,
                    output: target,
                    moves,
                    gap: since.elapsed(),
                });
                self.state = State::Playing { running, dock };
            }
            Err((_, Some(movable))) => self.state = State::Waiting { movable, since },
            Err((_, None)) => self.give_up(),
        }
    }

    // The fault is marked before the render drops, so the engine hears of it first
    fn give_up(&mut self) {
        self.stats.fail();
        self.state = State::Gone;
    }
}

/// The rule of decision 0016: the pinned output while the OS lists it, else the default.
fn target<'a>(outputs: &'a [Output], pinned: Option<&str>) -> Option<&'a Output> {
    pinned
        .and_then(|id| outputs.iter().find(|output| output.id == id))
        .or_else(|| outputs.iter().find(|output| output.is_default))
}

// Opens by id even when following the default, so the output reported is the one opened if the
// default moves again in between; that move wakes the follower once more.
fn start<M: Movable>(
    system: &impl System,
    request: &Request,
    stats: &Arc<Stats>,
    output: &Output,
    movable: M,
) -> Result<(Running, Dock<M>), (DriverError, Option<M>)> {
    let (render, dock) = Dock::lend(movable);
    let request = Request { device: Some(output.id.clone()), ..request.clone() };
    match system.driver(request, Arc::clone(stats)).start(render) {
        Ok(running) => Ok((running, dock)),
        Err(error) => Err((error, dock.take())),
    }
}
