//! A capture slot at its edges, through the real audio thread and graph on the offline driver
//! and under the allocation guard. Its input is a fake device that delivers a period at a time on
//! its own clock, to every mixer's stream on it, as each mixer opens its own.

use std::f64::consts::TAU;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use assert_no_alloc::assert_no_alloc;
use kernels::fixtures::{at_db, between, db, pink_noise, rms, room, talker};

use super::capture::{capture_voice, input_period_for};
use super::capture_producer::CaptureProducer;
use super::{CaptureVoice, SharedStats, SlotStats};
use crate::audio_thread::{AudioThread, Controls};
use crate::block::{BLOCK, RATE};
use crate::command::{Command, Retired};
use crate::driver::{Movable, OfflineDriver, Render};
use crate::graph::Graph;
use crate::params::{EchoCancelParams, Params};
use crate::slot::Slot;
use crate::slot::source::Source;

const SECOND: u64 = RATE as u64;

fn tone(hz: f64, k: u64) -> f32 {
    (0.25 * (TAU * hz * k as f64 / f64::from(RATE)).sin()) as f32
}

/// A slot that plays a tone from the song's first frame, to show the rest of the room plays on.
struct Tone {
    hz: f64,
    at: u64,
}

impl Source for Tone {
    fn pull(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = tone(self.hz, self.at);
            self.at += 1;
        }
    }
}

/// Hands over `samples`, then silence: the far end an echo test plays.
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

/// A fake input, `ppm` off the output's clock, that starts at output frame `start` and delivers
/// `period` frames whenever its clock has them.
struct Device {
    streams: Vec<CaptureProducer>,
    period: u64,
    ppm: i64,
    start: u64,
    delivered: u64,
    scratch: Vec<f32>,
}

impl Device {
    fn new(period: u64, ppm: i64) -> Self {
        let scratch = vec![0.0; period as usize];
        Self { streams: Vec::new(), period, ppm, start: 0, delivered: 0, scratch }
    }

    /// One mixer's stream on this input, as `createMixer` opens one per capture slot, under an
    /// output that takes a block a wake.
    fn open(&mut self) -> (CaptureVoice, std::sync::Arc<SharedStats>) {
        self.open_under(BLOCK as u64)
    }

    /// The same under an output whose callback takes `callback` frames at a wake.
    fn open_under(&mut self, callback: u64) -> (CaptureVoice, std::sync::Arc<SharedStats>) {
        let (producer, voice) = capture_voice();
        self.streams.push(producer);
        let voice =
            voice.for_input(self.period as u32, 0, callback as u32).expect("it takes these");
        let stats = voice.shared_stats();
        (voice, stats)
    }

    /// Delivers each whole period the input's clock has run by `frames` of output; `mic` is what
    /// the mic heard at each of its own frames.
    fn run_to(&mut self, frames: u64, mic: impl Fn(u64) -> f32) {
        let Some(ran) = frames.checked_sub(self.start) else { return };
        let due = (u128::from(ran) * (1_000_000 + self.ppm) as u128 / 1_000_000) as u64;
        let period = self.period as usize;
        while self.delivered + self.period <= due {
            for (k, sample) in self.scratch.iter_mut().enumerate() {
                *sample = mic(self.delivered + k as u64);
            }
            for stream in &mut self.streams {
                stream.push(&self.scratch[..period]);
            }
            self.delivered += self.period;
        }
    }
}

/// The real audio thread on the offline driver, keeping both channels of every block.
struct Rig {
    driver: OfflineDriver,
    controls: Controls,
    left: Vec<f32>,
    right: Vec<f32>,
    block: [f32; 2 * BLOCK],
}

impl Rig {
    fn new(slots: Vec<Slot>, setup: impl FnOnce(&mut Params), seconds: u64) -> Self {
        let (audio, mut controls) = AudioThread::new();
        let graph = Box::new(Graph::new(slots));
        controls.commands.push(Command::Swap { id: 1, graph }).ok().expect("room");
        controls.params.update(setup);
        let frames = (seconds * SECOND) as usize;
        Self {
            driver: OfflineDriver::new(audio.into_render()),
            controls,
            left: Vec::with_capacity(frames),
            right: Vec::with_capacity(frames),
            block: [0.0; 2 * BLOCK],
        }
    }

