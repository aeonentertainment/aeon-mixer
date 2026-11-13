//! The echo stages through a whole graph on the offline driver, closed loop: a `Room` hears the
//! master as the device played it, through a synthetic room, and hands it to a slot with a
//! singer on top, so the slot's own voice comes back to it as it does in a real room.

use assert_no_alloc::assert_no_alloc;
use kernels::fixtures::{at_db, db, pink_noise, rms, room, talker};
use rtrb::{Consumer, Producer, RingBuffer};

use super::*;
use crate::audio_thread::AudioThread;
use crate::backing::{Backing, Planar, Stems, Transport};
use crate::command::{Command, event_ring};
use crate::driver::OfflineDriver;
use crate::graph::Graph;
use crate::params::{
    CorrectorParams, EchoCancelParams, EffectParams, HowlGuardParams, VoiceParams, params_channel,
};

const SECOND: usize = RATE as usize;

/// A slot's microphone in a room. It hears the master a block after it is rendered, since a
/// played block reaches the air after it leaves the graph, through `path`, plus `singer`.
/// From `shift_at` on it hands everything over `shift` frames later and says so in its latency,
/// as a jitter buffer that moves its delay does. Before `joins_at` it hands over silence, as a
/// slot does before its phone joins.
struct Room {
    played: Consumer<f32>,
    heard: Vec<f32>,
    path: Vec<(usize, f32)>,
    /// Biquad coefficients and state: a resonance the room adds on the way, or none.
    resonance: Option<([f32; 5], [f32; 4])>,
    singer: Vec<f32>,
    floor: Vec<f32>,
    mic: Producer<f32>,
    at: usize,
    latency: u32,
    shift_at: usize,
    shift: usize,
    joins_at: usize,
}

impl Source for Room {
    fn pull(&mut self, out: &mut [f32]) {
        while let Ok(sample) = self.played.pop() {
            let sample = match &mut self.resonance {
                Some((c, z)) => {
                    let y = c[0] * sample + c[1] * z[0] + c[2] * z[1] - c[3] * z[2] - c[4] * z[3];
                    *z = [sample, z[0], y, z[2]];
                    y
                }
                None => sample,
            };
            self.heard.push(sample);
        }
        for sample in out.iter_mut() {
            let t = self.at.checked_sub(if self.at >= self.shift_at { self.shift } else { 0 });
            let joined = self.at >= self.joins_at;
            *sample = t.filter(|_| joined).map_or(0.0, |t| {
                let echo: f32 = self
                    .path
                    .iter()
                    .filter_map(|&(lag, gain)| Some(gain * self.heard.get(t.checked_sub(lag)?)?))
                    .sum();
                echo + self.singer.get(t).copied().unwrap_or(0.0) + self.floor[t % self.floor.len()]
            });
            let _ = self.mic.push(*sample);
            self.at += 1;
        }
    }

    fn latency_frames(&self) -> u32 {
        self.latency + if self.at >= self.shift_at { self.shift as u32 } else { 0 }
    }
}

/// Plays `far` hard right through one slot while the room's mic plays hard left through
/// another, for `frames`. Returns what the mic heard, what its slot handed on, and the slot's
/// findings at the end.
struct Run {
    mic: Vec<f32>,
    out: Vec<f32>,
    report: Arc<EchoReport>,
    /// The frame at the end of the block the howl guard first ducked in.
    first_catch: Option<usize>,
}

struct Scene {
    /// Played hard right: through a slot, or through the backing when the scene seeks.
    far: Vec<f32>,
    singer: Vec<f32>,
    path: Vec<(usize, f32)>,
    resonance: Option<[f32; 5]>,
    shift_at: usize,
    shift: usize,
    echo_cancel: bool,
    howl_guard: bool,
    /// The mic slot's other effects.
    effects: EffectParams,
    mic_gain: f32,
    /// The backing seeks to the second frame at the first.
    seek: Option<(usize, u64)>,
    /// The phone joins here: the slot is reset, as a new sender resets it, and the room is heard.
    joins_at: usize,
    /// How long to run; the far end's length unless set.
    frames: usize,
}

