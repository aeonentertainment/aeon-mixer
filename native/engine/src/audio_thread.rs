//! What runs inside the device callback: drain the commands, read the snapshot, render the live
//! graph, crossfade an outgoing one, and hand what is no longer held back through the retired
//! ring. A panic inside is caught here; every block after is silence, no further command is
//! taken, and `Fault` is sent, again each block until the event ring has room for it
//! (docs/design.md section 2). Moved to another output, it starts every slot's stream again and
//! fades in (section 5).

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::backing::Transport;
use crate::block::{BLOCK, RATE, Stereo};
use crate::command::{
    Command, CommandConsumer, CommandProducer, Event, EventConsumer, EventProducer, Retired,
    RetiredConsumer, RetiredProducer, command_ring, event_ring, retired_ring,
};
use crate::driver::{Movable, Render};
use crate::graph::Graph;
use crate::params::{ParamsReader, ParamsWriter, params_channel};
use crate::reactions::{Reactions, ReactionsFeed, reactions};

/// 5 ms: the graph swap fade of docs/design.md section 4.
pub const SWAP_FADE_FRAMES: u32 = RATE / 200;

pub struct AudioThread {
    params: ParamsReader,
    commands: CommandConsumer,
    events: EventProducer,
    retired: RetiredProducer,
    live: Box<Graph>,
    /// The graph fading out, with how many frames of the fade have played. It is held past the
    /// fade only while the retired ring has no room.
    outgoing: Option<Box<Graph>>,
    faded: u32,
    outgoing_left: [f32; BLOCK],
    outgoing_right: [f32; BLOCK],
    /// The engine's own voices, rendered once a block and summed into every graph that renders
    /// in it, so a reaction plays straight through a swap fade.
    reactions: Reactions,
    reaction_left: [f32; BLOCK],
    reaction_right: [f32; BLOCK],
    faulted: bool,
    fault_sent: bool,
    /// Set by a move: the next block starts every slot's stream again on the new clock.
    restart: bool,
    /// The stream's granted period, which the follower writes as each stream starts, and the
    /// one the graphs last heard: a graph swapped in hears it too.
    callback: Arc<AtomicU32>,
    heard: u32,
    /// Frames of the fade in after a move that have played; the fade is done at the swap fade.
    faded_in: u32,
}

/// The control thread's ends of the four crossings, and the feed for the reaction voices.
pub struct Controls {
    pub params: ParamsWriter,
    pub commands: CommandProducer,
    pub events: EventConsumer,
    pub retired: RetiredConsumer,
    pub reactions: ReactionsFeed,
}

impl AudioThread {
    /// Starts on the silent graph, as the engine does between songs.
    pub fn new() -> (Self, Controls) {
        let (params_writer, params) = params_channel();
        let (command_producer, commands) = command_ring();
        let (events, event_consumer) = event_ring();
        let (retired, retired_consumer) = retired_ring();
        let (reactions_feed, reactions) = reactions();
        let audio = Self {
            params,
            commands,
            events,
            retired,
            live: Box::new(Graph::silent()),
            outgoing: None,
            faded: 0,
            outgoing_left: [0.0; BLOCK],
            outgoing_right: [0.0; BLOCK],
            reactions,
            reaction_left: [0.0; BLOCK],
            reaction_right: [0.0; BLOCK],
            faulted: false,
            fault_sent: false,
            restart: false,
            callback: Arc::default(),
            heard: 0,
            faded_in: SWAP_FADE_FRAMES,
        };
        let controls = Controls {
            params: params_writer,
            commands: command_producer,
            events: event_consumer,
            retired: retired_consumer,
            reactions: reactions_feed,
        };
        (audio, controls)
    }

    pub fn into_render(mut self) -> Render {
        Box::new(move |out| self.render(out))
    }

    /// One block, as the driver calls it. Never panics: a panic below becomes a fault.
    pub fn render(&mut self, mut out: Stereo<'_>) {
        if !self.faulted && catch_unwind(AssertUnwindSafe(|| self.render_block(&mut out))).is_ok() {
            self.fade_in(&mut out);
            return;
        }
        self.faulted = true;
        out.silence();
        if !self.fault_sent {
            self.fault_sent = self.events.try_push(Event::Fault);
        }
    }

