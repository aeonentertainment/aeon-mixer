//! The backing path of docs/design.md 8.5: stems in memory, one playhead, stem gains, fades, the
//! live key, and the render job that decodes and pre-renders the stems on the worker thread.
//! Everything here is built on the control or worker thread; `Backing::render` and `steer` run
//! on the audio thread and allocate nothing.

mod backing_error;
mod decode;
mod live_key;
mod planar;
mod playhead;
mod prerender;
mod ramp;
mod render_job;
mod stems;
#[cfg(test)]
mod test_support;
mod transport;

pub use backing_error::BackingError;
use decode::decode;
pub use live_key::{CROSSFADE_FRAMES, LiveKey};
pub use planar::Planar;
pub use playhead::{Playhead, SEEK_FADE_FRAMES};
use prerender::prerender;
use ramp::Ramp;
pub use render_job::{RenderJob, StemFiles};
pub use stems::Stems;
pub use transport::Transport;

use crate::block::{BLOCK, RATE, Stereo};
use crate::command::EventProducer;
use crate::params::StemGains;

/// 50 ms: the stem gain ramp.
pub const STEM_RAMP_FRAMES: u32 = RATE / 20;

pub struct Backing {
    stems: Stems,
    armed_semitones: i8,
    playhead: Playhead,
    instrumental: Ramp,
    guide: Ramp,
    live_key: LiveKey,
    fades: [f32; BLOCK],
    left: [f32; BLOCK],
    right: [f32; BLOCK],
}

impl Backing {
    /// Built on the control thread from a render job's stems, which were rendered at
    /// `armed_semitones`. The live key's stretchers are allocated here.
    pub fn new(stems: Stems, armed_semitones: i8) -> Self {
        Self {
            playhead: Playhead::new(stems.frames),
            stems,
            armed_semitones,
            instrumental: Ramp::new(STEM_RAMP_FRAMES),
            guide: Ramp::new(STEM_RAMP_FRAMES),
            live_key: LiveKey::new(),
            fades: [0.0; BLOCK],
            left: [0.0; BLOCK],
            right: [0.0; BLOCK],
        }
    }

    /// Takes a backing command and acks it, now or when it lands.
    pub fn steer(&mut self, id: u32, transport: Transport, events: &mut EventProducer) {
        let playhead = &mut self.playhead;
        match transport {
            Transport::Start => playhead.start(id, events),
            Transport::Seek { to_frame } => playhead.seek(id, to_frame, events),
            Transport::Pause => playhead.pause(id, events),
            Transport::Resume => playhead.resume(id, events),
            Transport::StopAt { at_frame, fade_frames } => {
                playhead.stop_at(id, at_frame, fade_frames, events);
            }
            Transport::SetKey { semitones } => {
                self.live_key.set(semitones);
                playhead.ack(id, events);
            }
        }
    }

    /// Adds one block of the stems into `out`. Allocation-free.
    pub fn render(&mut self, gains: &StemGains, out: &mut Stereo<'_>, events: &mut EventProducer) {
        let read = self.playhead.next_block(&mut self.fades, events);
        if read.is_none() && !self.playhead.is_paused() {
            return;
        }
        self.mix(read, gains);
        self.live_key.process(&mut self.left, &mut self.right);
        self.playhead.set_latency(self.live_key.latency_frames() as u64);
        for (sum, sample) in out.left.iter_mut().zip(&self.left) {
            *sum += sample;
        }
        for (sum, sample) in out.right.iter_mut().zip(&self.right) {
            *sum += sample;
        }
    }

    /// The backing frame the room hears now, which every ack carries.
    pub fn position_frames(&self) -> u64 {
        self.playhead.position()
    }

    /// The armed key plus the live key the room hears: the shift every corrector's guide follows.
    pub fn semitones(&self) -> i8 {
        self.armed_semitones.saturating_add(self.live_key.semitones())
    }