impl Scene {
    fn new(far: Vec<f32>, path: Vec<(usize, f32)>) -> Self {
        let frames = far.len();
        Self {
            far,
            singer: vec![0.0; frames],
            path,
            resonance: None,
            shift_at: usize::MAX,
            shift: 0,
            echo_cancel: true,
            howl_guard: false,
            effects: EffectParams::default(),
            mic_gain: 1.0,
            seek: None,
            joins_at: 0,
            frames,
        }
    }

    fn run(self) -> Run {
        let frames = self.frames;
        let (mut played, played_out) = RingBuffer::new(4 * BLOCK);
        let (mic_in, mut mic_out) = RingBuffer::new(frames + BLOCK);
        let room = Room {
            played: played_out,
            heard: Vec::with_capacity(frames + BLOCK),
            path: self.path,
            resonance: self.resonance.map(|c| (c, [0.0; 4])),
            singer: self.singer,
            floor: at_db(pink_noise(SECOND, 99), -60.0),
            mic: mic_in,
            at: 0,
            latency: 0,
            shift_at: self.shift_at,
            shift: self.shift,
            joins_at: self.joins_at,
        };
        let mic_slot = Slot::new(Box::new(room));
        let report = mic_slot.echo_report();
        let (mut events, _drain) = event_ring();
        let mut graph = match self.seek {
            None => {
                Graph::new(vec![mic_slot, Slot::new(Box::new(Clip { samples: self.far, at: 0 }))])
            }
            Some(_) => {
                let mut graph = Graph::new(vec![mic_slot]);
                let silent = vec![0.0; self.far.len()];
                let stems = Stems::new(Planar { left: silent, right: self.far }, None);
                graph.set_backing(Backing::new(stems, 0));
                graph.steer(1, Transport::Start, &mut events);
                graph
            }
        };
        let (mut writer, mut reader) = params_channel();
        writer.update(|p| {
            p.slots[0].pan = -1.0;
            p.slots[0].gain = self.mic_gain;
            p.slots[0].effects = EffectParams {
                echo_cancel: EchoCancelParams { enabled: self.echo_cancel },
                howl_guard: HowlGuardParams { enabled: self.howl_guard },
                ..self.effects
            };
            p.slots[1].pan = 1.0;
        });
        let (seek, joins_at) = (self.seek, self.joins_at);
        let mut rendered = 0;
        let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
            if let Some((_, to_frame)) = seek.filter(|(at, _)| *at == rendered) {
                graph.steer(2, Transport::Seek { to_frame }, &mut events);
            }
            if joins_at > 0 && rendered == joins_at {
                graph.reset_slot(0);
            }
            rendered += BLOCK;
            let Stereo { left, right } = out;
            graph.render(
                reader.latest(),
                Stereo { left: &mut *left, right: &mut *right },
                &mut events,
            );
            for (l, r) in left.iter().zip(right.iter()) {
                let _ = played.push(0.5 * (l + r));
            }
        }));
        let mut out = Vec::with_capacity(frames);
        let mut interleaved = [0.0; 2 * BLOCK];
        let mut first_catch = None;
        while out.len() + BLOCK <= frames {
            assert_no_alloc(|| driver.pump_into(&mut interleaved));
            out.extend(interleaved.iter().step_by(2));
            if first_catch.is_none() && report.howl_catches() > 0 {
                first_catch = Some(out.len());
            }
        }
        let mic = std::iter::from_fn(|| mic_out.pop().ok()).collect();
        Run { mic, out, report, first_catch }
    }
}

/// Hands over `samples`, then silence.
struct Clip {
    samples: Vec<f32>,
    at: usize,
}

impl Source for Clip {
    fn pull(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.samples.get(self.at).copied().unwrap_or(0.0);
            self.at += 1;
        }
    }
}

fn seconds(signal: &[f32], from: f64, to: f64) -> &[f32] {
    &signal[(from * SECOND as f64) as usize..((to * SECOND as f64) as usize).min(signal.len())]
}

/// The level taken out over `[from, to)` seconds.
fn removed(run: &Run, from: f64, to: f64) -> f32 {
    db(rms(seconds(&run.mic, from, to))) - db(rms(seconds(&run.out, from, to)))
}