    fn render_block(&mut self, out: &mut Stereo<'_>) {
        if std::mem::take(&mut self.restart) {
            self.restart_slots();
        }
        let callback = self.callback.load(Ordering::Acquire);
        if callback != 0 && callback != self.heard {
            self.heard = callback;
            self.live.output_moved(callback);
            if let Some(outgoing) = &mut self.outgoing {
                outgoing.output_moved(callback);
            }
        }
        self.drain_commands();
        let params = *self.params.latest();
        let Self { reactions, reaction_left, reaction_right, live, events, .. } = self;
        reactions.render(Stereo { left: reaction_left, right: reaction_right });
        let add_reactions = |bus: &mut Stereo<'_>| {
            for ((l, r), (left, right)) in bus
                .left
                .iter_mut()
                .zip(bus.right.iter_mut())
                .zip(reaction_left.iter().zip(reaction_right.iter()))
            {
                *l += left;
                *r += right;
            }
        };
        live.render_with(
            &params,
            Stereo { left: out.left, right: out.right },
            events,
            add_reactions,
        );

        let Some(outgoing) = &mut self.outgoing else { return };
        if self.faded < SWAP_FADE_FRAMES {
            outgoing.render_with(
                &params,
                Stereo { left: &mut self.outgoing_left, right: &mut self.outgoing_right },
                &mut self.events,
                add_reactions,
            );
            let fade = SWAP_FADE_FRAMES as f32;
            for (i, (left, right)) in out.left.iter_mut().zip(out.right.iter_mut()).enumerate() {
                let incoming = ((self.faded + i as u32 + 1) as f32 / fade).min(1.0);
                *left = *left * incoming + self.outgoing_left[i] * (1.0 - incoming);
                *right = *right * incoming + self.outgoing_right[i] * (1.0 - incoming);
            }
            self.faded += BLOCK as u32;
        }
        if self.faded >= SWAP_FADE_FRAMES
            && self.retired.room() > 0
            && let Some(done) = self.outgoing.take()
        {
            self.retire(Retired::Graph(done));
        }
    }

    /// Takes commands until the ring is empty or the next one carries a box the retired ring
    /// could not take back. One slot stays reserved for the outgoing graph's own retirement.
    fn drain_commands(&mut self) {
        loop {
            let Some(next) = self.commands.peek() else { return };
            let carries_box = matches!(next, Command::Swap { .. } | Command::SetGuide { .. });
            if carries_box && self.retired.room() <= usize::from(self.outgoing.is_some()) {
                return;
            }
            let Some(command) = self.commands.pop() else { return };
            match command {
                Command::Swap { id, mut graph } => {
                    // Built before a move, a graph learns the output it now plays to
                    if self.heard != 0 {
                        graph.output_moved(self.heard);
                    }
                    if let Some(previous) = self.outgoing.take() {
                        self.retire(Retired::Graph(previous));
                    }
                    self.outgoing = Some(std::mem::replace(&mut self.live, graph));
                    self.faded = 0;
                    self.ack(id);
                }
                Command::SetGuide { slot, guide } => {
                    if let Some(displaced) = self.live.set_guide(slot, guide) {
                        self.retire(Retired::Guide(displaced));
                    }
                }
                Command::ResetSlot { slot } => self.live.reset_slot(slot),
                Command::Start { id } => self.steer(id, Transport::Start),
                Command::Seek { id, to_frame } => self.steer(id, Transport::Seek { to_frame }),
                Command::Pause { id } => self.steer(id, Transport::Pause),
                Command::Resume { id } => self.steer(id, Transport::Resume),
                Command::SetKey { id, semitones } => {
                    self.steer(id, Transport::SetKey { semitones })
                }
                Command::StopAt { id, at_frame, fade_frames } => {
                    self.steer(id, Transport::StopAt { at_frame, fade_frames });
                }
            }
        }
    }

    // Every packet a slot holds was anchored to the clock that stopped, and the gap's packets
    // waited in its ring; played now, the singer would stay a gap behind for the rest of the song.
    fn restart_slots(&mut self) {
        self.live.restart_slots();
        if let Some(outgoing) = &mut self.outgoing {
            outgoing.restart_slots();
        }
    }

    // A new output starts from silence, so its first swap fade's worth of frames ramps up.
    fn fade_in(&mut self, out: &mut Stereo<'_>) {
        if self.faded_in >= SWAP_FADE_FRAMES {
            return;
        }
        let fade = SWAP_FADE_FRAMES as f32;
        for (i, (left, right)) in out.left.iter_mut().zip(out.right.iter_mut()).enumerate() {
            let gain = ((self.faded_in + i as u32 + 1) as f32 / fade).min(1.0);
            *left *= gain;
            *right *= gain;
        }
        self.faded_in += BLOCK as u32;
    }

    fn steer(&mut self, id: u32, transport: Transport) {
        self.live.steer(id, transport, &mut self.events);
    }

    fn ack(&mut self, id: u32) {
        self.events.push(Event::Ack { id, position_frames: self.live.position_frames() });
    }

    /// Every caller has checked `room` first and this thread is the only producer, so the push
    /// cannot fail. If it ever did, leaking is the one failure that neither frees on the audio
    /// thread nor loses the device.
    fn retire(&mut self, retired: Retired) {
        if let Err(retired) = self.retired.push(retired) {
            std::mem::forget(retired);
        }
    }
}

