use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use engine::driver::{DriverError, Movable, Running};

use super::current::Current;
use super::dock::Dock;
use super::playing::Playing;
use super::refused::Refused;
use super::start_error::StartError;
use super::starting::Starting;
use super::system::System;
use crate::output::Output;
use crate::request::Request;
use crate::stats::Stats;

/// How long a lost output may wait for one to move to before the engine faults: none of the
/// output the rule picks, the one it left and the default will start. A command sent in the gap is
/// answered after it, and the binding gives up on one after a second.
pub const GIVE_UP: Duration = Duration::from_secs(1);

/// How long an output's start may take before it is refused as one that failed, at open and on a
/// move. A healthy output starts in tens of ms and a Bluetooth one can take a second or two, while
/// a CoreAudio output whose clock never runs is held for the HAL's own 10 s and then fails. Decision
/// 0016 has the case.
pub const REFUSE_AFTER: Duration = Duration::from_secs(3);

/// How often an output that refused to open is tried again, inside `GIVE_UP`.
const RETRY: Duration = Duration::from_millis(100);

/// How long a playing stream may go without a callback before it is lost, on every backend: a
/// device can stay listed and raise no error while it stops asking for audio, as a CoreAudio
/// IOProc does on a Mac asleep or in DarkWake. Twenty of WASAPI's 10 ms shared periods, the
/// longest any backend runs at, and short enough that the reopen fits in the second a command
/// may wait.
pub const STALL: Duration = Duration::from_millis(200);

/// How many times the wait for a silent stream doubles. A stream reopened after a stall that
/// still has not called back waits twice as long as the last before it is reopened again, up to
/// 32 stalls, 6.4 s. A Mac that sleeps through the night is then reopened about ten times a
/// minute while it is awake enough to run the follower, not five times a second, and is never
/// faulted for it.
const DOUBLINGS: u32 = 5;

/// The follower thread's state: the stream it runs, or the `Movable` it holds between streams.
pub struct Following<M, S> {
    request: Request,
    system: S,
    stats: Arc<Stats>,
    current: Current,
    state: State<M>,
    /// When the playing stream started: its silence counts from here until it calls back.
    started: Instant,
    stall: Duration,
    /// Stalls in a row with no callback between, each of which doubles the next wait.
    quiet: u32,
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
    /// Faulted: a torn render, or nothing that would start within `GIVE_UP`.
    Gone,
}