/// A karaoke room's far end: a voice over a pink bed, as `aec:measure` builds it, at the levels
/// it uses.
fn backing(frames: usize) -> Vec<f32> {
    let voice = at_db(talker(frames, 110.0, 3), -18.0);
    let bed = at_db(pink_noise(frames, 7), -26.0);
    voice.iter().zip(&bed).map(|(v, b)| v + b).collect()
}

/// A house mic: the output's own 10 ms, then a living room's direct path 10 dB down and a
/// 300 ms tail.
fn living_room() -> Vec<(usize, f32)> {
    room(480, 0.316, 0.3, 0.25, 11)
}

/// Whether the slot handed on what its mic heard. The left of the master carries the far slot at
/// cos(pi / 2), 6e-17 of it, so this allows a millionth; `Slot`'s own tests show off is bit for
/// bit.
fn untouched(run: &Run) -> bool {
    run.mic.iter().zip(&run.out).all(|(m, o)| (m - o).abs() <= 1e-6)
}

// With the canceller off the slot hands on its mic, as with any effect off.
#[test]
fn off_the_slot_hears_the_room_untouched() {
    let mut scene = Scene::new(backing(2 * SECOND), living_room());
    scene.echo_cancel = false;
    let run = scene.run();
    assert!(untouched(&run));
    assert_eq!(run.report.findings(), None);
}

/// The echo taken out over `[from, to)` seconds while `singer` sang: what the mic heard beyond
/// the singer, against what the slot handed on beyond them.
fn removed_under(run: &Run, singer: &[f32], from: f64, to: f64) -> f32 {
    let beyond = |signal: &[f32]| -> Vec<f32> {
        seconds(signal, from, to)
            .iter()
            .zip(seconds(singer, from, to))
            .map(|(s, v)| s - v)
            .collect()
    };
    db(rms(&beyond(&run.mic))) - db(rms(&beyond(&run.out)))
}

// A house mic in a living room, the singer silent for the first 8 s, singing from 8 to 14 s, and
// silent again to 20 s; the singer is in the master too and so in the room's echo. Tolerances:
// 15 dB out by the third second, 22 dB in each stretch without the singer once settled, 12 dB
// of the echo out while the singer sings over it, and the singer within half a decibel of how
// they sang. `aec:measure`'s GStreamer AEC3 removes 19 to 35 dB of a pure delay and leaves its
// singer 0.5 to 3 dB down; it has no room and no singer in its own echo.
#[test]
fn a_house_mic_hears_the_room_taken_out_and_the_singer_kept() {
    let frames = 20 * SECOND;
    let mut scene = Scene::new(backing(frames), living_room());
    let singer = at_db(talker(6 * SECOND, 180.0, 9), -20.0);
    scene.singer[8 * SECOND..14 * SECOND].copy_from_slice(&singer);
    let clean = scene.singer.clone();
    let run = scene.run();

    let third = removed(&run, 2.0, 3.0);
    let before = removed(&run, 3.0, 8.0);
    let while_sung = removed_under(&run, &clean, 9.0, 14.0);
    let after = removed(&run, 15.0, 20.0);
    let sung = db(rms(seconds(&run.out, 9.0, 14.0))) - db(rms(seconds(&clean, 9.0, 14.0)));
    let findings = run.report.findings().expect("found and adapted");
    println!(
        "removed {third:.1} dB in the third second, {before:.1} dB before the singer, \
         {while_sung:.1} dB under them, {after:.1} dB after; singer {sung:+.2} dB; {findings:?}"
    );
    assert!(third >= 15.0, "{third:.1} dB in the third second");
    assert!(before >= 22.0 && after >= 22.0, "{before:.1} dB before, {after:.1} dB after");
    assert!(while_sung >= 12.0, "{while_sung:.1} dB under the singer");
    assert!(sung.abs() <= 0.5, "the singer came out {sung:+.2} dB");
    let path = i64::from(findings.path_frames) - 480;
    assert!(path.abs() <= 16, "the path read {} frames, not 480", findings.path_frames);
}