impl Movable for AudioThread {
    fn render(&mut self, out: Stereo<'_>) {
        AudioThread::render(self, out);
    }

    fn moved(&mut self) {
        self.restart = true;
        self.faded_in = 0;
    }

    fn callback_frames(&self) -> Arc<AtomicU32> {
        Arc::clone(&self.callback)
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::command::EVENT_CAPACITY;
    use crate::driver::OfflineDriver;
    use crate::slot::Slot;
    use crate::slot::guide::Guide;
    use crate::slot::source::fixtures::{Constant, Panics};

    const CENTRE: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn dc_graph(level: f32) -> Box<Graph> {
        Box::new(Graph::new(vec![Slot::new(Box::new(Constant(level)))]))
    }

    fn pump(driver: &mut OfflineDriver, blocks: usize) -> Vec<f32> {
        let mut out = vec![0.0; blocks * 2 * BLOCK];
        assert_no_alloc(|| driver.pump_into(&mut out));
        out
    }

    fn events(controls: &mut Controls) -> Vec<Event> {
        std::iter::from_fn(|| controls.events.pop()).collect()
    }

    #[test]
    fn a_swap_fades_in_over_five_milliseconds_acks_and_retires_the_old_graph() {
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        assert!(pump(&mut driver, 1).iter().all(|s| *s == 0.0), "silent before the swap");

        controls.commands.push(Command::Swap { id: 7, graph: dc_graph(1.0) }).ok().unwrap();
        let blocks = (SWAP_FADE_FRAMES as usize).div_ceil(BLOCK) + 1;
        let out = pump(&mut driver, blocks);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();

        let slope = CENTRE / SWAP_FADE_FRAMES as f32;
        for pair in left.windows(2) {
            assert!(
                pair[1] - pair[0] <= slope + 1e-5,
                "step {} exceeds the fade's slope",
                pair[1] - pair[0]
            );
            assert!(pair[1] >= pair[0] - 1e-6, "the fade only rises");
        }
        assert!(
            (left[SWAP_FADE_FRAMES as usize - 1] - CENTRE).abs() < 1e-5,
            "at level when the fade ends"
        );
        assert_eq!(left[left.len() - 1], CENTRE);

        assert_eq!(events(&mut controls), vec![Event::Ack { id: 7, position_frames: 0 }]);
        assert!(
            matches!(controls.retired.pop(), Some(Retired::Graph(_))),
            "the silent graph came back"
        );
        assert!(controls.retired.pop().is_none());
    }

    #[test]
    fn swapping_to_silence_fades_out_without_a_step() {
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        controls.commands.push(Command::Swap { id: 1, graph: dc_graph(1.0) }).ok().unwrap();
        pump(&mut driver, 2);
        controls.retired.pop().expect("the silent graph");

        controls
            .commands
            .push(Command::Swap { id: 2, graph: Box::new(Graph::silent()) })
            .ok()
            .unwrap();
        let out = pump(&mut driver, 3);
        let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
        let slope = CENTRE / SWAP_FADE_FRAMES as f32;
        assert!(right.windows(2).all(|p| (p[1] - p[0]).abs() <= slope + 1e-5));
        assert!(right[SWAP_FADE_FRAMES as usize..].iter().all(|s| *s == 0.0));
        assert!(matches!(controls.retired.pop(), Some(Retired::Graph(_))));
    }

    #[test]
    fn a_move_keeps_the_graph_and_fades_the_new_output_in() {
        let (mut audio, mut controls) = AudioThread::new();
        controls.commands.push(Command::Swap { id: 1, graph: dc_graph(1.0) }).ok().unwrap();
        controls.commands.push(Command::Start { id: 2 }).ok().unwrap();
        let block = |audio: &mut AudioThread| {
            let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
            assert_no_alloc(|| audio.render(Stereo { left: &mut left, right: &mut right }));
            left
        };
        let before = 4;
        for _ in 0..before {
            block(&mut audio);
        }
        assert_eq!(block(&mut audio)[0], CENTRE, "at level before the move");

        Movable::moved(&mut audio);
        let blocks = (SWAP_FADE_FRAMES as usize).div_ceil(BLOCK) + 1;
        let left: Vec<f32> = (0..blocks).flat_map(|_| block(&mut audio)).collect();
        let slope = CENTRE / SWAP_FADE_FRAMES as f32;
        assert!(left[0] > 0.0 && left[0] <= slope + 1e-6, "the first frame starts near silence");
        assert!(left.windows(2).all(|p| p[1] - p[0] <= slope + 1e-5 && p[1] >= p[0] - 1e-6));
        assert!((left[SWAP_FADE_FRAMES as usize - 1] - CENTRE).abs() < 1e-5, "at level again");

        controls.commands.push(Command::Pause { id: 3 }).ok().unwrap();
        block(&mut audio);
        let acks: Vec<_> = events(&mut controls)
            .into_iter()
            .filter_map(|e| if let Event::Ack { id, .. } = e { Some(id) } else { None })
            .collect();
        assert_eq!(acks, vec![1, 2, 3], "the same graph answers after the move");
        assert!(matches!(controls.retired.pop(), Some(Retired::Graph(_))), "the silent graph");
        assert!(controls.retired.pop().is_none(), "and nothing else: the live graph stayed");
    }

    #[test]
    fn a_known_graph_pumps_the_same_samples_twice() {
        let render = |level| {
            let (audio, mut controls) = AudioThread::new();
            controls.commands.push(Command::Swap { id: 1, graph: dc_graph(level) }).ok().unwrap();
            controls.params.update(|p| p.slots[0].pan = 0.25);
            let mut driver = OfflineDriver::new(audio.into_render());
            pump(&mut driver, 8)
        };
        let first = render(0.6);
        assert_eq!(first, render(0.6));
        assert_ne!(first, render(0.5));
        assert!(first.iter().any(|s| *s != 0.0));
    }

    #[test]
    fn a_guide_lands_in_its_slot_and_the_displaced_one_is_retired() {
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        controls.commands.push(Command::Swap { id: 1, graph: dc_graph(0.0) }).ok().unwrap();
        let guide = |mask| Some(Box::new(Guide { notes: Vec::new(), key_mask: mask }));
        controls.commands.push(Command::SetGuide { slot: 0, guide: guide(1) }).ok().unwrap();
        controls.commands.push(Command::SetGuide { slot: 0, guide: guide(2) }).ok().unwrap();
        controls.commands.push(Command::SetGuide { slot: 3, guide: guide(4) }).ok().unwrap();
        pump(&mut driver, 1);

        let mut masks = Vec::new();
        while let Some(retired) = controls.retired.pop() {
            match retired {
                Retired::Guide(guide) => masks.push(guide.key_mask),
                Retired::Graph(_) => masks.push(0),
            }
        }
        assert_eq!(masks, vec![1, 4], "the displaced guide and the one with no slot come back");
        pump(&mut driver, 3);
        assert!(matches!(controls.retired.pop(), Some(Retired::Graph(_))), "then the old graph");
    }

    #[test]
    fn a_full_retired_ring_holds_commands_back_instead_of_dropping_a_graph() {
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        for id in 0..crate::command::RETIRED_CAPACITY as u32 + 2 {
            controls.commands.push(Command::Swap { id, graph: dc_graph(0.0) }).ok().unwrap();
            pump(&mut driver, 4);
        }
        let acked: Vec<_> = events(&mut controls)
            .into_iter()
            .filter_map(|e| if let Event::Ack { id, .. } = e { Some(id) } else { None })
            .collect();
        assert_eq!(acked.len(), crate::command::RETIRED_CAPACITY, "one swap per retired slot");

        while controls.retired.pop().is_some() {}
        pump(&mut driver, 4);
        let late: Vec<_> = events(&mut controls)
            .into_iter()
            .filter_map(|e| if let Event::Ack { id, .. } = e { Some(id) } else { None })
            .collect();
        assert_eq!(
            late,
            vec![acked.len() as u32, acked.len() as u32 + 1],
            "the held swaps land once there is room"
        );
    }

    #[test]
    fn backing_commands_ack_with_the_position() {
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        controls.commands.push(Command::Start { id: 3 }).ok().unwrap();
        controls.commands.push(Command::Seek { id: 4, to_frame: 48_000 }).ok().unwrap();
        controls.commands.push(Command::ResetSlot { slot: 0 }).ok().unwrap();
        pump(&mut driver, 1);
        assert_eq!(
            events(&mut controls),
            vec![
                Event::Ack { id: 3, position_frames: 0 },
                Event::Ack { id: 4, position_frames: 0 }
            ]
        );
    }

    #[test]
    fn a_reaction_plays_straight_through_a_swap_and_under_the_clipper() {
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        let blocks = (SWAP_FADE_FRAMES as usize).div_ceil(BLOCK) + 2;
        assert!(controls.reactions.play(&[0.5, 0.25].repeat(blocks * BLOCK)));
        controls.commands.push(Command::Swap { id: 1, graph: dc_graph(0.0) }).ok().unwrap();
        let out = pump(&mut driver, blocks);
        assert!(out.iter().step_by(2).all(|s| (*s - 0.5).abs() < 1e-6), "no notch on the left");
        assert!(out.iter().skip(1).step_by(2).all(|s| (*s - 0.25).abs() < 1e-6), "nor the right");

        // A hot reaction over a hot graph goes through the one clipper with it
        controls.commands.push(Command::Swap { id: 2, graph: dc_graph(1.0) }).ok().unwrap();
        pump(&mut driver, blocks);
        assert!(pump(&mut driver, 1).iter().all(|s| (*s - CENTRE).abs() < 1e-6));
        assert!(controls.reactions.play(&[1.0, 1.0].repeat(BLOCK)));
        let out = pump(&mut driver, 1);
        assert!(out.iter().all(|s| *s > CENTRE && *s < 1.0), "{} is the clipped sum", out[0]);
    }

    #[test]
    fn a_panic_in_render_becomes_silence_a_fault_and_no_more_commands() {
        // Not under the guard: the panic itself allocates its payload, and that is the point of
        // the boundary, not a leak in the render path.
        let (audio, mut controls) = AudioThread::new();
        let mut driver = OfflineDriver::new(audio.into_render());
        let panicking = Box::new(Graph::new(vec![Slot::new(Box::new(Panics))]));
        controls.commands.push(Command::Swap { id: 1, graph: panicking }).ok().unwrap();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let out = driver.pump(2);
        std::panic::set_hook(hook);
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(
            events(&mut controls),
            vec![Event::Ack { id: 1, position_frames: 0 }, Event::Fault]
        );

        controls.commands.push(Command::Start { id: 2 }).ok().unwrap();
        assert!(pump(&mut driver, 1).iter().all(|s| *s == 0.0));
        assert!(events(&mut controls).is_empty(), "a faulted engine takes no command");
    }

    #[test]
    fn a_fault_that_finds_the_event_ring_full_lands_once_there_is_room() {
        let (mut audio, mut controls) = AudioThread::new();
        for _ in 0..EVENT_CAPACITY {
            audio.events.push(Event::Underrun);
        }
        let mut driver = OfflineDriver::new(audio.into_render());
        let panicking = Box::new(Graph::new(vec![Slot::new(Box::new(Panics))]));
        controls.commands.push(Command::Swap { id: 1, graph: panicking }).ok().unwrap();
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        driver.pump(1);
        std::panic::set_hook(hook);

        // Under the guard from here: the retry runs on the audio thread, block after block
        assert!(pump(&mut driver, 3).iter().all(|s| *s == 0.0));
        assert_eq!(controls.events.dropped(), 1, "only the swap's ack is lost, not the fault");
        let full = events(&mut controls);
        assert_eq!(full.len(), EVENT_CAPACITY);
        assert!(!full.contains(&Event::Fault), "the ring had no room for it");

        assert!(pump(&mut driver, 1).iter().all(|s| *s == 0.0));
        assert_eq!(events(&mut controls), vec![Event::Fault]);
        pump(&mut driver, 2);
        assert!(events(&mut controls).is_empty(), "it is sent once");
        assert_eq!(controls.events.dropped(), 1);
    }
}