    fn frames(&self) -> u64 {
        self.left.len() as u64
    }

    fn pump(&mut self) {
        let Self { driver, block, .. } = self;
        assert_no_alloc(|| driver.pump_into(block));
        self.left.extend(self.block.iter().step_by(2));
        self.right.extend(self.block.iter().skip(1).step_by(2));
    }
}

/// Left and right: the capture slot hard left, the rest of the room hard right.
fn split(p: &mut Params) {
    p.slots[0].pan = -1.0;
    p.slots[1].pan = 1.0;
}

/// The lag at which `played` from `from` on is `heard` exactly, if one is.
fn exact_lag(played: &[f32], from: usize, heard: impl Fn(u64) -> f32) -> Option<u64> {
    (0..48 * BLOCK as u64).find(|lag| {
        played[from..].iter().enumerate().all(|(i, sample)| {
            let k = (from + i) as u64;
            k >= *lag && (sample - heard(k - lag)).abs() < 1e-6
        })
    })
}

/// A house mic that starts delivering three seconds into the song, as one switched on or plugged
/// in late does: silence until then and nothing counted against it, then once the slot has
/// primed and settled, a third of a second, the mic sample for sample. The other slot plays on,
/// exact, throughout.
#[test]
fn an_input_that_starts_mid_song_plays_from_its_first_periods() {
    let mut device = Device::new(128, 0);
    device.start = 3 * SECOND;
    let (voice, stats) = device.open();
    let slots = vec![Slot::new(Box::new(voice)), Slot::new(Box::new(Tone { hz: 660.0, at: 0 }))];
    let mut rig = Rig::new(slots, split, 6);
    while rig.frames() < 6 * SECOND {
        device.run_to(rig.frames() + BLOCK as u64, |k| tone(440.0, k));
        rig.pump();
    }
    let start = (3 * SECOND) as usize;
    assert!(rig.left[BLOCK * 3..start].iter().all(|s| s.abs() < 1e-6), "silent before the mic");
    let from = start + RATE as usize / 2;
    let lag = exact_lag(&rig.left, from, |k| tone(440.0, k.saturating_sub(3 * SECOND)));
    assert!(lag.is_some(), "the mic plays exactly once it delivers");
    let other = exact_lag(&rig.right, 3 * BLOCK, |k| tone(660.0, k));
    assert_eq!(other, Some(0), "the rest of the room never moved");
    assert_eq!(stats.read().concealed, 0, "waiting for a first delivery is no gap");
}

/// The input is lost three seconds in and never delivers again, as an unplugged USB mic's stream
/// stops. The slot goes silent once its ring runs out and counts every pull it could not fill;
/// the song plays on, and the rest of the room is exact throughout.
#[test]
fn an_input_lost_mid_song_goes_silent_and_the_song_plays_on() {
    let mut device = Device::new(128, 0);
    let (voice, stats) = device.open();
    let slots = vec![Slot::new(Box::new(voice)), Slot::new(Box::new(Tone { hz: 660.0, at: 0 }))];
    let mut rig = Rig::new(slots, split, 6);
    let lost = 3 * SECOND;
    while rig.frames() < 6 * SECOND {
        if rig.frames() < lost {
            device.run_to(rig.frames() + BLOCK as u64, |k| tone(440.0, k));
        }
        rig.pump();
    }
    let gone = lost as usize + 1024;
    assert!(rig.left[gone..].iter().all(|s| s.abs() < 1e-6), "silent once the ring ran out");
    assert_eq!(exact_lag(&rig.right, 3 * BLOCK, |k| tone(660.0, k)), Some(0));
    let blocks_since = (6 * SECOND - lost) / BLOCK as u64;
    let concealed = u64::from(stats.read().concealed);
    assert!(concealed + 3 >= blocks_since && concealed <= blocks_since, "{concealed}");
}