impl<M: Movable, S: System> Following<M, S> {
    /// Opens the first stream where `request` says to play, on the calling thread. A stream
    /// with no callback for `stall` is lost.
    pub fn open(
        request: Request,
        movable: M,
        system: S,
        stats: Arc<Stats>,
        watch: Box<dyn Send>,
        stall: Duration,
    ) -> Result<Self, StartError> {
        let outputs = system.outputs().map_err(StartError::Backend)?;
        let output = target(&outputs, request.device.as_deref())
            .ok_or(StartError::Unavailable { output: None, error: DriverError::NoDevice })?
            .clone();
        let (running, dock) =
            start(&system, &request, &stats, &output, movable).map_err(|(error, _)| {
                StartError::Unavailable { output: Some(output.clone()), error }
            })?;
        let current = Current::new(Playing {
            info: running.info,
            output,
            moves: 0,
            gap: Duration::ZERO,
            refused: None,
        });
        Ok(Self {
            request,
            system,
            stats,
            current,
            state: State::Playing { running, dock },
            started: Instant::now(),
            stall,
            quiet: 0,
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
                State::Playing { .. } => {
                    let silence = self.silence();
                    woken.recv_timeout(self.patience().saturating_sub(silence))
                }
                State::Gone => woken.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            if stopping.load(Ordering::Acquire) || woke == Err(RecvTimeoutError::Disconnected) {
                return;
            }
            self.reconcile(Instant::now());
        }
    }

    // Plays where the rule says, or does nothing when it already does: a wake says only that
    // something changed, so three of them for one change cost one move. An output the rule picks
    // that will not start sends the engine back to the one it left, and is not tried again until
    // the rule picks another, so a refusal costs one gap, not one on every wake.
    fn reconcile(&mut self, woke_at: Instant) {
        let stalled = self.stalled();
        let lost = self.stats.take_lost() || stalled;
        // A list that fails to read leaves a stream that still plays alone; one that has lost its
        // device stops, and waiting retries the list
        let outputs = match self.system.outputs() {
            Ok(outputs) => outputs,
            Err(_) if lost && matches!(self.state, State::Playing { .. }) => Vec::new(),
            Err(_) => return,
        };
        let target = target(&outputs, self.request.device.as_deref()).cloned();
        let playing = self.current.get();
        let refused = playing.refused.as_ref().is_some_and(|refused| {
            target.as_ref().is_some_and(|target| target.id == refused.output.id)
        });

        if let State::Playing { .. } = self.state {
            let listed = outputs.iter().any(|output| output.id == playing.output.id);
            let stays =
                refused || target.as_ref().is_none_or(|target| target.id == playing.output.id);
            if listed && stays && !lost {
                // Cleared only where it stays: a move sets the refusal its own start leaves
                if playing.refused.is_some() && !refused {
                    self.current.set(Playing { refused: None, ..playing });
                }
                return;
            }
            self.quiet = if stalled { (self.quiet + 1).min(DOUBLINGS) } else { 0 };
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

        let State::Waiting { mut movable, since } = mem::replace(&mut self.state, State::Gone)
        else {
            return;
        };
        // Where the rule says, then the output it left while the OS still lists it, then the
        // default, which a pinned engine falls back to when its pin and that one both fail
        let left = outputs.iter().find(|output| output.id == playing.output.id);
        let default = outputs.iter().find(|output| output.is_default);
        let mut candidates: Vec<&Output> = Vec::with_capacity(3);
        for output in target.iter().chain(left).chain(default) {
            if !candidates.iter().any(|tried| tried.id == output.id) {
                candidates.push(output);
            }
        }
        let mut refusal = None;
        for output in candidates {
            match start(&self.system, &self.request, &self.stats, output, movable) {
                Ok((running, dock)) => {
                    self.current.set(Playing {
                        info: running.info,
                        output: output.clone(),
                        moves: playing.moves + 1,
                        gap: since.elapsed(),
                        refused: refusal,
                    });
                    self.state = State::Playing { running, dock };
                    self.started = Instant::now();
                    return;
                }
                Err((error, Some(back))) => {
                    if target.as_ref() == Some(output) {
                        refusal =
                            Some(Refused { output: output.clone(), error: error.to_string() });
                    }
                    movable = back;
                }
                Err((_, None)) => return self.give_up(),
            }
        }
        self.state = State::Waiting { movable, since };
    }

    // Whether the playing stream has asked for no audio for its whole patience. Its device may
    // still be listed and still be the target: a stall is a loss all the same.
    fn stalled(&mut self) -> bool {
        matches!(self.state, State::Playing { .. }) && self.silence() >= self.patience()
    }

    // How long the playing stream has asked for no audio: since its last callback, or since it
    // started if it has not called back yet. A callback ends a run of stalls.
    fn silence(&mut self) -> Duration {
        let heard = self.stats.last_callback().filter(|at| *at >= self.started);
        if heard.is_some() {
            self.quiet = 0;
        }
        heard.unwrap_or(self.started).elapsed()
    }

    fn patience(&self) -> Duration {
        self.stall * (1 << self.quiet)
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
// default moves again in between; that move wakes the follower once more. A start still running
// after `REFUSE_AFTER` is refused, and the movable comes back unless the stream has rendered with
// it, which means it is running and the start is waited out.
fn start<M: Movable>(
    system: &impl System,
    request: &Request,
    stats: &Arc<Stats>,
    output: &Output,
    movable: M,
) -> Result<(Running, Dock<M>), (DriverError, Option<M>)> {
    let (callback, latency) = (movable.callback_frames(), movable.output_latency_frames());
    let (render, dock) = Dock::lend(movable);
    let request = Request { device: Some(output.id.clone()), ..request.clone() };
    let starting = match Starting::spawn(system.driver(request, Arc::clone(stats)), render) {
        Ok(starting) => starting,
        Err(error) => return Err((error, dock.take())),
    };
    let started = match starting.within(REFUSE_AFTER) {
        Some(result) => result,
        None => match dock.take() {
            Some(movable) => {
                let error = format!("did not start within {} s", REFUSE_AFTER.as_secs());
                return Err((DriverError::Backend(error), Some(movable)));
            }
            None => starting.finish(),
        },
    };
    match started {
        Ok(running) => {
            // The stream may have rendered already. A capture slot re-priming from `moved` waits
            // for more mic than arrives before this store, and the next block hears it; so does
            // the guide clock.
            latency.store(running.info.output_latency_frames, Ordering::Release);
            callback.store(running.info.period_frames, Ordering::Release);
            Ok((running, dock))
        }
        Err(error) => Err((error, dock.take())),
    }
}
