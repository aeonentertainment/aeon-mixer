//! The follower on fake devices. Each fake holds the render its stream was lent, and the test
//! pumps it under the allocation guard as the OS would call it. The fakes report what the follower
//! does to them on a channel, and the test waits on that, never on time.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use assert_no_alloc::assert_no_alloc;
use engine::block::{BLOCK, RATE, Stereo};
use engine::driver::{Driver, DriverError, Movable, Render, Running, StreamInfo};

use super::{Follower, System};
use crate::output::Output;
use crate::request::Request;
use crate::stats::Stats;
use crate::wake::Wake;

const WAIT: Duration = Duration::from_secs(5);

#[derive(Debug, PartialEq)]
enum Seen {
    Listed,
    Started(String),
    Stopped(String),
    Refused(String),
    Dropped,
}

/// The OS's side: what it lists, the one stream running, and the wake its watch was handed.
#[derive(Default)]
struct Room {
    outputs: Vec<Output>,
    refuses: Vec<String>,
    unreadable: bool,
    device: Option<(String, Render)>,
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

    /// Renders `blocks` on the running device, under the guard, and returns its left channel.
    fn pump(&self, blocks: usize) -> Vec<f32> {
        let mut room = self.room();
        let (_, render) = room.device.as_mut().expect("a stream is running");
        let mut out = vec![0.0; blocks * BLOCK];
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        for chunk in out.chunks_mut(BLOCK) {
            assert_no_alloc(|| render(Stereo { left: &mut left, right: &mut right }));
            chunk.copy_from_slice(&left);
        }
        out
    }

    /// One block, caught as a backend's feed catches it. Not under the guard: a panic allocates.
    fn pump_panicking(&self) -> bool {
        let mut room = self.room();
        let (_, render) = room.device.as_mut().expect("a stream is running");
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

    fn driver(&self, request: Request, _stats: Arc<Stats>) -> Box<dyn Driver + Send> {
        Box::new(FakeDriver { fake: self.clone(), id: request.device.expect("opened by id") })
    }

    fn watch(&self, wake: Wake) -> Result<Box<dyn Send>, DriverError> {
        self.room().wake = Some(wake);
        Ok(Box::new(()))
    }
}

struct FakeDriver {
    fake: Fake,
    id: String,
}

impl Driver for FakeDriver {
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let Self { fake, id } = *self;
        if fake.room().refuses.contains(&id) {
            let _ = fake.seen.send(Seen::Refused(id));
            return Err(DriverError::Backend("refused".into()));
        }
        // A latency of its own per output, so the move shows in what the follower reports
        let latency = 100 * id.len() as u32;
        let info = StreamInfo {
            sample_rate: RATE,
            period_frames: BLOCK as u32,
            output_latency_frames: latency,
        };
        fake.room().device = Some((id.clone(), render));
        let _ = fake.seen.send(Seen::Started(id));
        Ok(Running::new(info, move || {
            let device = fake.room().device.take();
            if let Some((id, render)) = device {
                drop(render);
                let _ = fake.seen.send(Seen::Stopped(id));
            }
        }))
    }
}

/// A render that writes the frames it has rendered, so a stream's first sample says whether it
/// carried on from the last one, and counts its moves.
struct Counter {
    frame: f32,
    moves: Arc<AtomicU32>,
    panics: bool,
    dropped: Sender<Seen>,
}

impl Movable for Counter {
    fn render(&mut self, out: Stereo<'_>) {
        assert!(!self.panics, "a render that panics");
        for (left, right) in out.left.iter_mut().zip(out.right.iter_mut()) {
            *left = self.frame;
            *right = self.frame;
            self.frame += 1.0;
        }
    }

    fn moved(&mut self) {
        self.moves.fetch_add(1, Ordering::Relaxed);
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
    moves: Arc<AtomicU32>,
    stats: Arc<Stats>,
    follower: Option<Follower>,
}

impl Rig {
    /// A follower opened on `outputs`, pinned to `pinned`, its first stream started.
    fn new(outputs: &[(&str, bool)], pinned: Option<&str>) -> Self {
        Self::with(outputs, pinned, false)
    }

    fn with(outputs: &[(&str, bool)], pinned: Option<&str>, panics: bool) -> Self {
        let (sender, seen) = channel();
        let fake = Fake { room: Arc::default(), seen: sender.clone() };
        fake.offer(outputs);
        let moves = Arc::new(AtomicU32::new(0));
        let counter = Counter { frame: 0.0, moves: Arc::clone(&moves), panics, dropped: sender };
        let stats = Arc::new(Stats::new());
        let request = Request { device: pinned.map(Into::into), ..Request::default() };
        let follower = Follower::start(request, counter, fake.clone(), Arc::clone(&stats))
            .expect("the first output opens");
        Self { fake, seen, moves, stats, follower: Some(follower) }
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
    /// wake at a time, so once it lists the outputs again, the last one is done. Only for a
    /// follower that is playing, which lists only when woken.
    fn settle(&self) {
        self.fake.wake().raise();
        while self.next() != Seen::Listed {}
    }

    fn playing(&self) -> (String, u32, u32) {
        let playing = self.follower.as_ref().expect("following").current().get();
        (playing.output.id, playing.moves, playing.info.output_latency_frames)
    }

    fn opened(&self) -> Option<String> {
        self.fake.room().device.as_ref().map(|(id, _)| id.clone())
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
    assert_eq!(rig.moves.load(Ordering::Relaxed), 1);

    let after = rig.fake.pump(2);
    assert_eq!(after[0], before[before.len() - 1] + 1.0, "the next frame, on the new output");
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
    rig.fake.offer(&[("hdmi", false), ("built-in", true), ("usb", false)]);
    rig.expect(&[stopped("built-in"), started("usb")]);
    rig.settle();
    assert_eq!(rig.playing(), ("usb".into(), 2, 300));
    assert_eq!(rig.moves.load(Ordering::Relaxed), 2);
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

#[test]
fn nothing_to_play_on_within_a_second_faults_the_engine() {
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
    let rig = Rig::with(&[("usb", true), ("built-in", false)], None, true);
    rig.expect(&[started("usb")]);
    assert!(rig.fake.pump_panicking());

    rig.fake.offer(&[("usb", false), ("built-in", true)]);
    rig.expect(&[Seen::Dropped, stopped("usb")]);
    rig.settle();
    assert!(rig.stats.report().failed);
    assert_eq!(rig.moves.load(Ordering::Relaxed), 0);
    assert_eq!(rig.opened(), None, "no stream starts with a torn render");
}

#[test]
fn closing_stops_the_stream_and_drops_the_render() {
    let mut rig = Rig::new(&[("usb", true)], None);
    rig.expect(&[started("usb")]);
    rig.follower = None;
    rig.expect(&[stopped("usb"), Seen::Dropped]);
}