/// The next song's mixer, with its own stream on the same mic, is armed while this one captures
/// and swapped in two seconds in. Its first pull drops what its ring took while armed, so once it
/// has primed and settled, a third of a second after the swap, it plays the live mic exactly and a
/// block or two behind it, not 85 ms. The old graph comes back on the retired ring and is dropped
/// here.
#[test]
fn a_mixer_swapped_in_while_capturing_opens_on_the_live_mic() {
    let mut device = Device::new(128, 0);
    let (first, _) = device.open();
    let (next, next_stats) = device.open();
    let mut rig = Rig::new(vec![Slot::new(Box::new(first))], |p| p.slots[0].pan = -1.0, 5);
    let mut next = Some(Box::new(Graph::new(vec![Slot::new(Box::new(next))])));
    let swap_at = 2 * SECOND;
    while rig.frames() < 5 * SECOND {
        if rig.frames() == swap_at {
            let graph = next.take().expect("once");
            rig.controls.commands.push(Command::Swap { id: 2, graph }).ok().expect("room");
        }
        device.run_to(rig.frames() + BLOCK as u64, |k| tone(440.0, k));
        rig.pump();
    }
    let from = swap_at as usize + RATE as usize / 2;
    let lag = exact_lag(&rig.left, from, |k| tone(440.0, k)).expect("exact after the swap");
    assert!(lag <= 2 * BLOCK as u64, "the next mixer plays the live mic: {lag} frames behind");
    assert_eq!(next_stats.read().concealed, 0);
    let mut retired = 0;
    while let Some(item) = rig.controls.retired.pop() {
        retired += usize::from(matches!(item, Retired::Graph(_)));
    }
    assert!(retired >= 2, "the silent graph and the first mixer came back");
}

/// The far end through a living room into a house mic on a fake input `period` frames at a time,
/// `ppm` off the output's clock, with the canceller on, for ten seconds. Returns the echo taken out
/// in each second and the canceller's findings at the end.
fn echo_through_capture(period: u64, ppm: i64) -> (Vec<f32>, Option<kernels::EchoFindings>) {
    let seconds = 10;
    let frames = (seconds * SECOND) as usize;
    let voice_far = at_db(talker(frames, 110.0, 3), -18.0);
    let bed = at_db(pink_noise(frames, 7), -26.0);
    let far: Vec<f32> = voice_far.iter().zip(&bed).map(|(v, b)| v + b).collect();
    let path = room(480, 0.316, 0.3, 0.25, 11);
    let floor = at_db(pink_noise(RATE as usize, 99), -60.0);
    let mut device = Device::new(period, ppm);
    let (voice, _) = device.open();
    let mic_slot = Slot::new(Box::new(voice));
    let report = mic_slot.echo_report();
    let slots = vec![mic_slot, Slot::new(Box::new(Clip { samples: far, at: 0 }))];
    let echo_cancel = |p: &mut Params| {
        split(p);
        p.slots[0].effects.echo_cancel = EchoCancelParams { enabled: true };
    };
    let mut rig = Rig::new(slots, echo_cancel, seconds);
    let mut mic = Vec::with_capacity(frames + RATE as usize);
    // The room's echo of what it played, at each frame of the output's clock
    let mut echo = Vec::with_capacity(frames + RATE as usize);
    let nearest = path.iter().map(|(lag, _)| *lag).min().expect("a path");
    while rig.frames() < seconds * SECOND {
        let (left, right) = (&rig.left, &rig.right);
        let played = |i: usize| left.get(i).zip(right.get(i)).map_or(0.0, |(l, r)| 0.5 * (l + r));
        // An echo is at least the nearest lag late, so it is known that far past what has played
        while echo.len() < rig.frames() as usize + nearest {
            let at = echo.len();
            let heard: f32 = path
                .iter()
                .filter(|(lag, _)| at >= *lag)
                .map(|(lag, gain)| gain * played(at - lag))
                .sum();
            echo.push(heard);
        }
        // The mic's own frame `k` falls at this moment on the output's clock
        let heard = |k: u64| {
            let t = k as f64 * 1_000_000.0 / (1_000_000 + ppm) as f64;
            between(&echo, t) + floor[k as usize % floor.len()]
        };
        let before = device.delivered;
        device.run_to(rig.frames(), heard);
        mic.extend((before..device.delivered).map(heard));
        rig.pump();
    }
    let removed = (0..seconds as usize - 1)
        .map(|second| {
            let at =
                |signal: &[f32]| rms(&signal[second * RATE as usize..(second + 1) * RATE as usize]);
            db(at(&mic)) - db(at(&rig.left))
        })
        .collect();
    (removed, report.findings())
}

