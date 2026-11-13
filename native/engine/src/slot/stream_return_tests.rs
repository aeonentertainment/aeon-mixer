//! A phone stops sending while the backing plays on, then comes back as a new stream whose echo
//! lands a fraction of a packet off where the old stream's did: a whole graph on the offline
//! driver, closed loop, the room hearing the master as the device played it.

use std::collections::VecDeque;
use std::ops::Range;

use assert_no_alloc::{assert_no_alloc, permit_alloc};
use kernels::fixtures::{at_db, db, pink_noise, rms, room, talker};
use rtrb::RingBuffer;

use super::source::fixtures::Tapped;
use super::*;
use crate::backing::{Backing, Planar, Stems, Transport};
use crate::command::event_ring;
use crate::driver::OfflineDriver;
use crate::graph::Graph;
use crate::ingest::fixtures::{DELAY, OpusSender};
use crate::ingest::packet::{Packet, RING_CAPACITY, packet_ring};
use crate::ingest::{Codec, JitterBuffer, PacketSource};
use crate::params::{EchoCancelParams, params_channel};

const SECOND: usize = RATE as usize;
/// The phone sends over `[0, STOPS)`, nothing for two seconds, and from `RETURNS` a new stream.
const STOPS: usize = 6 * SECOND;
const RETURNS: usize = 8 * SECOND;
const END: usize = RETURNS + 3 * SECOND;
/// From the return, the canceller has this long to take its usual echo out again...
const RECOVERS_IN: f64 = 2.0;
/// ...within this much of what it took out in the second before the phone stopped, as after a
/// source gone silent.
const USUAL_WITHIN_DB: f32 = 3.0;
/// A phone back with a room far louder than it learned has this long: a search's 2 s, and two
/// more to learn the room again.
const LOUDER_RECOVERS_IN: f64 = 4.0;
/// A singer holding a note into a phone at the mouth, softly and loudly: 9 and 17 dB over the
/// echo the phone hears.
const SINGERS_DB: [f32; 2] = [-20.0, -12.0];
/// The quickest a stream's packets reach the mixer here, 1.3 ms.
const QUICKEST: usize = BLOCK / 2;

/// Where one stream's echo lands: the phone starts the stream `start` frames in and every packet
/// takes `network` frames to arrive. The jitter buffer anchors a stream's first packet a playout
/// delay after the block it arrives before, so the echo sits as far after the room as the
/// stream's start falls before that block.
#[derive(Clone, Copy, Debug)]
struct Stream {
    start: usize,
    network: usize,
}

impl Stream {
    /// The stream that starts at or just after `from` and anchors the room's echo `lands`
    /// frames late, its packets arriving half a block before the block they are pushed in.
    fn landing(from: usize, frame: usize, lands: usize) -> Self {
        assert!(from.is_multiple_of(BLOCK) && lands >= frame + QUICKEST);
        let start = from + (BLOCK - lands % BLOCK) % BLOCK;
        Self { start, network: lands - frame - QUICKEST }
    }
}

/// How the new stream lands against the old one: `n / d` of a packet later or earlier.
#[derive(Clone, Copy, Debug)]
enum Off {
    Later(usize, usize),
    Earlier(usize, usize),
}

impl Off {
    /// The old stream and the new: every packet of the new one takes the offset longer or shorter
    /// to arrive than the old one's did, and the phone starts it where its first packet falls as
    /// far before a block as the old one's did, so its echo lands exactly that offset away.
    fn streams(self, frame: usize) -> (Stream, Stream) {
        // Late enough that a stream landing a packet earlier still takes time to arrive
        let old = 2 * frame + BLOCK;
        let new = match self {
            Self::Later(n, d) => old + frame * n / d,
            Self::Earlier(n, d) => old - frame * n / d,
        };
        (Stream::landing(0, frame, old), Stream::landing(RETURNS, frame, new))
    }
}

/// The phone: what its mic heard, sent a packet at a time on its stream, and the packets on
/// their way.
struct Phone {
    sender: OpusSender,
    streams: (Stream, Stream),
    heard: Vec<f32>,
    /// The next packet's first frame.
    next: usize,
    on_the_way: VecDeque<(usize, Packet)>,
}

impl Phone {
    fn new(frame: usize, streams: (Stream, Stream), end: usize) -> Self {
        Self {
            sender: OpusSender::new(frame, 0x5107),
            streams,
            heard: Vec::with_capacity(end + BLOCK),
            next: streams.0.start,
            on_the_way: VecDeque::with_capacity(end / frame),
        }
    }

