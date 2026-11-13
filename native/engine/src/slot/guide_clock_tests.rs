//! The guide clock across what can move it: a seek, a pause, a stream that stalls, a live key
//! change and a move to another output. The clock is the backing playhead (docs/design.md
//! section 3), so packet timing never moves it; these are the moves a host makes or the room
//! hears, each on the offline clock under the allocation guard. Each test checks where the
//! readings sit, that every target is the guide's at its reading's position, so none is stale,
//! and how many target changes a host counting them would see.

use assert_no_alloc::assert_no_alloc;
use kernels::{Preset, Stretch};

use crate::audio_thread::AudioThread;
use crate::backing::test_support::SECOND;
use crate::backing::{Backing, Planar, Stems, Transport};
use crate::block::{BLOCK, RATE, Stereo};
use crate::command::{Command, Event, EventConsumer, EventProducer, event_ring};
use crate::driver::Movable;
use crate::graph::Graph;
use crate::ingest::fixtures::L16Sender;
use crate::ingest::packet::RING_CAPACITY;
use crate::ingest::{Codec, JitterBuffer, PacketSource, packet_ring};
use crate::params::{CorrectorParams, Params};
use crate::slot::Slot;
use crate::slot::guide::{Guide, Note};
use crate::slot::source::Source;
use crate::slot::source::fixtures::Sine;

const B_FLAT_3: u8 = 58;
/// The detector's 10 ms hop: a reading comes every this many frames the slot renders.
const HOP: i64 = 480;
const HOP_MS: f64 = 10.0;
const CORRECTING: CorrectorParams =
    CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 };

/// One reading as a tuning panel sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Read {
    position_ms: f64,
    target: Option<i16>,
    confidence: f32,
}

/// How far a reading's centre trails the newest frame it read.
fn lag() -> i64 {
    kernels::PitchCorrector::new(RATE).reading_lag_samples() as i64
}

/// A reading that ends at `frame` on the guide's clock sits here.
fn position_ms(frame: i64) -> f64 {
    (frame - lag()) as f64 * 1000.0 / f64::from(RATE)
}

fn guide(notes: &[(u64, u64)]) -> Box<Guide> {
    let notes = notes.iter().map(|&(start_frame, end_frame)| Note {
        midi: B_FLAT_3,
        start_frame,
        end_frame,
    });
    Box::new(Guide { notes: notes.collect(), key_mask: 0 })
}

fn pitch(event: Event) -> Option<Read> {
    match event {
        Event::Pitch { position_ms, target_midi, confidence, .. } => {
            Some(Read { position_ms, target: target_midi, confidence })
        }
        _ => None,
    }
}

/// Every reading's target is the guide's note at its position, B flat moved by `semitones`, so
/// none is stale: the one the corrector set for its last run, a frame before the reading ends.
fn assert_on_the_guide(reads: &[Read], notes: &[(u64, u64)], semitones: i16) {
    for read in reads {
        let frame = (read.position_ms * f64::from(RATE) / 1000.0).round() as i64 + lag() - 1;
        let sounding =
            notes.iter().any(|&(start, end)| (start as i64..end as i64).contains(&frame));
        let expected = sounding.then(|| i16::from(B_FLAT_3) + semitones);
        assert_eq!(read.target, expected, "{read:?}");
    }
}

/// Each target that differs from the reading's before it: what a host counting changes sees.
fn changes(reads: &[Read]) -> Vec<(f64, Option<i16>)> {
    reads
        .windows(2)
        .filter(|w| w[0].target != w[1].target)
        .map(|w| (w[1].position_ms, w[1].target))
        .collect()
}

/// The readings after which the guide clock did not move on by one hop.
fn jumps(reads: &[Read]) -> Vec<usize> {
    (1..reads.len())
        .filter(|&i| (reads[i].position_ms - reads[i - 1].position_ms - HOP_MS).abs() > 1e-9)
        .collect()
}

/// A one-slot song over 20 s of silent backing, its corrector following B flat 3 at each of
/// `notes`, started and rendered a block at a time, with no output latency.
struct Song {
    graph: Graph,
    params: Params,
    events: EventProducer,
    drain: EventConsumer,
    reads: Vec<Read>,
}