/// A house mic on the output's clock, as one on the same interface is, at a 128- or a 480-frame
/// input period: the canceller takes 15 dB of the room's echo out in the third second and 20 dB in
/// every second from the fifth, near what the slot test of a house mic asks, though the capture
/// slot is silent its first third of a second. The path it finds is the room's 480 frames and the
/// slot's own wait.
#[test]
fn a_capture_slot_on_the_outputs_clock_has_its_echo_cancelled() {
    for (period, wait) in [(128u64, 144u32), (480, 592)] {
        let (removed, findings) = echo_through_capture(period, 0);
        println!("{period} frames: {removed:.1?} dB out, second by second; {findings:?}");
        assert!(removed[2] >= 15.0, "{period}: {removed:.1?}");
        assert!(removed[4..].iter().all(|db| *db >= 20.0), "{period}: {removed:.1?}");
        let path = findings.expect("found and adapted").path_frames;
        assert_eq!(path, 480 + wait, "{period}: the path the canceller found");
    }
}

/// A house mic on its own clock, 100 or 200 ppm either way, as a USB mic beside the speakers is:
/// the echo's delay slides a few frames a second, and the slot splices a frame back a block at a
/// time when the ring's fill slips a step. The canceller follows the slide at the rate it learns
/// and each splice the slot reports. So from the fifth second it takes out within 3 dB of what it
/// takes from the same mic on the output's clock, at a 128-frame input period: measured, 0.9 dB at
/// most. A 480-frame input slips 32 frames every 6.7 s at 100 ppm, and the second holding its
/// splices comes out up to 3.3 dB under, so it is held to 4.
#[test]
fn a_capture_slot_on_its_own_clock_has_its_echo_cancelled_as_on_the_outputs() {
    for (period, within) in [(128u64, 3.0), (480, 4.0)] {
        let (same, _) = echo_through_capture(period, 0);
        for ppm in [100i64, -100, 200, -200] {
            let (removed, findings) = echo_through_capture(period, ppm);
            println!("{period} frames, {ppm} ppm: {removed:.1?} dB out against {same:.1?}");
            assert!(findings.is_some(), "{period}, {ppm}: found and adapted");
            for second in 4..removed.len() {
                let case = format!("{period} frames, {ppm} ppm, second {}", second + 1);
                assert!(removed[second] > same[second] - within, "{case}: {removed:.1?}");
            }
        }
    }
}

/// A 480-frame output callback, WASAPI shared's on the laptop in decision 0003, renders three or
/// four blocks back to back, and the input delivers nothing between them. So before the first of
/// them the ring holds up to three blocks more than a block-a-wake output would leave, all of it
/// healthy. Under a 128- and a 256-frame input the slot plays the mic exactly and drops nothing:
/// no gap and no move.
#[test]
fn grouped_pulls_under_a_long_output_callback_drop_nothing_healthy() {
    let callback = 480u64;
    for input in [128u64, 256] {
        let mut device = Device::new(input, 0);
        let (voice, stats) = device.open_under(callback);
        let mut rig = Rig::new(vec![Slot::new(Box::new(voice))], |p| p.slots[0].pan = -1.0, 6);
        let mut wakes = 0;
        while rig.frames() < 6 * SECOND {
            device.run_to(wakes * callback, |k| tone(440.0, k));
            wakes += 1;
            while rig.frames() < wakes * callback {
                rig.pump();
            }
        }
        let read = stats.read();
        assert_eq!((read.concealed, read.resyncs), (0, 0), "{input}: {read:?}");
        let lag = exact_lag(&rig.left, SECOND as usize, |k| tone(440.0, k));
        assert!(lag.is_some(), "{input}: the mic plays exactly");
    }
}