    /// One block of what the mic heard: sends every packet it completes.
    fn hear(&mut self, block: &[f32]) {
        self.heard.extend_from_slice(block);
        let frame = self.sender.frame;
        while self.next + frame <= self.heard.len() {
            if (STOPS..RETURNS).contains(&self.next) {
                // Stopped: the clock runs on to the new stream's start, and nothing is sent
                let new = self.streams.1.start;
                self.sender.pause((new - self.next) as u32);
                self.next = new;
                continue;
            }
            let stream = if self.next < STOPS { self.streams.0 } else { self.streams.1 };
            let pcm = &self.heard[self.next..self.next + frame];
            let packet = self.sender.packet(pcm).expect("no DTX");
            self.next += frame;
            self.on_the_way.push_back((self.next + stream.network, packet));
        }
    }

    /// The packets that have arrived by `now`.
    fn arrived(&mut self, now: usize) -> impl Iterator<Item = Packet> {
        let due = self.on_the_way.iter().take_while(|(at, _)| *at < now).count();
        self.on_the_way.drain(..due).map(|(_, packet)| packet)
    }
}

/// What the source handed the slot, what the slot played, and the path the canceller reported
/// at each block.
struct Run {
    handed: Vec<f32>,
    out: Vec<f32>,
    paths: Vec<Option<u32>>,
}

/// What happens around the phone.
#[derive(Clone)]
struct Scene {
    frame: usize,
    off: Off,
    /// The backing is silent over this.
    quiet: Range<usize>,
    /// Until this frame the room's echo is this many dB quieter, as a phone further from the
    /// speakers hears it.
    quieter: Option<(usize, f32)>,
    /// A singer holds a note into the mic over this stretch, at this level in dBFS.
    singer: Option<(Range<usize>, f32)>,
    end: usize,
}

impl Scene {
    fn new(frame: usize, off: Off) -> Self {
        Self { frame, off, quiet: 0..0, quieter: None, singer: None, end: END }
    }
}

/// A phone in a living room, 10 ms and 10 dB down with a 300 ms tail, hearing a karaoke room's
/// backing through a slot with the canceller on. The backing plays hard right and the slot hard
/// left, so the master's left is the slot alone.
fn run(scene: Scene) -> Run {
    let Scene { frame, off, quiet, quieter, singer, end } = scene;
    let (sung, voice) = singer.map_or((0..0, Vec::new()), |(sung, db)| {
        let voice = at_db(held_note(sung.len()), db);
        (sung, voice)
    });
    let (quieter_until, gain) =
        quieter.map_or((0, 1.0), |(until, db)| (until, 10f32.powf(-db / 20.0)));
    let mut far: Vec<f32> = at_db(talker(end, 110.0, 3), -18.0)
        .iter()
        .zip(&at_db(pink_noise(end, 7), -26.0))
        .map(|(v, b)| v + b)
        .collect();
    far[quiet].fill(0.0);
    let path = room(480, 0.316, 0.3, 0.25, 11);
    let floor = at_db(pink_noise(SECOND, 99), -60.0);
    let mut phone = Phone::new(frame, off.streams(frame), end);
    let (mut packets, consumer) = packet_ring(RING_CAPACITY);
    let jitter = JitterBuffer::new(DELAY, Codec::opus().expect("a decoder"));
    let source = Box::new(PacketSource::new(consumer, jitter));
    let (handed, mut handed_out) = RingBuffer::new(end + BLOCK);
    let slot = Slot::new(Box::new(Tapped { inner: source, handed }));
    let report = slot.echo_report();
    let mut graph = Graph::new(vec![slot]);
    let (mut events, _drain) = event_ring();
    let silent = vec![0.0; far.len()];
    graph.set_backing(Backing::new(Stems::new(Planar { left: silent, right: far }, None), 0));
    graph.steer(1, Transport::Start, &mut events);
    let (mut writer, mut reader) = params_channel();
    writer.update(|p| {
        p.slots[0].pan = -1.0;
        p.slots[0].effects.echo_cancel = EchoCancelParams { enabled: true };
    });
    let mut played: Vec<f32> = Vec::with_capacity(end + BLOCK);
    let mut heard = [0.0; BLOCK];
    let (mut paths, mut paths_out) = RingBuffer::new(end / BLOCK + 1);
    let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
        // The phone and its network are other threads, which may allocate
        permit_alloc(|| {
            let at = played.len();
            for (k, sample) in heard.iter_mut().enumerate() {
                let t = at + k;
                let echo: f32 = path
                    .iter()
                    .filter_map(|&(lag, gain)| Some(gain * played.get(t.checked_sub(lag)?)?))
                    .sum();
                let echo = if t < quieter_until { gain * echo } else { echo };
                let sings = if sung.contains(&t) { voice[t - sung.start] } else { 0.0 };
                *sample = echo + floor[t % floor.len()] + sings;
            }
            phone.hear(&heard);
            for packet in phone.arrived(at) {
                assert!(packets.push(&packet), "the ring never fills in a test");
            }
        });
        let Stereo { left, right } = out;
        graph.render(reader.latest(), Stereo { left: &mut *left, right: &mut *right }, &mut events);
        permit_alloc(|| {
            played.extend(left.iter().zip(right.iter()).map(|(l, r)| 0.5 * (l + r)));
            let _ = paths.push(report.findings().map(|found| found.path_frames));
        });
    }));
    let mut out = Vec::with_capacity(end);
    let mut interleaved = [0.0; 2 * BLOCK];
    while out.len() < end {
        assert_no_alloc(|| driver.pump_into(&mut interleaved));
        out.extend(interleaved.iter().step_by(2));
    }
    let handed = std::iter::from_fn(|| handed_out.pop().ok()).collect();
    let paths = std::iter::from_fn(|| paths_out.pop().ok()).collect();
    Run { handed, out, paths }
}