impl Song {
    fn new(source: Box<dyn Source>, notes: &[(u64, u64)]) -> Self {
        let mut slot = Slot::new(source);
        slot.set_guide(Some(guide(notes)));
        let mut graph = Graph::new(vec![slot]);
        graph.set_backing(Backing::new(Stems::new(Planar::silence(20 * SECOND), None), 0));
        let (events, drain) = event_ring();
        let mut params = Params::default();
        params.slots[0].effects.corrector = CORRECTING;
        let mut song = Self { graph, params, events, drain, reads: Vec::new() };
        song.steer(Transport::Start);
        song
    }

    /// A singer holding 226 Hz throughout.
    fn singing(notes: &[(u64, u64)]) -> Self {
        Self::new(Box::new(Sine::new(226.0, 0.5)), notes)
    }

    fn steer(&mut self, transport: Transport) {
        self.graph.steer(0, transport, &mut self.events);
    }

    fn render(&mut self, frames: usize) {
        for _ in 0..frames / BLOCK {
            let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
            let Self { graph, params, events, .. } = self;
            assert_no_alloc(|| {
                graph.render(params, Stereo { left: &mut left, right: &mut right }, events);
            });
            self.reads.extend(std::iter::from_fn(|| self.drain.pop()).filter_map(pitch));
        }
    }
}

// A seek moves the clock in one jump, after its 20 ms fade out, and the first reading after it
// carries the target at the new position, never the one it left. The jump changes the target only
// when it crosses an edge: a seek back out of the note, one forward into it, one within it.
#[test]
fn a_seek_lands_on_the_target_at_its_new_position() {
    const NOTE: (u64, u64) = (48_000, 96_000);
    let into = Some(i16::from(B_FLAT_3));
    let cases = [
        (72_000, 24_000, vec![into, None, into]),
        (24_000, 72_000, vec![into, None]),
        (57_600, 86_400, vec![into, None]),
    ];
    for (from, to, expected) in cases {
        let mut song = Song::singing(&[NOTE]);
        song.render(from);
        song.steer(Transport::Seek { to_frame: to });
        song.render(72_000);

        assert_on_the_guide(&song.reads, &[NOTE], 0);
        let jumps = jumps(&song.reads);
        assert_eq!(jumps.len(), 1, "{from} to {to}: {jumps:?}");
        let landed = song.reads[jumps[0]].position_ms;
        let earliest = position_ms(to as i64);
        assert!(
            landed > earliest && landed <= earliest + HOP_MS,
            "{from} to {to}: the first reading after the seek sits at {landed}"
        );
        let targets: Vec<_> = changes(&song.reads).into_iter().map(|(_, target)| target).collect();
        assert_eq!(targets, expected, "{from} to {to}");
    }
}

// The playhead stops 64 frames short of the note. Paused, every block holds the clock there, so the
// target stays the key's and the readings stay on the held frame; resumed, it crosses the edge once.
#[test]
fn a_pause_short_of_an_edge_crosses_it_once_after_the_resume() {
    const HELD: u64 = 49_024;
    const NOTE: (u64, u64) = (HELD + 64, 96_000);
    let mut song = Song::singing(&[NOTE]);
    song.render(SECOND);
    song.steer(Transport::Pause);
    song.render(SECOND);
    assert_eq!(song.graph.position_frames(), HELD, "the pause's fade out ran on to here");
    let paused = song.reads.len();
    song.steer(Transport::Resume);
    song.render(SECOND);

    assert_on_the_guide(&song.reads, &[NOTE], 0);
    let held = &song.reads[paused - 50..paused];
    let on_the_held_frame = |read: &Read| read.position_ms == position_ms(HELD as i64);
    assert!(held.iter().all(|read| on_the_held_frame(read) && read.target.is_none()), "{held:?}");
    assert!(song.reads.windows(2).all(|w| w[1].position_ms >= w[0].position_ms), "never back");
    let into = changes(&song.reads);
    assert_eq!(into.len(), 2, "into the note and out at its end: {into:?}");
    let edge = position_ms(NOTE.0 as i64);
    assert!((edge..edge + HOP_MS).contains(&into[0].0), "{into:?}");
}

