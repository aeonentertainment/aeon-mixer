//! The follower on fake devices. Each fake holds the render its stream was lent, and the test
//! pumps it under the allocation guard as the OS would call it. The fakes report what the follower
//! does to them on a channel, and the test waits on that, never on time.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use assert_no_alloc::assert_no_alloc;
use engine::block::{BLOCK, RATE, Stereo};
use engine::driver::{Driver, DriverError, Movable, Render, Running, StreamInfo};

use super::following::{GIVE_UP, STALL};
use super::{Follower, StartError, System};
use crate::output::Output;
use crate::request::Request;
use crate::stats::Stats;
use crate::wake::Wake;

const WAIT: Duration = Duration::from_secs(5);

/// The stall for a test that never pumps: longer than any test runs, so no stream is lost to it.
const NEVER_STALLS: Duration = Duration::from_secs(3600);

#[derive(Debug, PartialEq)]
enum Seen {
    Listed,
    Started(String),
    Stopped(String),
    Refused(String),
    Dropped,
    Unwatched,
}

/// The OS's side: what it lists, the one stream running with the stats it records callbacks
/// on, when each stream started, and the wake its watch was handed.
#[derive(Default)]
struct Room {
    outputs: Vec<Output>,
    refuses: Vec<String>,
    unreadable: bool,
    device: Option<(String, Render, Arc<Stats>)>,
    starts: Vec<Instant>,
    wake: Option<Wake>,
}

#[derive(Clone)]
struct Fake {
    room: Arc<Mutex<Room>>,
    seen: Sender<Seen>,
}

impl Fake {
    fn room(&self) -> std::sync::MutexGuard<'_, Room> {
        self.room.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Lists `outputs`, the default one marked, and raises the watch as the OS would.
    fn offer(&self, outputs: &[(&str, bool)]) {
        let mut room = self.room();
        room.outputs = outputs
            .iter()
            .map(|(id, is_default)| Output {
                id: (*id).into(),
                name: format!("{id} speakers"),
                is_default: *is_default,
            })
            .collect();
        if let Some(wake) = &room.wake {
            wake.raise();
        }
    }

    fn wake(&self) -> Wake {
        self.room().wake.clone().expect("the follower watches")
    }

    /// Calls the running device back `blocks` times, under the guard, and returns its left
    /// channel.
    fn pump(&self, blocks: usize) -> Vec<f32> {
        let mut room = self.room();
        let (_, render, stats) = room.device.as_mut().expect("a stream is running");
        let mut out = vec![0.0; blocks * BLOCK];
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        for chunk in out.chunks_mut(BLOCK) {
            assert_no_alloc(|| {
                stats.callback();
                render(Stereo { left: &mut left, right: &mut right });
            });
            chunk.copy_from_slice(&left);
        }
        out
    }

    /// One block, caught as a backend's feed catches it. Not under the guard: a panic allocates.
    fn pump_panicking(&self) -> bool {
        let mut room = self.room();
        let (_, render, _) = room.device.as_mut().expect("a stream is running");
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let caught = catch_unwind(AssertUnwindSafe(|| {
            render(Stereo { left: &mut left, right: &mut right });
        }));
        std::panic::set_hook(hook);
        caught.is_err()
    }
}

impl System for Fake {
    fn outputs(&self) -> Result<Vec<Output>, DriverError> {
        let room = self.room();
        let outputs = (!room.unreadable).then(|| room.outputs.clone());
        drop(room);
        let _ = self.seen.send(Seen::Listed);
        outputs.ok_or(DriverError::Backend("unreadable".into()))
    }

    fn driver(&self, request: Request, stats: Arc<Stats>) -> Box<dyn Driver + Send> {
        let id = request.device.expect("opened by id");
        Box::new(FakeDriver { fake: self.clone(), id, stats })
    }

    fn watch(&self, wake: Wake) -> Result<Box<dyn Send>, DriverError> {
        self.room().wake = Some(wake);
        Ok(Box::new(Watching(self.seen.clone())))
    }
}