    /// Sums the stems from `read` through their gain ramps and the playhead's fades, or silence
    /// while the playhead holds.
    fn mix(&mut self, read: Option<u64>, gains: &StemGains) {
        self.instrumental.aim(gains.instrumental);
        self.guide.aim(gains.guide);
        let Some(start) = read else {
            self.left.fill(0.0);
            self.right.fill(0.0);
            return;
        };
        let start = usize::try_from(start).unwrap_or(usize::MAX);
        for i in 0..BLOCK {
            let frame = start.saturating_add(i);
            let instrumental = self.instrumental.next();
            let guide = self.guide.next();
            let [mut left, mut right] = self.stems.instrumental.at(frame).map(|s| s * instrumental);
            if let Some(stem) = &self.stems.guide {
                let [guide_left, guide_right] = stem.at(frame);
                left += guide_left * guide;
                right += guide_right * guide;
            }
            self.left[i] = left * self.fades[i];
            self.right[i] = right * self.fades[i];
        }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;
    use kernels::{Preset, Stretch};

    use super::*;
    use crate::audio_thread::{AudioThread, Controls};
    use crate::command::{Command, Event, event_ring};
    use crate::driver::OfflineDriver;
    use crate::graph::Graph;
    use test_support::{SECOND, cents_between, harmonic_tone, median_pitch, rms_at};

    /// A graph holding only `backing`, swapped into an offline engine and started on its first
    /// block, so output frame `n` is backing frame `n`.
    struct Rig {
        driver: OfflineDriver,
        controls: Controls,
    }

    impl Rig {
        fn new(backing: Backing) -> Self {
            let (audio, controls) = AudioThread::new();
            let mut graph = Graph::silent();
            graph.set_backing(backing);
            let mut rig = Self { driver: OfflineDriver::new(audio.into_render()), controls };
            rig.send(Command::Swap { id: 0, graph: Box::new(graph) });
            rig.send(Command::Start { id: 1 });
            rig
        }

        fn send(&mut self, command: Command) {
            assert!(self.controls.commands.push(command).is_ok(), "the command ring is full");
        }

        /// At least `frames` more, in whole blocks, under the allocation guard.
        fn pump(&mut self, frames: usize) -> [Vec<f32>; 2] {
            let mut out = vec![0.0; frames.div_ceil(BLOCK) * 2 * BLOCK];
            assert_no_alloc(|| self.driver.pump_into(&mut out));
            [
                out.iter().step_by(2).copied().collect(),
                out.iter().skip(1).step_by(2).copied().collect(),
            ]
        }

        fn events(&mut self) -> Vec<Event> {
            std::iter::from_fn(|| self.controls.events.pop()).collect()
        }

        fn ack(&mut self, id: u32) -> Option<u64> {
            self.events().into_iter().find_map(|event| match event {
                Event::Ack { id: acked, position_frames } if acked == id => Some(position_frames),
                _ => None,
            })
        }
    }

    fn stereo(samples: Vec<f32>) -> Planar {
        Planar { right: samples.clone(), left: samples }
    }

    fn assert_close(actual: f32, expected: f32, tolerance: f32, what: &str) {
        assert!((actual - expected).abs() <= tolerance, "{what}: {actual} vs {expected}");
    }

    #[test]
    fn a_seek_lands_on_the_asked_frame_acks_it_and_fades_without_a_step() {
        // Every sample is its own frame index, scaled, so the output names the frame it came from
        let frames = 10 * SECOND;
        let scale = 0.5 / frames as f32;
        let ramp = (0..frames).map(|i| i as f32 * scale).collect();
        let mut rig = Rig::new(Backing::new(Stems::new(stereo(ramp), None), 0));
        let mut heard = rig.pump(SECOND)[0].clone();

        let to = 6 * SECOND as u64;
        rig.send(Command::Seek { id: 5, to_frame: to });
        let landed = loop {
            heard.extend(&rig.pump(BLOCK)[0]);
            if let Some(position) = rig.ack(5) {
                break position;
            }
            assert!(heard.len() < 2 * SECOND, "the seek never landed");
        };
        assert_eq!(landed, to, "the ack reports the frame the seek landed on");

        let after = rig.pump(2 * SEEK_FADE_FRAMES as usize)[0].clone();
        let last = after.len() - 1;
        let drift = after[last] / scale - (to as usize + last) as f32;
        assert!(drift.abs() <= BLOCK as f32, "{drift} frames from where the seek should be");

        heard.extend(after);
        let slope = 0.5 / SEEK_FADE_FRAMES as f32 + scale;
        let step = heard[SECOND / 2..].windows(2).map(|p| (p[1] - p[0]).abs()).fold(0.0, f32::max);
        assert!(step <= slope, "a step of {step} outruns the 20 ms fades");
    }

    #[test]
    fn a_track_rendered_at_speed_1_09_ends_within_50_ms_of_its_expected_duration() {
        // Half a second of silence, three of tone, one of silence: the tone's edges show whether
        // the stretcher's lead was trimmed and its tail flushed
        let mut track = vec![0.0; SECOND / 2];
        track.extend(harmonic_tone(220.0, 3 * SECOND));
        track.resize(track.len() + SECOND, 0.0);
        let progress = std::sync::atomic::AtomicU32::new(0);
        let stems = prerender(Stems::new(stereo(track), None), 0, 1.09, &progress);
        assert_eq!(f32::from_bits(progress.into_inner()), 1.0);

        let at_speed = |seconds: f64| seconds * f64::from(RATE) / 1.09;
        let tolerance = 0.05 * f64::from(RATE);
        let rendered = &stems.instrumental.left;
        let steady = rms_at(rendered, at_speed(2.0) as usize);
        let window = SECOND / 400;
        let audible = |at: &usize| rms_at(rendered, *at) > steady / 2.0;
        let onset = (0..rendered.len()).find(audible).unwrap() + window;
        let offset = (0..rendered.len()).rfind(audible).unwrap() + window;
        for (edge, expected) in [(onset, at_speed(0.5)), (offset, at_speed(3.5))] {
            let error = edge as f64 - expected;
            assert!(error.abs() < tolerance, "an edge {error:+.0} frames from where it belongs");
        }
        let cents = cents_between(median_pitch(&rendered[SECOND..2 * SECOND]), 220.0);
        assert!(cents.abs() < 5.0, "speed moved the key by {cents} cents");
        let expected = at_speed(4.5);

        let mut rig = Rig::new(Backing::new(stems, 0));
        let mut played = 0;
        while !rig.events().contains(&Event::BackingEnded) {
            rig.pump(BLOCK);
            played += BLOCK;
            assert!(played < 5 * SECOND, "the track never ended");
        }
        assert!((played as f64 - expected).abs() < tolerance, "ended after {played} frames");
    }

    #[test]
    fn stop_at_with_a_three_second_fade_is_silent_at_its_frame_and_ends_once() {
        let mut rig = Rig::new(Backing::new(Stems::new(stereo(vec![0.4; 10 * SECOND]), None), 0));
        let at = 5 * SECOND;
        rig.send(Command::StopAt { id: 2, at_frame: at as u64, fade_frames: 3 * SECOND as u32 });
        let [left, right] = rig.pump(7 * SECOND);

        assert_close(left[2 * SECOND - 1], 0.4, 1e-6, "full level before the fade");
        assert_close(left[3 * SECOND + SECOND / 2], 0.2, 1e-3, "halfway down the fade");
        assert!(left[at..].iter().chain(&right[at..]).all(|s| *s == 0.0), "silent from the frame");
        let events = rig.events();
        assert!(events.contains(&Event::Ack { id: 2, position_frames: 0 }));
        assert_eq!(events.iter().filter(|e| **e == Event::BackingEnded).count(), 1);

        rig.pump(SECOND);
        assert!(!rig.events().contains(&Event::BackingEnded), "raised once");
    }

    #[test]
    fn a_guide_vocal_gain_change_ramps_over_50_ms_with_no_step() {
        let frames = SECOND;
        let stems = Stems::new(Planar::silence(frames), Some(stereo(vec![0.4; frames])));
        let mut rig = Rig::new(Backing::new(stems, 0));
        rig.pump(SECOND / 10);
        rig.controls.params.update(|p| p.stems.guide = 0.0);
        let [left, _] = rig.pump(SECOND / 10);

        let ramp = STEM_RAMP_FRAMES as usize;
        let slope = 0.4 / ramp as f32;
        assert_close(left[0], 0.4 - slope, 1e-6, "the ramp starts on the next block");
        assert!(left.windows(2).all(|p| (p[1] - p[0]).abs() <= slope + 1e-6), "no step");
        assert_close(left[ramp / 2 - 1], 0.2, 1e-4, "halfway at 25 ms");
        assert!(left[ramp - 1..].iter().all(|s| s.abs() < 1e-6), "silent at 50 ms");
    }

    #[test]
    fn a_live_key_change_of_two_semitones_moves_a_tone_two_semitones() {
        let tone = harmonic_tone(220.0, 3 * SECOND);
        let mut rig = Rig::new(Backing::new(Stems::new(stereo(tone), None), 0));
        rig.pump(SECOND / 4);
        rig.send(Command::SetKey { id: 3, semitones: 2 });
        let channels = rig.pump(2 * SECOND);

        let expected = 220.0 * 2f64.powf(2.0 / 12.0);
        for channel in channels {
            // The warm-up and the crossfade are over well inside half a second
            let cents = cents_between(median_pitch(&channel[SECOND / 2..]), expected);
            assert!(cents.abs() < 5.0, "{cents:+.1} cents from two semitones up");
        }
    }

    #[test]
    fn correctors_follow_armed_plus_heard_key_and_position_drops_the_stretchers_latency() {
        let (mut events, _drain) = event_ring();
        let mut backing = Backing::new(Stems::new(Planar::silence(2 * SECOND), None), 3);
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        let mut render = |backing: &mut Backing, events: &mut EventProducer, blocks: usize| {
            for _ in 0..blocks {
                let mut out = Stereo { left: &mut left, right: &mut right };
                assert_no_alloc(|| backing.render(&StemGains::default(), &mut out, events));
            }
        };

        backing.steer(1, Transport::Start, &mut events);
        render(&mut backing, &mut events, 10);
        assert_eq!((backing.semitones(), backing.position_frames()), (3, 10 * BLOCK as u64));

        backing.steer(2, Transport::SetKey { semitones: 2 }, &mut events);
        render(&mut backing, &mut events, 1);
        assert_eq!(backing.semitones(), 3, "the old key while the new one warms");

        let latency = Stretch::new(2, RATE, Preset::Cheaper).latency_frames();
        let blocks = (latency + CROSSFADE_FRAMES as usize).div_ceil(BLOCK) + 1;
        render(&mut backing, &mut events, blocks);
        let read = (11 + blocks) * BLOCK;
        assert_eq!(backing.semitones(), 5, "the new key once the crossfade is done");
        assert_eq!(backing.position_frames(), (read - latency) as u64);
    }
}
