//! A phone stops sending while the backing plays on, then comes back as a new stream whose echo
//! lands a fraction of a packet off where the old stream's did: a whole graph on the offline
//! driver, closed loop, the room hearing the master as the device played it.

use std::collections::VecDeque;

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
    fn new(frame: usize, streams: (Stream, Stream)) -> Self {
        Self {
            sender: OpusSender::new(frame, 0x5107),
            streams,
            heard: Vec::with_capacity(END + BLOCK),
            next: streams.0.start,
            on_the_way: VecDeque::with_capacity(END / frame),
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

/// A phone in a living room, 10 ms and 10 dB down with a 300 ms tail, hearing a karaoke room's
/// backing through a slot with the canceller on. The backing plays hard right and the slot hard
/// left, so the master's left is the slot alone.
fn run(frame: usize, off: Off) -> Run {
    let far: Vec<f32> = at_db(talker(END, 110.0, 3), -18.0)
        .iter()
        .zip(&at_db(pink_noise(END, 7), -26.0))
        .map(|(v, b)| v + b)
        .collect();
    let path = room(480, 0.316, 0.3, 0.25, 11);
    let floor = at_db(pink_noise(SECOND, 99), -60.0);
    let mut phone = Phone::new(frame, off.streams(frame));
    let (mut packets, consumer) = packet_ring(RING_CAPACITY);
    let jitter = JitterBuffer::new(DELAY, Codec::opus().expect("a decoder"));
    let source = Box::new(PacketSource::new(consumer, jitter));
    let (handed, mut handed_out) = RingBuffer::new(END + BLOCK);
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
    let mut played: Vec<f32> = Vec::with_capacity(END + BLOCK);
    let mut heard = [0.0; BLOCK];
    let (mut paths, mut paths_out) = RingBuffer::new(END / BLOCK + 1);
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
                *sample = echo + floor[t % floor.len()];
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
    let mut out = Vec::with_capacity(END);
    let mut interleaved = [0.0; 2 * BLOCK];
    while out.len() < END {
        assert_no_alloc(|| driver.pump_into(&mut interleaved));
        out.extend(interleaved.iter().step_by(2));
    }
    let handed = std::iter::from_fn(|| handed_out.pop().ok()).collect();
    let paths = std::iter::from_fn(|| paths_out.pop().ok()).collect();
    Run { handed, out, paths }
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
            let run = run(frame, off);
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
        let run = run(120, off);
        let lost: Vec<usize> =
            (learned..run.paths.len()).filter(|&b| run.paths[b].is_none()).collect();
        assert!(lost.is_empty(), "{off:?}: no findings at blocks {lost:?}");
    }
}