// A singer following the melody they hear, a semitone's tenth sharp and 40 ms behind, sings the
// backing's own pitches at a lag the finder searches. Coherent for a moment at a time, it is
// never mistaken for the room: with no echo at all, nothing is found and nothing is taken out.
#[test]
fn a_singer_in_tune_with_the_backing_is_not_taken_for_its_echo() {
    let frames = 12 * SECOND;
    let mut scene = Scene::new(backing(frames), Vec::new());
    let along = at_db(talker(frames, 110.0 * 2f64.powf(10.0 / 1200.0), 3), -20.0);
    scene.singer[40 * 48..].copy_from_slice(&along[..frames - 40 * 48]);
    let run = scene.run();
    assert_eq!(run.report.findings(), None);
    assert!(untouched(&run));
}

// A phone's jitter buffer moves its delay by 5 ms at 6 s and says so. The canceller follows the
// move two blocks later, as its source reports it, without finding the path again: from 20 ms
// after the move the echo comes out within 3 dB of how it did before.
#[test]
fn a_source_that_moves_its_delay_keeps_its_echo_cancelled() {
    let mut scene = Scene::new(backing(10 * SECOND), living_room());
    scene.shift_at = 6 * SECOND;
    scene.shift = 240;
    let run = scene.run();
    let before = removed(&run, 5.0, 6.0);
    let after = removed(&run, 6.02, 7.0);
    println!("removed {before:.1} dB in the second before the move, {after:.1} dB after it");
    assert!(after >= before - 3.0, "{after:.1} dB after the move, {before:.1} dB before");
}

/// The RBJ peaking filter, a room's resonance on the way back to the mic.
fn peak(hz: f32, q: f32, gain_db: f32) -> [f32; 5] {
    let a = 10f32.powf(gain_db / 40.0);
    let w = std::f32::consts::TAU * hz / RATE as f32;
    let alpha = w.sin() / (2.0 * q);
    let a0 = 1.0 + alpha / a;
    [
        (1.0 + alpha * a) / a0,
        -2.0 * w.cos() / a0,
        (1.0 - alpha * a) / a0,
        -2.0 * w.cos() / a0,
        (1.0 - alpha / a) / a0,
    ]
}

/// A phone 80 ms round trip from the speakers with a room that rings at 1 kHz: 12 dB under unity
/// everywhere but there, where the loop is 3 dB over and runs away 37 dB a second. The room
/// hears the slot at half, since it is one side of the mono mix.
fn ringing_scene(frames: usize) -> Scene {
    let mut scene = Scene::new(at_db(pink_noise(frames, 7), -40.0), vec![(80 * 48, 0.5)]);
    scene.resonance = Some(peak(1000.0, 8.0, 15.0));
    let singer = at_db(talker(frames, 180.0, 9), -26.0);
    scene.singer.copy_from_slice(&singer);
    scene.echo_cancel = false;
    scene
}

/// The level, in dBFS, of the 1 kHz howl in `signal` over each 50 ms.
fn howl_levels(signal: &[f32]) -> Vec<f32> {
    let step = std::f32::consts::TAU * 1000.0 / RATE as f32;
    signal
        .as_chunks::<{ SECOND / 20 }>()
        .0
        .iter()
        .map(|chunk| {
            let (re, im) = chunk.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, s)| {
                (re + s * (step * i as f32).cos(), im + s * (step * i as f32).sin())
            });
            db(2.0 * (re * re + im * im).sqrt() / chunk.len() as f32)
        })
        .collect()
}