/// A held, voiced note with no gap: ten harmonics falling as 1/h on 220 Hz with a 5 Hz vibrato of
/// a quarter tone, as a singer sustains a vowel.
fn held_note(frames: usize) -> Vec<f32> {
    let mut phase = 0.0f32;
    (0..frames)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let hz = 220.0 * (1.0 + 0.0145 * (std::f32::consts::TAU * 5.0 * t).sin());
            phase = (phase + hz / RATE as f32) % 1.0;
            (1..=10).map(|h| (std::f32::consts::TAU * h as f32 * phase).sin() / h as f32).sum()
        })
        .collect()
}

fn level(signal: &[f32], from: f64, to: f64) -> f32 {
    db(rms(&signal[(from * SECOND as f64) as usize..(to * SECOND as f64) as usize]))
}

/// The echo taken out over `[from, to)` seconds.
fn removed(run: &Run, from: f64, to: f64) -> f32 {
    level(&run.handed, from, to) - level(&run.out, from, to)
}

// A phone's canceller learns the room for six seconds, then the phone stops for two and comes
// back as a new stream whose echo lands a quarter, a half or three quarters of a packet later or
// earlier than the old one's, on 2.5 ms and 20 ms packets. In each case, in the second that ends
// two seconds after the return, the canceller takes out within 3 dB of what it took out in the
// second before the phone stopped.
#[test]
fn a_phone_back_as_a_new_stream_off_its_old_alignment_has_its_echo_cancelled_again() {
    let return_at = RETURNS as f64 / SECOND as f64;
    let offs: Vec<_> =
        [1, 2, 3].into_iter().flat_map(|n| [Off::Later(n, 4), Off::Earlier(n, 4)]).collect();
    let rows: Vec<_> = [120, 960]
        .into_iter()
        .flat_map(|frame| offs.iter().map(move |&off| (frame, off)))
        .map(|(frame, off)| {
            let run = run(Scene::new(frame, off));
            let usual = removed(&run, 5.0, 6.0);
            let back = removed(&run, return_at + RECOVERS_IN - 1.0, return_at + RECOVERS_IN);
            let path = |at: f64| run.paths[(at * SECOND as f64) as usize / BLOCK];
            let quarters: Vec<String> = (0..12)
                .map(|q| {
                    let from = return_at + 0.25 * q as f64;
                    format!("{:.1}", removed(&run, from, from + 0.25))
                })
                .collect();
            println!(
                "{frame}-frame packets, {off:?}: {usual:.1} dB out before, {back:.1} in the \
                 second to {RECOVERS_IN} s after the return; path {:?} before, {:?} after; by \
                 the quarter second from the return {}",
                path(5.9),
                path(return_at + RECOVERS_IN),
                quarters.join(", ")
            );
            (frame, off, usual, back)
        })
        .collect();
    for (frame, off, usual, back) in rows {
        assert!(
            back >= usual - USUAL_WITHIN_DB,
            "{frame}-frame packets, {off:?}: {back:.1} dB out after, {usual:.1} before"
        );
    }
}