/// The fake's listener registration, which says when it is removed.
struct Watching(Sender<Seen>);

impl Drop for Watching {
    fn drop(&mut self) {
        let _ = self.0.send(Seen::Unwatched);
    }
}

struct FakeDriver {
    fake: Fake,
    id: String,
    stats: Arc<Stats>,
}

impl Driver for FakeDriver {
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let Self { fake, id, stats } = *self;
        if fake.room().refuses.contains(&id) {
            let _ = fake.seen.send(Seen::Refused(id));
            return Err(DriverError::Backend("refused".into()));
        }
        // A latency and a period of its own per output, so the move shows in what the follower
        // reports and hands the render
        let latency = 100 * id.len() as u32;
        let info = StreamInfo {
            sample_rate: RATE,
            period_frames: 60 * id.len() as u32,
            output_latency_frames: latency,
        };
        let mut room = fake.room();
        room.device = Some((id.clone(), render, stats));
        room.starts.push(Instant::now());
        drop(room);
        let _ = fake.seen.send(Seen::Started(id));
        Ok(Running::new(info, move || {
            let device = fake.room().device.take();
            if let Some((id, render, _)) = device {
                drop(render);
                let _ = fake.seen.send(Seen::Stopped(id));
            }
        }))
    }
}

/// A render that writes the frames it has rendered, counting from 1, so a stream's first sample says whether it
/// carried on from the last one. The block after a move is written negated, as the audio thread
/// fades that block in, so the output shows that the render heard the move.
struct Counter {
    frame: f32,
    moved: bool,
    panics: bool,
    dropped: Sender<Seen>,
    callback: Arc<AtomicU32>,
    latency: Arc<AtomicU32>,
}

impl Movable for Counter {
    fn render(&mut self, out: Stereo<'_>) {
        assert!(!self.panics, "a render that panics");
        let sign = if std::mem::take(&mut self.moved) { -1.0 } else { 1.0 };
        for (left, right) in out.left.iter_mut().zip(out.right.iter_mut()) {
            *left = sign * self.frame;
            *right = sign * self.frame;
            self.frame += 1.0;
        }
    }

    fn moved(&mut self) {
        self.moved = true;
    }

    fn callback_frames(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.callback)
    }

    fn output_latency_frames(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.latency)
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        let _ = self.dropped.send(Seen::Dropped);
    }
}

struct Rig {
    fake: Fake,
    seen: Receiver<Seen>,
    /// What the render was told of each stream's period and output latency.
    callback: Arc<AtomicU32>,
    latency: Arc<AtomicU32>,
    stats: Arc<Stats>,
    follower: Option<Follower>,
}

impl Rig {
    /// A follower opened on `outputs`, pinned to `pinned`, its first stream started.
    fn new(outputs: &[(&str, bool)], pinned: Option<&str>) -> Self {
        Self::with(outputs, pinned, false, NEVER_STALLS)
    }

    fn with(outputs: &[(&str, bool)], pinned: Option<&str>, panics: bool, stall: Duration) -> Self {
        Self::starting(outputs, &[], pinned, panics, stall)
            .unwrap_or_else(|(error, ..)| panic!("the first output opens: {error}"))
    }

    /// A follower started on `outputs`, of which `refuses` will not start.
    fn starting(
        outputs: &[(&str, bool)],
        refuses: &[&str],
        pinned: Option<&str>,
        panics: bool,
        stall: Duration,
    ) -> Result<Self, (StartError, Fake, Receiver<Seen>)> {
        let (sender, seen) = channel();
        let fake = Fake { room: Arc::default(), seen: sender.clone() };
        fake.offer(outputs);
        fake.room().refuses = refuses.iter().map(|id| (*id).into()).collect();
        let (callback, latency) = (Arc::new(AtomicU32::new(0)), Arc::new(AtomicU32::new(0)));
        let counter = Counter {
            frame: 1.0,
            moved: false,
            panics,
            dropped: sender,
            callback: Arc::clone(&callback),
            latency: Arc::clone(&latency),
        };
        let stats = Arc::new(Stats::new());
        let request = Request { device: pinned.map(Into::into), ..Request::default() };
        match Follower::stalling_after(stall, request, counter, fake.clone(), Arc::clone(&stats)) {
            Ok(follower) => {
                Ok(Self { fake, seen, stats, callback, latency, follower: Some(follower) })
            }
            Err(error) => Err((error, fake, seen)),
        }
    }