/// The output wakes late once a second and catches up, pulling two callbacks' blocks at one wake,
/// as WASAPI does after a late wake and the timer clock does when it falls behind. The input
/// delivered all the while, so before the catch-up the ring holds a callback more than usual, all of
/// it healthy. Under a block-a-wake and a 480-frame callback, with a 128-frame input, the slot drops
/// none of it and never runs short.
#[test]
fn an_output_that_catches_up_after_a_late_wake_drops_nothing() {
    for callback in [128u64, 480] {
        let mut device = Device::new(128, 0);
        let (voice, stats) = device.open_under(callback);
        let mut rig = Rig::new(vec![Slot::new(Box::new(voice))], |p| p.slots[0].pan = -1.0, 6);
        let mut wakes = 0;
        while rig.frames() < 6 * SECOND {
            wakes += 1;
            let late = wakes % (SECOND / callback) == 0;
            if late {
                wakes += 1;
            }
            // A wake hears what the input delivered by when it starts, late or not
            device.run_to((wakes - 1) * callback, |k| tone(440.0, k));
            while rig.frames() < wakes * callback {
                rig.pump();
            }
        }
        let read = stats.read();
        assert_eq!((read.concealed, read.resyncs), (0, 0), "{callback}: {read:?}");
    }
}

/// A 128-frame mic under 128-frame callbacks for 3 s, then the output follower moves the song to a
/// device whose callback takes `to` frames, after 20 ms with no output while it reopens. As the
/// follower does, `Movable::moved` runs before the new stream and the new period is in the audio
/// thread's cell before its first block, so the first block under the new callback already hears
/// it. The mic runs on through the gap. Returns the left channel, the gap, and the slot's counters
/// at the end and a second after the move.
fn moved_to(to: u32) -> (Vec<f32>, u64, SlotStats, SlotStats) {
    let mut device = Device::new(128, 0);
    let (voice, stats) = device.open_under(128);
    let (mut audio, mut controls) = AudioThread::new();
    let graph = Box::new(Graph::new(vec![Slot::new(Box::new(voice))]));
    controls.commands.push(Command::Swap { id: 1, graph }).ok().expect("room");
    controls.params.update(|p| p.slots[0].pan = -1.0);
    let moving = Arc::new(AtomicBool::new(false));
    let cell = audio.callback_frames();
    let render: Render = Box::new({
        let (moving, cell) = (Arc::clone(&moving), Arc::clone(&cell));
        move |out| {
            // Between streams, off the audio thread: `moved`, then the new stream's period as its
            // start returns; here, just before its first block
            if moving.swap(false, Ordering::Relaxed) {
                Movable::moved(&mut audio);
                cell.store(to, Ordering::Release);
            }
            audio.render(out);
        }
    });
    let mut driver = OfflineDriver::new(render);
    let (mut left, mut block) = (Vec::with_capacity(9 * SECOND as usize), [0.0; 2 * BLOCK]);
    let (move_at, gap) = (3 * SECOND, 960);
    let mut after_a_second = None;
    // `wall` is the input's clock; the output renders up to it, less the gap once it has moved
    let (mut callback, mut wall, mut lost) = (128, 0, 0);
    while (left.len() as u64) < 8 * SECOND {
        if lost == 0 && left.len() as u64 >= move_at {
            (wall, lost, callback) = (wall + gap, gap, u64::from(to));
            moving.store(true, Ordering::Relaxed);
        }
        if after_a_second.is_none() && left.len() as u64 >= move_at + SECOND {
            after_a_second = Some(stats.read());
        }
        device.run_to(wall, |k| tone(440.0, k));
        wall += callback;
        while (left.len() as u64) + lost < wall {
            assert_no_alloc(|| driver.pump_into(&mut block));
            left.extend(block.iter().step_by(2));
        }
    }
    (left, gap, after_a_second.expect("ran past it"), stats.read())
}