// A host reads `stats().echo` whenever it likes, so a slot that has learned its room reports it
// on every block: from the second before the phone stops, through the stop, its return and the
// search for where its new stream's echo landed, to the end.
#[test]
fn a_phone_back_as_a_new_stream_reports_its_echo_on_every_block() {
    let learned = 5 * SECOND / BLOCK;
    for off in [Off::Later(1, 4), Off::Earlier(1, 4)] {
        let run = run(Scene::new(120, off));
        let lost: Vec<usize> =
            (learned..run.paths.len()).filter(|&b| run.paths[b].is_none()).collect();
        assert!(lost.is_empty(), "{off:?}: no findings at blocks {lost:?}");
    }
}

// A phone comes back a quarter packet off during four seconds of silent backing, three of them
// after its return, as in an intro or a break. With nothing in the master to find the echo by,
// the canceller waits, and finds it once the backing plays again: in the second that ends two
// seconds after the backing returns, it takes out within 3 dB of the second before the phone
// stopped.
#[test]
fn a_phone_back_during_a_silent_backing_has_its_echo_found_when_the_backing_returns() {
    let quiet = RETURNS - SECOND..RETURNS + 3 * SECOND;
    let plays_at = quiet.end as f64 / SECOND as f64;
    for off in [Off::Later(1, 4), Off::Earlier(1, 4)] {
        let scene =
            Scene { quiet: quiet.clone(), end: quiet.end + 3 * SECOND, ..Scene::new(120, off) };
        let run = run(scene);
        let usual = removed(&run, 5.0, 6.0);
        let back = removed(&run, plays_at + RECOVERS_IN - 1.0, plays_at + RECOVERS_IN);
        println!("{off:?}: {usual:.1} dB out before, {back:.1} in the second to 2 s after");
        assert!(back >= usual - USUAL_WITHIN_DB, "{off:?}: {back:.1} dB out, {usual:.1} before");
    }
}

// A phone that learned a room from the table, 15 dB quieter than the living room, comes back
// picked up, hearing the living room itself, with the backing playing. The filter's estimate is
// too quiet against the mic for any lag to stand out, so the search ends once the master has
// reached the mic for 2 s, and the filter learns the room afresh. Adapted, it would have held in
// double talk for good, the mic 15 dB over its estimate. In the second that ends 4 s after the
// return it takes out at least what learning the living room from nothing takes out in its own
// second second.
#[test]
fn a_phone_back_with_its_echo_much_louder_has_it_cancelled_again() {
    let scene = Scene { end: RETURNS + 6 * SECOND, ..Scene::new(120, Off::Later(0, 4)) };
    let learning = removed(&run(scene.clone()), 1.0, 2.0);
    let back = run(Scene { quieter: Some((STOPS, 15.0)), ..scene });
    let usual = removed(&back, 5.0, 6.0);
    let from = RETURNS as f64 / SECOND as f64 + LOUDER_RECOVERS_IN - 1.0;
    let back = removed(&back, from, from + 1.0);
    println!(
        "{usual:.1} dB out before, {back:.1} in the second to {LOUDER_RECOVERS_IN} s after the \
         return; learning the room from nothing, {learning:.1} in its second second"
    );
    assert!(back >= learning, "{back:.1} dB out, {learning:.1} learning from nothing");
}

// A phone comes back on the singer's first note, held for two seconds over the backing, softly
// and loudly. The voice outweighs the estimate on most frames, the search names no lag in its
// two seconds of audible master, and it ends keeping the room it learned over the song.
// Tolerances: from a quarter second after the note, once its tail has passed through the slot's
// 25 ms and the search has ended, a second takes out within 3 dB of the second before the phone
// stopped; and the slot reports its echo on every block, which a filter started over would not.
#[test]
fn a_phone_back_under_a_held_note_keeps_its_room() {
    let return_at = RETURNS as f64 / SECOND as f64;
    for db in SINGERS_DB {
        let scene = Scene {
            singer: Some((RETURNS..RETURNS + 2 * SECOND, db)),
            end: RETURNS + 4 * SECOND,
            ..Scene::new(120, Off::Later(0, 4))
        };
        let run = run(scene);
        let usual = removed(&run, 5.0, 6.0);
        let after = removed(&run, return_at + 2.25, return_at + 3.25);
        let echo = level(&run.handed, 5.0, 6.0);
        let learned = 5 * SECOND / BLOCK;
        let lost = (learned..run.paths.len()).filter(|&b| run.paths[b].is_none()).count();
        println!(
            "singer at {db:.0} dBFS over an echo at {echo:.1}: {usual:.1} dB out before, \
             {after:.1} in the second after the note; {lost} blocks with no findings"
        );
        assert!(after >= usual - USUAL_WITHIN_DB, "{after:.1} dB out, {usual:.1} before");
        assert_eq!(lost, 0, "the filter started over");
    }
}