    fn next(&self) -> Seen {
        self.seen.recv_timeout(WAIT).expect("the follower acts")
    }

    /// What the follower did to the devices, in order. Lists are left out: a wake may or may not
    /// fold into another, and a follower waiting for an output lists again as it retries.
    fn expect(&self, seen: &[Seen]) {
        for expected in seen {
            let next = std::iter::repeat_with(|| self.next()).find(|seen| *seen != Seen::Listed);
            assert_eq!(next.as_ref(), Some(expected));
        }
    }

    /// Waits until the follower has finished whatever its last wake started: it handles one
    /// wake at a time, so once it lists the outputs again, the last one is done. A wake still
    /// pending folds into the one raised here, so the first list may begin that reconcile rather
    /// than follow it; the second list is from a wake raised after the first, so follows it. Only
    /// for a follower that is playing, which lists only when woken.
    fn settle(&self) {
        for _ in 0..2 {
            self.fake.wake().raise();
            while self.next() != Seen::Listed {}
        }
    }

    fn playing(&self) -> (String, u32, u32) {
        let playing = self.follower.as_ref().expect("following").current().get();
        (playing.output.id, playing.moves, playing.info.output_latency_frames)
    }

    fn opened(&self) -> Option<String> {
        self.fake.room().device.as_ref().map(|(id, ..)| id.clone())
    }
}

fn started(id: &str) -> Seen {
    Seen::Started(id.into())
}

fn stopped(id: &str) -> Seen {
    Seen::Stopped(id.into())
}

#[test]
fn a_new_default_takes_the_song_with_nothing_lost_but_the_gap() {
    let rig = Rig::new(&[("hdmi", true), ("built-in", false)], None);
    rig.expect(&[started("hdmi")]);
    assert_eq!(rig.playing(), ("hdmi".into(), 0, 400));
    let before = rig.fake.pump(3);

    rig.fake.offer(&[("hdmi", false), ("built-in", true)]);
    rig.expect(&[stopped("hdmi"), started("built-in")]);
    rig.settle();
    assert_eq!(rig.playing(), ("built-in".into(), 1, 800), "the new output and its latency");

    let after = rig.fake.pump(2);
    let next = before[before.len() - 1] + 1.0;
    assert_eq!(after[0], -next, "the next frame, on the new output, in the block the move fades");
    assert_eq!(after[BLOCK], next + BLOCK as f32, "and the song plays on after it");
}

/// A capture slot sizes its ring by how much the output pulls at a wake, and the guide clock
/// subtracts the output's latency, so the render hears each stream's granted period and latency as
/// that stream starts, before the follower reports the move: 240 frames and 400 on `hdmi`, then
/// 480 and 800 on `built-in`.
#[test]
fn each_stream_tells_the_render_its_period_and_latency_as_it_starts() {
    let rig = Rig::new(&[("hdmi", true), ("built-in", false)], None);
    let told = || (rig.callback.load(Ordering::Acquire), rig.latency.load(Ordering::Acquire));
    rig.expect(&[started("hdmi")]);
    rig.settle();
    assert_eq!(told(), (240, 400));
    rig.fake.offer(&[("hdmi", false), ("built-in", true)]);
    rig.expect(&[stopped("hdmi"), started("built-in")]);
    rig.settle();
    assert_eq!(told(), (480, 800));
}