// A phone's 5 ms packets stop for 200 ms, far longer than its 15 ms playout delay, and a note
// starts in the silence. Packets never set the clock, so the readings keep coming 10 ms apart, the
// target turns at the edge though nobody is heard, and the voice comes back pulled to the note.
#[test]
fn a_stalled_stream_comes_back_on_the_note_it_stalled_into() {
    const PACKET: usize = 240;
    const STALL: std::ops::Range<usize> = SECOND..SECOND + SECOND / 5;
    const NOTE: (u64, u64) = (52_800, 120_000);
    let (mut producer, consumer) = packet_ring(RING_CAPACITY);
    let jitter = JitterBuffer::new(RATE * 15 / 1000, Codec::L16);
    let mut song = Song::new(Box::new(PacketSource::new(consumer, jitter)), &[NOTE]);
    let mut voice = Sine::new(226.0, 0.5);
    let mut sender = L16Sender::new(PACKET, 1);
    let mut pcm = [0.0; PACKET];
    let (mut sent, mut rendered) = (0, 0);
    while rendered < 3 * SECOND {
        // A packet reaches the ring once the output clock has passed its first frame; in the
        // stall the phone keeps capturing, and every packet is lost on the way
        while sent <= rendered {
            voice.pull(&mut pcm);
            let packet = sender.packet(&pcm);
            if !STALL.contains(&sent) {
                assert!(producer.push(&packet), "the ring has room");
            }
            sent += PACKET;
        }
        song.render(BLOCK);
        rendered += BLOCK;
    }

    assert_on_the_guide(&song.reads, &[NOTE], 0);
    // The one step is at the stream's opening, before the song's first note: until its first
    // packet says it sends 5 ms frames, the jitter buffer counts on 20 ms ones
    let [opening] = jumps(&song.reads)[..] else { panic!("{:?}", jumps(&song.reads)) };
    assert!(song.reads[opening].position_ms < 0.0, "{:?}", song.reads[opening]);
    assert_eq!(changes(&song.reads).len(), 2, "into the note and out of it");
    // The clock names when the singer sang. `end` is the last reading whose window ends in the
    // stall; within two hops after it, the voice is read again, and on the note
    let end = position_ms(STALL.end as i64);
    let back = song.reads.iter().find(|r| r.position_ms > end && r.confidence > 0.9);
    let back = back.expect("the voice came back");
    assert!(back.position_ms <= end + 2.0 * HOP_MS, "read again at {}", back.position_ms);
    assert_eq!(back.target, Some(i16::from(B_FLAT_3)));
    // Every reading whose window lies in the stall hears nothing, but the last: the jitter buffer
    // knows its delay to within the block a packet waits, so the voice may reach it early
    let inside = position_ms(STALL.start as i64 + 2 * lag())..end;
    let silent: Vec<_> = song.reads.iter().filter(|r| inside.contains(&r.position_ms)).collect();
    assert!(silent.len() >= 15 && silent.iter().all(|r| r.confidence < 0.5), "{silent:?}");
}

// A live key change moves the guide's note by the same semitones once, as its crossfade ends. The
// first one also engages the stretcher, so the room then hears the backing its latency late,
// 140 ms of it again, and the clock steps back with it; later changes cost no step.
#[test]
fn a_live_key_change_transposes_the_target_once() {
    const NOTE: (u64, u64) = (24_000, 200_000);
    let mut song = Song::singing(&[NOTE]);
    song.render(SECOND);
    song.steer(Transport::SetKey { semitones: 2 });
    song.render(SECOND);
    song.steer(Transport::SetKey { semitones: 4 });
    song.render(SECOND);

    let targets: Vec<_> = changes(&song.reads).into_iter().map(|(_, target)| target).collect();
    let note = i16::from(B_FLAT_3);
    assert_eq!(targets, [Some(note), Some(note + 2), Some(note + 4)]);
    let latency = Stretch::new(2, RATE, Preset::Cheaper).latency_frames() as f64;
    let jumps = jumps(&song.reads);
    assert_eq!(jumps.len(), 1, "{jumps:?}");
    let (before, after) = (song.reads[jumps[0] - 1].position_ms, song.reads[jumps[0]].position_ms);
    assert!((before + HOP_MS - after - latency * 1000.0 / f64::from(RATE)).abs() < 1e-9);
    assert_eq!(changes(&song.reads)[1].0, after, "the key and the step land in one block");
}

