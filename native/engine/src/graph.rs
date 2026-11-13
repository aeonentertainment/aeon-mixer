//! One mixing graph for one performance: slots into a stereo bus, through the master. Built on
//! the control thread, rendered on the audio thread, and dropped on the control thread again
//! once it comes back through the retired ring.

use crate::backing::{Backing, Transport};
use crate::block::{MAX_SLOTS, Stereo};
use crate::command::{Event, EventProducer};
use crate::master::Master;
use crate::params::Params;
use crate::return_ring::ReturnProducer;
use crate::slot::Slot;
use crate::slot::cue::Cue;
use crate::slot::guide::Guide;
use crate::slot::shared::Shared;

pub struct Graph {
    slots: Vec<Slot>,
    backing: Option<Backing>,
    /// The engine's output latency, from a backing frame rendered to it leaving the speaker.
    backing_offset_frames: u32,
    master: Master,
}

impl Graph {
    /// At most `MAX_SLOTS` slots, since `Params` carries that many.
    pub fn new(slots: Vec<Slot>) -> Self {
        assert!(slots.len() <= MAX_SLOTS, "a graph holds at most {MAX_SLOTS} slots");
        Self { slots, backing: None, backing_offset_frames: 0, master: Master::new() }
    }

    /// What the engine renders between songs.
    pub fn silent() -> Self {
        Self::new(Vec::new())
    }

    /// One block. Reads `params` once, writes `out`, pushes events. Allocation-free.
    pub fn render(&mut self, params: &Params, out: Stereo<'_>, events: &mut EventProducer) {
        self.render_with(params, out, events, |_| {});
    }