#[test]
fn a_burst_of_notifications_moves_once() {
    let rig = Rig::new(&[("hdmi", true), ("usb", false)], None);
    rig.expect(&[started("hdmi")]);
    rig.fake.offer(&[("hdmi", false), ("usb", true)]);
    rig.fake.wake().raise();
    rig.fake.wake().raise();
    rig.expect(&[stopped("hdmi"), started("usb")]);
    rig.settle();
    rig.settle();
    let more: Vec<_> = rig.seen.try_iter().filter(|seen| *seen != Seen::Listed).collect();
    assert!(more.is_empty(), "the other wakes found nothing to do: {more:?}");
    assert_eq!(rig.playing().1, 1, "one move");
    assert_eq!(rig.opened().as_deref(), Some("usb"));
}

#[test]
fn a_pinned_output_ignores_the_default_until_it_goes_and_returns_when_it_comes_back() {
    let rig = Rig::new(&[("hdmi", true), ("usb", false)], Some("usb"));
    rig.expect(&[started("usb")]);

    rig.fake.offer(&[("hdmi", false), ("built-in", true), ("usb", false)]);
    rig.settle();
    assert_eq!(rig.playing(), ("usb".into(), 0, 300), "a new default does not move a pin");

    rig.fake.offer(&[("hdmi", false), ("built-in", true)]);
    rig.expect(&[stopped("usb"), started("built-in")]);
    assert!(rig.fake.pump(1)[0] < 0.0, "the render heard the move to the default");
    rig.fake.offer(&[("hdmi", false), ("built-in", true), ("usb", false)]);
    rig.expect(&[stopped("built-in"), started("usb")]);
    rig.settle();
    assert_eq!(rig.playing(), ("usb".into(), 2, 300));
    assert_eq!(rig.fake.pump(1)[0], -(1.0 + BLOCK as f32), "and the move back, a block on");
}

#[test]
fn a_lost_stream_reopens_and_one_with_nowhere_to_go_waits_for_an_output() {
    let rig = Rig::new(&[("usb", true)], None);
    rig.expect(&[started("usb")]);

    // A stream that saw its device go, though the OS still lists it: wedged, so open it again
    rig.stats.lose();
    rig.expect(&[stopped("usb"), started("usb")]);

    // Unplugged, and nothing else listed yet: the render waits, then plays on what appears
    rig.fake.offer(&[]);
    rig.expect(&[stopped("usb")]);
    rig.fake.offer(&[("built-in", true)]);
    rig.expect(&[started("built-in")]);
    rig.settle();
    assert_eq!(rig.playing(), ("built-in".into(), 2, 800));
    assert!(!rig.stats.report().failed);
    assert_eq!(rig.fake.pump(1).len(), BLOCK, "the render came through both moves");
}

#[test]
fn a_stream_that_stops_calling_back_reopens_and_waits_longer_each_time_it_stays_silent() {
    let rig = Rig::with(&[("built-in", true)], None, false, STALL);
    rig.expect(&[started("built-in")]);
    rig.settle();
    rig.fake.pump(3);
    let last = rig.stats.last_callback().expect("it called back");

    // The device stays listed and nothing errors: its callbacks just stop, as on a Mac in DarkWake
    for _ in 0..3 {
        rig.expect(&[stopped("built-in"), started("built-in")]);
    }
    let starts = rig.fake.room().starts.clone();
    let reopened = starts[1] - last;
    assert!(
        (STALL..GIVE_UP).contains(&reopened),
        "reopened {reopened:?} after the last callback, inside the second a command waits"
    );
    assert!(starts[2] - starts[1] >= 2 * STALL, "the next reopen waits twice as long");
    assert!(starts[3] - starts[2] >= 4 * STALL, "and the one after twice again");
    // A stream reports its start before the follower records the move
    rig.settle();
    assert_eq!(rig.playing().1, 3, "the host hears outputChanged for every reopen");
    assert!(!rig.stats.report().failed, "silence never faults the engine");

    // Calling back again, as on waking, ends the run: the next stall is caught after one STALL
    rig.settle();
    rig.fake.pump(1);
    let last = rig.stats.last_callback().expect("it called back");
    rig.settle();
    rig.expect(&[stopped("built-in"), started("built-in")]);
    let reopened = rig.fake.room().starts[4] - last;
    assert!(reopened < 4 * STALL, "reopened {reopened:?} after the callback, not 8 stalls on");
}