// The step back replays 140 ms of backing to the room, and an edge inside it is heard twice: dry
// at the old key, then stretched at the new one. The target follows what the room hears, so it
// turns at the edge twice, the second time transposed. Nothing reports an edge the room did not
// hear, and only the first change of a song can do this.
#[test]
fn an_edge_the_first_key_change_replays_is_crossed_again_at_the_new_key() {
    const NOTE: (u64, u64) = (52_800, 200_000);
    let mut song = Song::singing(&[NOTE]);
    song.render(SECOND);
    song.steer(Transport::SetKey { semitones: 2 });
    song.render(SECOND);

    let [stepped] = jumps(&song.reads)[..] else { panic!("one step") };
    let (dry, stretched) = song.reads.split_at(stepped);
    assert_on_the_guide(dry, &[NOTE], 0);
    assert_on_the_guide(stretched, &[NOTE], 2);
    let note = i16::from(B_FLAT_3);
    let targets: Vec<_> = changes(&song.reads).into_iter().map(|(_, target)| target).collect();
    assert_eq!(targets, [Some(note), None, Some(note + 2)]);
}

// Once the stretcher is engaged, a seek back to the start leaves the heard position at 0 until the
// playhead is past the stretcher's latency, 140 ms. The clock holds there as it does on a pause, so
// an edge 64 frames in is crossed once, after the hold, and the readings never step back.
#[test]
fn a_seek_to_the_start_under_the_live_key_holds_the_clock_until_it_is_heard() {
    const NOTE: (u64, u64) = (64, 96_000);
    let mut song = Song::singing(&[NOTE]);
    song.steer(Transport::SetKey { semitones: 2 });
    song.render(SECOND);
    let before = song.reads.len();
    song.steer(Transport::Seek { to_frame: 0 });
    song.render(SECOND);

    let after = &song.reads[before..];
    let landed = (1..after.len()).find(|&i| after[i].position_ms < after[i - 1].position_ms);
    let landed = landed.expect("the seek went back");
    let restarted = &after[landed..];
    assert_on_the_guide(restarted, &[NOTE], 2);
    let held = restarted.iter().take_while(|read| read.position_ms == position_ms(0)).count();
    assert!(held >= 12, "{held} readings held at the start");
    assert!(restarted.windows(2).all(|w| w[1].position_ms >= w[0].position_ms), "never back");
    let into: Vec<_> = changes(restarted).into_iter().map(|(_, target)| target).collect();
    assert_eq!(into, [Some(i16::from(B_FLAT_3) + 2)], "into the note once");
}

// A move to another output holds the playhead for the gap and starts the slot again, so the next
// reading comes a hop after the move on the clock where it stopped: no jump, and an edge just past
// the move is crossed once. The output keeps its latency here; #105 tests one that changes it.
#[test]
fn an_output_move_holds_the_clock_through_the_gap() {
    const LATENCY: u32 = 480;
    const MOVED_AT: i64 = SECOND as i64;
    const NOTE: (u64, u64) = (MOVED_AT as u64 + 200, 90_000);
    let (mut audio, mut controls) = AudioThread::new();
    let mut slot = Slot::new(Box::new(Sine::new(226.0, 0.5)));
    slot.set_guide(Some(guide(&[NOTE])));
    let mut graph = Box::new(Graph::new(vec![slot]));
    graph.set_backing(Backing::new(Stems::new(Planar::silence(4 * SECOND), None), 0));
    graph.set_backing_offset_frames(LATENCY);
    controls.commands.push(Command::Swap { id: 1, graph }).ok().unwrap();
    controls.commands.push(Command::Start { id: 2 }).ok().unwrap();
    controls.params.update(|p| p.slots[0].effects.corrector = CORRECTING);
    let mut reads = Vec::new();
    let mut render = |audio: &mut AudioThread, frames: i64| {
        for _ in 0..frames / BLOCK as i64 {
            let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
            assert_no_alloc(|| audio.render(Stereo { left: &mut left, right: &mut right }));
            reads.extend(std::iter::from_fn(|| controls.events.pop()).filter_map(pitch));
        }
    };
    render(&mut audio, MOVED_AT);
    Movable::moved(&mut audio);
    render(&mut audio, MOVED_AT);

    // A reading every hop before the move, and every hop again from it
    let before = (MOVED_AT / HOP) as usize;
    assert_eq!(reads.len(), 2 * before);
    assert_eq!(jumps(&reads), [], "the clock carried on from where it stopped");
    assert_eq!(reads[before].position_ms, position_ms(MOVED_AT + HOP - i64::from(LATENCY)));
    assert_on_the_guide(&reads, &[NOTE], 0);
    assert_eq!(changes(&reads).len(), 2, "into the note once, and out at its end");
}