    /// `render` with `add_voices` summing what the engine owns beyond this graph, its reaction
    /// voices, into the bus ahead of the master, so they are clipped and metered with the mix.
    pub fn render_with(
        &mut self,
        params: &Params,
        mut out: Stereo<'_>,
        events: &mut EventProducer,
        add_voices: impl FnOnce(&mut Stereo<'_>),
    ) {
        out.silence();
        let any_solo = params.slots[..self.slots.len()].iter().any(|slot| slot.solo);
        let shared = Shared { any_solo, cue: self.cue(), history: self.master.history() };
        for (index, (slot, slot_params)) in self.slots.iter_mut().zip(&params.slots).enumerate() {
            slot.render(index as u8, slot_params, shared, &mut out, events);
        }
        if let Some(backing) = &mut self.backing {
            backing.render(&params.stems, &mut out, events);
        }
        add_voices(&mut out);
        self.master.process(&mut out, events);
    }

    /// The backing playhead, which acks report; zero with no backing.
    pub fn position_frames(&self) -> u64 {
        self.backing.as_ref().map_or(0, Backing::position_frames)
    }

    /// The device's output latency, which the guide clock subtracts so a singer is corrected
    /// toward the note they hear, not the one being rendered. The audio thread sets it as the
    /// graph is swapped in, and again in the first block after a move to another output.
    pub fn set_backing_offset_frames(&mut self, frames: u32) {
        self.backing_offset_frames = frames;
    }

    /// Where the backing the room hears is, and its key, at the top of this block.
    fn cue(&self) -> Cue {
        let Some(backing) = &self.backing else { return Cue::default() };
        let heard = i64::try_from(backing.position_frames()).unwrap_or(i64::MAX);
        Cue {
            frame: heard - i64::from(self.backing_offset_frames),
            semitones: backing.semitones(),
            advancing: backing.is_advancing(),
        }
    }

    /// Gives the master a return ring for the network thread to encode, before the graph goes
    /// live: the mixer's `rtpReturn`.
    pub fn set_return(&mut self, ring: ReturnProducer) {
        self.master.set_return(ring);
    }

    /// Installs the backing path once its render job is done, before the graph goes live.
    pub fn set_backing(&mut self, backing: Backing) {
        self.backing = Some(backing);
    }

    /// A backing command, acked by the backing path when it lands, or at once with no backing.
    pub fn steer(&mut self, id: u32, transport: Transport, events: &mut EventProducer) {
        match &mut self.backing {
            Some(backing) => backing.steer(id, transport, events),
            None => events.push(Event::Ack { id, position_frames: 0 }),
        }
    }

    /// Gives `guide` to a slot and returns what must be retired: the guide it displaced, or
    /// `guide` itself when there is no such slot.
    pub fn set_guide(&mut self, slot: u8, guide: Option<Box<Guide>>) -> Option<Box<Guide>> {
        match self.slots.get_mut(usize::from(slot)) {
            Some(slot) => slot.set_guide(guide),
            None => guide,
        }
    }

    /// Every slot hears that the output's callback now takes `callback_frames` at a wake.
    pub fn output_moved(&mut self, callback_frames: u32) {
        for slot in &mut self.slots {
            slot.output_moved(callback_frames);
        }
    }

    /// After a move to another output: every slot starts again on the new clock.
    pub fn restart_slots(&mut self) {
        for slot in &mut self.slots {
            slot.restart();
        }
    }

    pub fn reset_slot(&mut self, slot: u8) {
        if let Some(slot) = self.slots.get_mut(usize::from(slot)) {
            slot.reset();
        }
    }

    pub fn slots(&self) -> &[Slot] {
        &self.slots
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::backing::test_support::SECOND;
    use crate::backing::{Planar, Stems};
    use crate::block::{BLOCK, RATE};
    use crate::command::{Event, EventConsumer, event_ring};
    use crate::master::KNEE;
    use crate::params::{ReverbParams, VoiceParams};
    use crate::slot::meter::{FLOOR_DB, WINDOW_FRAMES};
    use crate::slot::source::Source;
    use crate::slot::source::fixtures::{Constant, Sine};

    const CENTRE: f32 = std::f32::consts::FRAC_1_SQRT_2;

    struct Bench {
        graph: Graph,
        params: Params,
        events: EventProducer,
        drain: EventConsumer,
        left: [f32; BLOCK],
        right: [f32; BLOCK],
    }

    impl Bench {
        fn new(graph: Graph) -> Self {
            let (events, drain) = event_ring();
            Self {
                graph,
                params: Params::default(),
                events,
                drain,
                left: [0.0; BLOCK],
                right: [0.0; BLOCK],
            }
        }

        fn render(&mut self) {
            let Self { graph, params, events, left, right, .. } = self;
            assert_no_alloc(|| graph.render(params, Stereo { left, right }, events));
        }

        fn render_blocks(&mut self, blocks: usize) {
            for _ in 0..blocks {
                self.render();
            }
        }

        fn levels(&mut self) -> Vec<Event> {
            std::iter::from_fn(|| self.drain.pop())
                .filter(|event| matches!(event, Event::Level { .. }))
                .collect()
        }
    }

    /// Samples a slot plays once, then silence.
    struct Recording {
        samples: Vec<f32>,
        at: usize,
    }

    impl Source for Recording {
        fn pull(&mut self, out: &mut [f32]) {
            for sample in out {
                *sample = self.samples.get(self.at).copied().unwrap_or(0.0);
                self.at += 1;
            }
        }
    }

    fn recording(samples: Vec<f32>) -> Box<Recording> {
        Box::new(Recording { samples, at: 0 })
    }

    fn graph_of(sources: impl IntoIterator<Item = f32>) -> Graph {
        Graph::new(sources.into_iter().map(|dc| Slot::new(Box::new(Constant(dc)))).collect())
    }

    fn assert_close(actual: f32, expected: f32, tolerance: f32, what: &str) {
        assert!((actual - expected).abs() <= tolerance, "{what}: {actual} vs {expected}");
    }

    #[test]
    fn gain_and_the_pan_law_shape_a_dc_source() {
        let mut bench = Bench::new(graph_of([1.0]));
        bench.params.slots[0].gain = 0.5;
        bench.render();
        assert_close(bench.left[BLOCK - 1], 0.5 * CENTRE, 1e-6, "centre left");
        assert_close(bench.right[BLOCK - 1], 0.5 * CENTRE, 1e-6, "centre right");

        bench.params.slots[0].pan = -1.0;
        bench.render_blocks(2);
        assert_close(bench.left[0], 0.5, 1e-6, "hard left, left");
        assert_close(bench.right[0], 0.0, 1e-6, "hard left, right");
    }

    #[test]
    fn a_gain_change_ramps_across_exactly_one_block() {
        let mut bench = Bench::new(graph_of([0.5]));
        bench.params.slots[0].pan = -1.0;
        bench.render();
        bench.params.slots[0].gain = 0.0;
        bench.render();
        let step = 0.5 / BLOCK as f32;
        for (i, sample) in bench.left.iter().enumerate() {
            assert_close(*sample, 0.5 - step * (i + 1) as f32, 1e-5, "ramp sample");
        }
        bench.render();
        assert!(bench.left.iter().all(|s| *s == 0.0), "the block after the ramp is silent");
    }

    #[test]
    fn mute_reaches_silence_inside_one_block_and_leaves_the_meter_alone() {
        let mut bench = Bench::new(graph_of([1.0]));
        bench.render();
        bench.params.slots[0].mute = true;
        bench.render();
        assert!(bench.left[0] < CENTRE && bench.left[0] > 0.0, "the ramp has begun");
        assert_close(bench.left[BLOCK - 1], 0.0, 1e-5, "silent by the last frame");
        bench.render_blocks(WINDOW_FRAMES as usize / BLOCK);
        assert!(bench.left.iter().chain(&bench.right).all(|s| *s == 0.0));
        let levels = bench.levels();
        assert!(!levels.is_empty());
        for level in levels {
            let Event::Level { slot: 0, peak_db, .. } = level else { panic!("{level:?}") };
            assert_close(peak_db, 0.0, 0.01, "a muted slot still meters its singer");
        }
    }

    #[test]
    fn solo_silences_the_unsoloed_and_mute_wins() {
        let mut bench = Bench::new(graph_of([0.5, 0.5, 0.5]));
        bench.params.slots[0].pan = -1.0;
        bench.params.slots[1].pan = 1.0;
        bench.params.slots[2].pan = 1.0;
        bench.params.slots[1].solo = true;
        bench.render();
        assert_close(bench.left[BLOCK - 1], 0.0, 1e-6, "the unsoloed slot is silent");
        assert_close(bench.right[BLOCK - 1], 0.5, 1e-6, "only the soloed slot plays");

        bench.params.slots[1].mute = true;
        bench.render_blocks(2);
        assert!(bench.left.iter().chain(&bench.right).all(|s| *s == 0.0), "mute wins over solo");
    }

    #[test]
    fn meters_read_a_known_sine_within_a_tenth_of_a_decibel_at_ten_hertz() {
        let mut bench = Bench::new(Graph::new(vec![Slot::new(Box::new(Sine::new(1000.0, 0.5)))]));
        let blocks_per_second = (crate::block::RATE as usize).div_ceil(BLOCK);
        bench.render_blocks(blocks_per_second);
        let levels = bench.levels();
        assert_eq!(levels.len(), 10, "ten readings in a second");
        for level in levels {
            let Event::Level { slot: 0, rms_db, peak_db } = level else { panic!("{level:?}") };
            assert_close(rms_db, -9.03, 0.1, "rms");
            assert_close(peak_db, -6.02, 0.1, "peak");
        }
    }

    #[test]
    fn the_clipper_bounds_a_hot_sum_and_passes_half_scale_untouched() {
        let mut hot = Bench::new(graph_of([1.0, 1.0]));
        hot.render();
        assert!(hot.left.iter().all(|s| *s > KNEE && *s < 1.0), "a 1.41 sum is bent under 1.0");

        let mut half = Bench::new(graph_of([0.5 / CENTRE]));
        half.render();
        let error_db = 20.0 * (half.left[BLOCK - 1] / 0.5).log10();
        assert!(error_db.abs() < 0.01, "half scale moved by {error_db} dB");
    }

    #[test]
    fn a_guide_goes_to_its_slot_or_comes_straight_back() {
        let mut graph = graph_of([0.0]);
        let guide = Box::new(Guide { notes: Vec::new(), key_mask: 0b1010_1011_0101 });
        assert!(graph.set_guide(0, Some(guide)).is_none());
        assert_eq!(graph.slots()[0].guide().map(|g| g.key_mask), Some(0b1010_1011_0101));
        let displaced = graph.set_guide(0, None).expect("the old guide comes back");
        assert_eq!(displaced.key_mask, 0b1010_1011_0101);
        assert!(graph.set_guide(7, Some(displaced)).is_some(), "no slot 7, so it comes back");
    }

    // The guide clock is the backing frame heard, less the output latency, plus the slot's guide
    // offset: readings follow the playhead, and moving either offset moves every one alike
    #[test]
    fn pitch_readings_sit_on_the_guide_clock() {
        let readings = |offset_frames: u32, guide_offset_ms: f32| {
            let mut graph = Graph::new(vec![Slot::new(Box::new(Sine::new(220.0, 0.5)))]);
            graph.set_backing(Backing::new(Stems::new(Planar::silence(20 * SECOND), None), 0));
            graph.set_backing_offset_frames(offset_frames);
            let mut bench = Bench::new(graph);
            bench.params.slots[0].guide_offset_ms = guide_offset_ms;
            bench.graph.steer(
                1,
                Transport::Seek { to_frame: 10 * SECOND as u64 },
                &mut bench.events,
            );
            bench.graph.steer(2, Transport::Start, &mut bench.events);
            bench.render_blocks(SECOND / 2 / BLOCK);
            std::iter::from_fn(|| bench.drain.pop())
                .filter_map(|e| {
                    if let Event::Pitch { position_ms, .. } = e { Some(position_ms) } else { None }
                })
                .collect::<Vec<f64>>()
        };
        let plain = readings(0, 0.0);
        assert!(plain.len() >= 40, "{} readings in half a second", plain.len());
        assert!(plain.iter().all(|ms| (9_950.0..10_500.0).contains(ms)), "off the playhead");
        assert!(plain.windows(2).all(|p| (p[1] - p[0] - 10.0).abs() < 1e-9), "10 ms apart");

        let moved = readings(480, 50.0);
        assert_eq!(plain.len(), moved.len());
        for (plain, moved) in plain.iter().zip(&moved) {
            assert!((moved - plain - 40.0).abs() < 1e-9, "{plain} moved to {moved}");
        }
    }

    // The meter sits before the reverb, so the level is the singing and not the room
    #[test]
    fn the_reverb_rings_on_after_the_voice_but_the_meter_hears_only_the_voice() {
        let burst = WINDOW_FRAMES as usize;
        let voice = (0..burst).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
        let mut bench = Bench::new(Graph::new(vec![Slot::new(recording(voice))]));
        bench.params.slots[0].effects.reverb =
            ReverbParams { enabled: true, room_size: 0.9, mix: 0.5 };
        let mut out = Vec::new();
        for _ in 0..(4 * burst).div_ceil(BLOCK) {
            bench.render();
            out.extend(bench.left);
        }
        assert!(
            out[2 * burst..3 * burst].iter().any(|s| s.abs() > 1e-3),
            "no tail after the voice"
        );
        let levels = bench.levels();
        let Event::Level { peak_db, .. } = levels[2] else { panic!("{:?}", levels[2]) };
        assert_eq!(peak_db, FLOOR_DB, "the meter heard the reverb");
    }

    // The pitch readings and the slot's level come from ahead of the voice, so the trail, the score
    // and the level words judge the singing: a voice changes the sound and none of them
    #[test]
    fn a_voice_changes_the_sound_and_not_the_readings_or_the_level() {
        let events = |voice: VoiceParams| {
            let mut bench =
                Bench::new(Graph::new(vec![Slot::new(Box::new(Sine::new(220.0, 0.5)))]));
            bench.params.slots[0].effects.voice = voice;
            let mut out = Vec::new();
            for _ in 0..(RATE as usize / 2).div_ceil(BLOCK) {
                bench.render();
                out.extend(bench.left);
            }
            let slot_events = std::iter::from_fn(|| bench.drain.pop())
                .filter(|e| matches!(e, Event::Pitch { .. } | Event::Level { .. }));
            (slot_events.collect::<Vec<Event>>(), out)
        };
        let (plain, dry) = events(VoiceParams::default());
        let (voiced, wet) = events(VoiceParams { enabled: true, formant_semitones: -2.0 });
        assert!(plain.iter().any(|e| matches!(e, Event::Pitch { .. })), "no readings");
        assert!(plain.iter().any(|e| matches!(e, Event::Level { .. })), "no level");
        assert_eq!(plain, voiced);
        assert_ne!(dry, wet, "the voice changed nothing");
    }
}