#[test]
fn a_list_that_fails_to_read_moves_only_a_stream_that_lost_its_device() {
    let rig = Rig::new(&[("usb", true)], None);
    rig.expect(&[started("usb")]);
    rig.fake.room().unreadable = true;
    rig.settle();
    assert_eq!(rig.opened().as_deref(), Some("usb"), "a stream that plays stays");

    rig.stats.lose();
    rig.expect(&[stopped("usb")]);
    rig.fake.room().unreadable = false;
    rig.fake.offer(&[("built-in", true)]);
    rig.expect(&[started("built-in")]);
}

/// The Mac case of aeon-karaoke#510: the default moved to a monitor whose audio would not start,
/// and the engine faulted a second later with the old output still there. It goes back to the old
/// one instead, with one gap, and says which output refused.
#[test]
fn a_new_default_that_will_not_start_leaves_the_song_on_the_output_it_left() {
    let rig = Rig::new(&[("usb", true), ("monitor", false)], None);
    rig.expect(&[started("usb")]);
    let before = rig.fake.pump(2);
    rig.fake.room().refuses.push("monitor".into());

    rig.fake.offer(&[("usb", false), ("monitor", true)]);
    rig.expect(&[stopped("usb"), Seen::Refused("monitor".into()), started("usb")]);
    rig.settle();
    assert_eq!(rig.playing(), ("usb".into(), 1, 300), "back where it was, as one move");
    let refused = rig.follower.as_ref().expect("following").current().get().refused;
    let refused = refused.expect("the refusal is reported");
    assert_eq!(
        (refused.output.id.as_str(), refused.error.as_str()),
        ("monitor", "audio backend: refused")
    );
    assert!(!rig.stats.report().failed, "a refused move never faults the engine");
    assert_eq!(rig.fake.pump(1)[0], -(before[before.len() - 1] + 1.0), "the song carries on");

    // Later wakes leave it be: the rule still picks the monitor, which already refused
    rig.settle();
    rig.settle();
    let more: Vec<_> = rig.seen.try_iter().filter(|seen| *seen != Seen::Listed).collect();
    assert!(more.is_empty(), "no retry on every wake: {more:?}");
}

/// A refusal lasts while the rule picks that output. Once it picks another, here because the
/// monitor left the list as it powered off, the refusal clears with no move, and the monitor's
/// return as the default is tried afresh.
#[test]
fn a_refused_output_is_tried_again_once_the_rule_has_picked_another() {
    let rig = Rig::new(&[("usb", true), ("monitor", false)], None);
    rig.expect(&[started("usb")]);
    rig.fake.room().refuses.push("monitor".into());
    rig.fake.offer(&[("usb", false), ("monitor", true)]);
    rig.expect(&[stopped("usb"), Seen::Refused("monitor".into()), started("usb")]);

    rig.fake.room().refuses.clear();
    rig.fake.offer(&[("usb", true)]);
    rig.settle();
    let current = rig.follower.as_ref().expect("following").current();
    assert_eq!((current.get().refused, rig.playing().1), (None, 1), "cleared, and no move");

    rig.fake.offer(&[("usb", false), ("monitor", true)]);
    rig.expect(&[stopped("usb"), started("monitor")]);
    rig.settle();
    assert_eq!(rig.playing(), ("monitor".into(), 2, 700));
}

/// A pinned output that comes back but will not start leaves the engine on the default it moved
/// to when the pin went.
#[test]
fn a_pinned_output_that_returns_but_will_not_start_leaves_the_song_on_the_default() {
    let rig = Rig::new(&[("hdmi", true), ("usb", false)], Some("usb"));
    rig.expect(&[started("usb")]);
    rig.fake.offer(&[("hdmi", true)]);
    rig.expect(&[stopped("usb"), started("hdmi")]);

    rig.fake.room().refuses.push("usb".into());
    rig.fake.offer(&[("hdmi", true), ("usb", false)]);
    rig.expect(&[stopped("hdmi"), Seen::Refused("usb".into()), started("hdmi")]);
    rig.settle();
    assert_eq!(rig.playing(), ("hdmi".into(), 2, 400));
    assert!(!rig.stats.report().failed);
}