// The loop is the fixture's to prove: without the guard it runs to the clipper. With it, the
// guard catches the howl while it is still quiet and the room never hears it loud.
// Tolerances: caught within half a second of the howl passing -40 dBFS, and the howl never
// above -20 dBFS in ten seconds. `aec:measure` has no howl to compare.
#[test]
fn a_phone_that_starts_to_howl_is_caught() {
    let frames = 10 * SECOND;
    let unguarded = howl_levels(&ringing_scene(frames).run().out);
    assert!(unguarded.iter().any(|db| *db > -6.0), "the loop never ran away: {unguarded:?}");

    let mut scene = ringing_scene(frames);
    scene.howl_guard = true;
    let run = scene.run();
    let guarded = howl_levels(&run.out);
    let rose = guarded.iter().position(|db| *db > -40.0).expect("the howl started");
    let unguarded_rose = unguarded.iter().position(|db| *db > -40.0).expect("started");
    let loudest = guarded.iter().copied().fold(f32::MIN, f32::max);
    let catches = run.report.howl_catches();
    let caught_ms = run.first_catch.expect("caught") * 1000 / SECOND;
    let caught_db = unguarded[caught_ms / 50];
    println!(
        "the howl passed -40 dBFS at {} ms and was caught at {caught_ms} ms, at {caught_db:.1} \
         dBFS; at most {loudest:.1} dBFS guarded; {catches} catches in 10 s",
        rose * 50
    );
    assert_eq!(rose, unguarded_rose, "the same loop up to the catch");
    assert!(caught_ms - rose * 50 <= 500, "caught {} ms after it rose", caught_ms - rose * 50);
    assert!(loudest < -20.0, "the howl reached {loudest:.1} dBFS");
}

/// The frame in `[from, from + 100 ms)` where `signal` is loudest.
fn loudest_from(signal: &[f32], from: usize) -> usize {
    (from..from + SECOND / 10)
        .max_by(|a, b| signal[*a].abs().total_cmp(&signal[*b].abs()))
        .expect("frames to search")
}

// Engaged on a room it has learned, the canceller hands a click over on the very sample it
// entered: it subtracts an estimate and holds nothing back, so it costs the mic path no delay
// (docs/design.md 8.9). The click's own echo comes back 10 ms later and is taken out with the
// rest. Tolerances: 20 dB of the room out in the second before the click, and not one sample.
#[test]
fn an_engaged_canceller_hands_a_click_over_on_the_sample_it_entered() {
    let click = 6 * SECOND + 77;
    let mut scene = Scene::new(backing(7 * SECOND), living_room());
    scene.singer[click] = 0.5;
    let run = scene.run();
    let engaged = removed(&run, 5.0, 6.0);
    assert!(engaged >= 20.0, "{engaged:.1} dB out before the click");
    assert_eq!(loudest_from(&run.out, click), click);
}

// A seek fades the backing out, jumps 20 s on and fades it in: the reference jumps and the room
// does not. The canceller keeps what it learned, so from 100 ms after the seek it removes within
// 3 dB of the second before it.
#[test]
fn a_backing_seek_leaves_the_echo_cancelled() {
    let seek_at = 6 * SECOND;
    let mut scene = Scene::new(backing(30 * SECOND), living_room());
    scene.seek = Some((seek_at, 26 * SECOND as u64));
    scene.frames = 9 * SECOND;
    let run = scene.run();
    let before = removed(&run, 5.0, 6.0);
    let after = removed(&run, 6.1, 9.0);
    println!("removed {before:.1} dB before the seek, {after:.1} dB after it");
    assert!(after >= before - 3.0, "{after:.1} dB after the seek, {before:.1} dB before");
}

// A phone joins at 6 s into a slot that has had the canceller on and heard nothing: the slot is
// reset, as a new sender resets it, and the room starts. The canceller finds it from scratch, as
// at the start of a song: 20 dB out from 3 s after the join, and the path where the room is.
#[test]
fn a_phone_that_joins_mid_song_has_its_echo_found() {
    let mut scene = Scene::new(backing(12 * SECOND), living_room());
    scene.joins_at = 6 * SECOND;
    let run = scene.run();
    assert!(seconds(&run.out, 0.0, 6.0).iter().all(|s| s.abs() < 1e-6), "silent before");
    let after = removed(&run, 9.0, 12.0);
    let path = run.report.findings().expect("found").path_frames;
    println!("removed {after:.1} dB from 3 s after the join; path {path} frames");
    assert!(after >= 20.0, "{after:.1} dB after the join");
    assert!(path.abs_diff(480) <= 16, "the path read {path} frames");
}