/// A move from 128-frame to 480-frame callbacks, and to 2048, as some Bluetooth outputs grant,
/// under a 128-frame mic. The slot primes and settles again, silent for a third of a second, under
/// the new callback's headroom, then plays the mic exactly and drops nothing: no gap and no move.
#[test]
fn an_output_that_moves_to_a_longer_callback_drops_nothing_healthy() {
    for to in [480, 2048] {
        let (left, gap, after_a_second, end) = moved_to(to);
        let from = (3 * SECOND + SECOND / 2) as usize;
        // The input's clock ran on through the gap, so its frame for an output frame is a gap later
        let lag = exact_lag(&left, from, |k| tone(440.0, k + gap));
        assert!(lag.is_some(), "{to}: the mic plays exactly under the new callback");
        assert_eq!((end.concealed, end.resyncs), (0, 0), "{to}: {end:?}");
        assert_eq!(end, after_a_second, "{to}");
    }
}

/// A move to a callback the ring cannot hold beside a 128-frame mic, 4096 frames: it cannot be
/// refused, so the slot goes silent until the next move. It counts that once as a resync, reads 0
/// as its playout delay so a host can see why, and stays put: no priming that runs dry, no gap
/// counted, nothing moving in its counters after.
#[test]
fn an_output_with_a_callback_the_ring_cannot_hold_leaves_the_slot_silent_once() {
    let (left, _, after_a_second, end) = moved_to(4096);
    let from = (3 * SECOND) as usize + 4096;
    assert!(left[from..].iter().all(|s| s.abs() < 1e-6), "silent under it");
    assert_eq!((end.concealed, end.resyncs), (0, 1), "{end:?}");
    assert_eq!(end.playout_delay_ms, 0.0);
    assert_eq!(end, after_a_second, "nothing moves once it is silent");
}

/// The host hands a playing house mic to the next singer with `resetSlot`, three seconds in. It is
/// the same device, so the slot plays the mic on, sample for sample across the reset, with no gap
/// and no move; only its corrector starts again. A move still re-primes it, as the move tests show.
#[test]
fn reset_slot_on_a_playing_capture_slot_keeps_playing_the_mic() {
    let mut device = Device::new(128, 0);
    let (voice, stats) = device.open();
    let mut rig = Rig::new(vec![Slot::new(Box::new(voice))], |p| p.slots[0].pan = -1.0, 6);
    while rig.frames() < 6 * SECOND {
        if rig.frames() == 3 * SECOND {
            rig.controls.commands.push(Command::ResetSlot { slot: 0 }).ok().expect("room");
        }
        device.run_to(rig.frames() + BLOCK as u64, |k| tone(440.0, k));
        rig.pump();
    }
    let lag = exact_lag(&rig.left, SECOND as usize, |k| tone(440.0, k));
    assert!(lag.is_some(), "the mic plays exactly from before the reset to the end");
    let read = stats.read();
    assert_eq!((read.concealed, read.resyncs), (0, 0), "{read:?}");
}

/// Under a 2048-frame output callback, which some Bluetooth outputs grant, a mic that grants
/// whatever it is asked for is asked for a period the ring holds beside that callback, 896 frames,
/// not the callback's 2048. So it opens, and plays the mic exactly with no gap and no move.
#[test]
fn under_a_2048_frame_output_a_mic_is_asked_for_a_period_it_can_play_at() {
    let callback = 2048u64;
    let asked = u64::from(input_period_for(callback as u32));
    assert_eq!(asked, 896);
    assert_eq!(input_period_for(128), 128, "a short callback asks for its own period");
    assert_eq!(input_period_for(480), 480);
    let mut device = Device::new(asked, 0);
    let (voice, stats) = device.open_under(callback);
    let mut rig = Rig::new(vec![Slot::new(Box::new(voice))], |p| p.slots[0].pan = -1.0, 6);
    let mut wakes = 0;
    while rig.frames() < 6 * SECOND {
        device.run_to(wakes * callback, |k| tone(440.0, k));
        wakes += 1;
        while rig.frames() < wakes * callback {
            rig.pump();
        }
    }
    let read = stats.read();
    assert_eq!((read.concealed, read.resyncs), (0, 0), "{read:?}");
    let lag = exact_lag(&rig.left, 2 * SECOND as usize, |k| tone(440.0, k));
    assert!(lag.is_some(), "the mic plays exactly");
}