/// A pinned output back but refusing, then the default it fell back to unplugged: the new default
/// takes the song, as the rule of decision 0016 says it would with no pin.
#[test]
fn a_pinned_engine_whose_pin_refuses_and_whose_fallback_goes_plays_on_the_default() {
    let rig = Rig::new(&[("hdmi", true), ("usb", false)], Some("usb"));
    rig.expect(&[started("usb")]);
    rig.fake.offer(&[("hdmi", true)]);
    rig.expect(&[stopped("usb"), started("hdmi")]);
    rig.fake.room().refuses.push("usb".into());
    rig.fake.offer(&[("hdmi", true), ("usb", false)]);
    rig.expect(&[stopped("hdmi"), Seen::Refused("usb".into()), started("hdmi")]);

    rig.fake.offer(&[("usb", false), ("speakers", true)]);
    rig.expect(&[stopped("hdmi"), Seen::Refused("usb".into()), started("speakers")]);
    rig.settle();
    assert_eq!(rig.playing(), ("speakers".into(), 3, 800));
    assert!(!rig.stats.report().failed);
}

#[test]
fn nothing_that_will_start_within_a_second_faults_the_engine() {
    let rig = Rig::new(&[("usb", true), ("built-in", false)], None);
    rig.expect(&[started("usb")]);
    rig.fake.room().refuses.push("built-in".into());

    rig.fake.offer(&[("built-in", true)]);
    rig.expect(&[stopped("usb"), Seen::Refused("built-in".into())]);
    // The follower retries until it gives up and drops the render; nothing here waits on time
    while rig.next() != Seen::Dropped {}
    assert!(rig.stats.report().failed, "the engine faults");
    assert_eq!(rig.opened(), None);
}

#[test]
fn a_render_that_panicked_is_never_moved() {
    let rig = Rig::with(&[("usb", true), ("built-in", false)], None, true, NEVER_STALLS);
    rig.expect(&[started("usb")]);
    assert!(rig.fake.pump_panicking());

    rig.fake.offer(&[("usb", false), ("built-in", true)]);
    rig.expect(&[Seen::Dropped, stopped("usb")]);
    rig.settle();
    assert!(rig.stats.report().failed);
    assert_eq!(rig.opened(), None, "no stream starts with a torn render");
}

/// aeon-karaoke#511 opens another output when the default will not start, so the failure names
/// the output that refused, and leaves no stream, render or listener behind to hold it.
#[test]
fn an_output_that_will_not_start_at_open_is_named_and_leaves_nothing_behind() {
    let Err((error, fake, seen)) = Rig::starting(
        &[("monitor", true), ("built-in", false)],
        &["monitor"],
        None,
        false,
        NEVER_STALLS,
    ) else {
        panic!("the monitor refuses, so nothing opens");
    };
    let StartError::Unavailable { output: Some(output), .. } = &error else {
        panic!("an unavailable output, not {error:?}");
    };
    assert_eq!(output.id, "monitor");
    let seen: Vec<_> = seen.try_iter().filter(|seen| *seen != Seen::Listed).collect();
    assert_eq!(seen, [Seen::Refused("monitor".into()), Seen::Dropped, Seen::Unwatched]);
    assert!(fake.room().device.is_none(), "no stream runs");
}

#[test]
fn an_os_with_no_output_listed_is_unavailable_with_none_named() {
    let Err((error, ..)) = Rig::starting(&[], &[], None, false, NEVER_STALLS) else {
        panic!("nothing to open");
    };
    assert!(matches!(error, StartError::Unavailable { output: None, .. }), "{error:?}");
}

#[test]
fn closing_stops_the_stream_and_drops_the_render() {
    let mut rig = Rig::new(&[("usb", true)], None);
    rig.expect(&[started("usb")]);
    rig.follower = None;
    rig.expect(&[stopped("usb"), Seen::Dropped, Seen::Unwatched]);
}