/// A room for a swap test: one mic, its own ring of what was played, from its graph's first block.
fn room_for(path: Vec<(usize, f32)>, frames: usize) -> (Room, Producer<f32>, Consumer<f32>) {
    let (played, played_out) = RingBuffer::new(4 * BLOCK);
    let (mic_in, mic_out) = RingBuffer::new(frames + BLOCK);
    let room = Room {
        played: played_out,
        heard: Vec::with_capacity(frames + BLOCK),
        path,
        resonance: None,
        singer: Vec::new(),
        floor: at_db(pink_noise(SECOND, 99), -60.0),
        mic: mic_in,
        at: 0,
        latency: 0,
        shift_at: usize::MAX,
        shift: 0,
        joins_at: 0,
    };
    (room, played, mic_out)
}

// The next song's mixer swaps in at 4 s, with its own slot and its own empty echo history, while
// the room still rings with the old one through the swap fade. Its canceller finds the room on
// its own: 20 dB out from 3 s after the swap. The swap, its fade and the retire all run under
// the allocation guard.
#[test]
fn a_swapped_in_mixer_finds_the_room_again() {
    let (frames, swap_at) = (11 * SECOND, 4 * SECOND);
    let far = backing(frames);
    let graph_for = |room: Room, far: &[f32]| {
        let far = Clip { samples: far.to_vec(), at: 0 };
        Box::new(Graph::new(vec![Slot::new(Box::new(room)), Slot::new(Box::new(far))]))
    };
    let (room_a, mut played_a, _) = room_for(living_room(), frames);
    let (room_b, mut played_b, mut mic_b) = room_for(living_room(), frames);
    let (first, mut next) = (graph_for(room_a, &far), Some(graph_for(room_b, &far[swap_at..])));
    let (mut audio, mut controls) = AudioThread::new();
    controls.params.update(|p| {
        p.slots[0].pan = -1.0;
        p.slots[0].effects.echo_cancel = EchoCancelParams { enabled: true };
        p.slots[1].pan = 1.0;
    });
    controls.commands.push(Command::Swap { id: 1, graph: first }).ok().expect("room");
    let mut rendered = 0usize;
    let mut driver = OfflineDriver::new(Box::new(move |out: Stereo<'_>| {
        let Stereo { left, right } = out;
        audio.render(Stereo { left: &mut *left, right: &mut *right });
        for (l, r) in left.iter().zip(right.iter()) {
            let played = 0.5 * (l + r);
            let _ = played_a.push(played);
            if rendered >= swap_at {
                let _ = played_b.push(played);
            }
        }
        rendered += BLOCK;
    }));
    let mut out = Vec::with_capacity(frames);
    let mut interleaved = [0.0; 2 * BLOCK];
    while out.len() + BLOCK <= frames {
        if out.len() == swap_at {
            controls
                .commands
                .push(Command::Swap { id: 2, graph: next.take().expect("once") })
                .ok()
                .expect("room");
        }
        assert_no_alloc(|| driver.pump_into(&mut interleaved));
        out.extend(interleaved.iter().step_by(2));
    }
    let mic: Vec<f32> = std::iter::from_fn(|| mic_b.pop().ok()).collect();
    let tail = &out[swap_at..];
    let at = |s: usize| s * SECOND;
    let after = db(rms(&mic[at(3)..at(7)])) - db(rms(&tail[at(3)..at(7)]));
    println!("removed {after:.1} dB from 3 s after the swap");
    assert!(after >= 20.0, "{after:.1} dB after the swap");
}

// Turned on, learned and turned off again, the canceller hands the slot over bit for bit from
// the block it went off: off forgets the path and does no work, however long it was on.
#[test]
fn a_canceller_turned_off_after_learning_is_bit_for_bit_again() {
    let frames = 4 * SECOND;
    let far = backing(frames);
    let path = living_room();
    let echo = kernels::fixtures::through(&far, &path);
    let mut slot = Slot::new(Box::new(Clip { samples: echo.clone(), at: 0 }));
    let report = slot.echo_report();
    let mut history = EchoHistory::new();
    let (mut events, _drain) = event_ring();
    let mut params = SlotParams { pan: -1.0, ..SlotParams::default() };
    params.effects.echo_cancel = EchoCancelParams { enabled: true };
    let mut learned = false;
    for (index, (played, heard)) in
        far.as_chunks::<BLOCK>().0.iter().zip(echo.as_chunks::<BLOCK>().0).enumerate()
    {
        let on = index * BLOCK < 3 * SECOND;
        params.effects.echo_cancel.enabled = on;
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        assert_no_alloc(|| {
            let shared = Shared {
                any_solo: false,
                cue: crate::slot::cue::Cue::default(),
                history: &history,
            };
            slot.render(
                0,
                &params,
                shared,
                &mut Stereo { left: &mut left, right: &mut right },
                &mut events,
            );
            history.push(played, played);
        });
        learned |= report.findings().is_some();
        if !on {
            assert!(
                left.iter().zip(heard).all(|(o, h)| o.to_bits() == h.to_bits()),
                "block {index}"
            );
        }
    }
    assert!(learned, "the canceller learned the room before it went off");
    assert_eq!(report.findings(), None);
}

/// One room with correction at full strength and the voice two formant semitones bright, a loop
/// 6 dB over unity at 1 kHz through an 80 ms trip: whether the guard ever caught it, and the
/// loudest the howl came out at, in dBFS. `seed` sets the singer's pitch, from 140 to 240 Hz, the
/// singer and the room's noise.
fn correcting_room(seed: u32) -> (bool, f32) {
    let frames = 10 * SECOND;
    let mut scene =
        Scene::new(at_db(pink_noise(frames, 7 + seed / 6), -40.0), vec![(80 * 48, 0.7)]);
    scene.resonance = Some(peak(1000.0, 8.0, 15.0));
    let singer = at_db(talker(frames, 140.0 + 20.0 * f64::from(seed % 6), 9 + seed), -26.0);
    scene.singer.copy_from_slice(&singer);
    scene.echo_cancel = false;
    scene.effects = EffectParams {
        corrector: CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 },
        voice: VoiceParams { enabled: true, formant_semitones: 2.0 },
        ..EffectParams::default()
    };
    scene.howl_guard = true;
    let run = scene.run();
    let loudest = howl_levels(&run.out).into_iter().fold(f32::MIN, f32::max);
    (run.first_catch.is_some(), loudest)
}

// With correction and a +2 voice on, a loop 6 dB over unity runs away, and the corrector moves the
// howl as it grows. One room is one draw of a chaotic loop: the singer, the noise and each
// platform's rounding decide to a tenth of a dB when the guard sees it, so a single room passes or
// fails by chance. The guard is held to a rate over 96 seeded rooms instead. Over 480 rooms on
// macOS, the guard before this one let 46 % of them pass -20 dBFS and never caught 1.7 %; this one
// lets 20 % pass and catches every one. Of these 96 it lets 17 pass against 38. The bound, 33 of
// 96, sits 3.6 standard deviations over 20 % and 2.2 under the old guard's 46 %. One room in 96
// may go uncaught: 1 of the 480 does with a corrector that reads every 5 ms.
#[test]
fn the_guard_catches_howls_with_the_corrector_and_the_voice_on() {
    const ROOMS: u32 = 96;
    const MOST_LOUD: usize = 33;
    const MOST_UNCAUGHT: usize = 1;
    const THREADS: u32 = 8;
    let rooms: Vec<(bool, f32)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..THREADS)
            .map(|worker| {
                scope.spawn(move || {
                    (1..=ROOMS)
                        .filter(|seed| seed % THREADS == worker)
                        .map(correcting_room)
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers.into_iter().flat_map(|worker| worker.join().expect("a room panicked")).collect()
    });
    let loud = rooms.iter().filter(|(_, loudest)| *loudest > -20.0).count();
    let uncaught = rooms.iter().filter(|(caught, _)| !caught).count();
    println!("{loud} of {ROOMS} rooms louder than -20 dBFS, {uncaught} never caught");
    assert!(uncaught <= MOST_UNCAUGHT, "{uncaught} rooms never caught");
    assert!(loud <= MOST_LOUD, "{loud} of {ROOMS} rooms louder than -20 dBFS");
}
