//! The mixer's side of the song: each line turned into the parameter snapshot or command the
//! live mixer sent for it, as `src/mixer.ts` sends them, at the frame it was made. The player
//! hands them to the engine in their blocks.

use engine::command::Command;
use engine::graph::Graph;
use engine::params::Params;

use super::Bundle;
use super::calls::{self, ms_to_frames};
use super::line::{Event, Guide};
use super::manifest::StemKind;
use super::player::Step;
use super::profile::Profile;

/// `STOP_FADE_MS` in `src/backing.ts`: how long `stopAtMs` fades the stems out.
const STOP_FADE_MS: f64 = 3000.0;

pub(super) struct Desk<'a> {
    profile: &'a Profile,
    bundle: &'a Bundle,
    /// What the live mixer's parameters hold after the calls so far.
    params: Params,
    /// The graph until `start` swaps it in.
    graph: Option<Box<Graph>>,
    /// Each slot's guide, held while armed and sent at `start`.
    guides: Vec<Option<Guide>>,
    next_id: u32,
    /// The stems' length as this replay rendered them.
    rendered: Option<u64>,
    notes: &'a mut Vec<String>,
    steps: Vec<(i64, Step)>,
    /// The frame of the call being turned into steps.
    at: i64,
}

impl<'a> Desk<'a> {
    /// A desk for `bundle`, with the parameters as the mixer was created and its graph armed.
    pub(super) fn new(
        profile: &'a Profile,
        bundle: &'a Bundle,
        params: Params,
        graph: Box<Graph>,
        rendered: Option<u64>,
        notes: &'a mut Vec<String>,
    ) -> Self {
        let guides = vec![None; bundle.manifest.slots.len()];
        let (graph, steps) = (Some(graph), Vec::new());
        Self { profile, bundle, params, graph, guides, next_id: 0, rendered, notes, steps, at: 0 }
    }

    /// Every step the calls applied make, at its call's frame, in the order made.
    pub(super) fn into_steps(self) -> Vec<(i64, Step)> {
        self.steps
    }

    /// Turns the call `event`, made at frame `at`, into the steps the live mixer sent for it.
    pub(super) fn apply(&mut self, at: i64, event: &Event) -> Result<(), String> {
        self.at = at;
        let live = self.graph.is_none();
        match event {
            Event::SetSlot { index, settings } => {
                let slot = self.slot(*index)?;
                self.update(|p| calls::set_slot(&mut p.slots[slot], settings));
            }
            Event::SetSlotEffects { index, effects } => {
                let slot = self.slot(*index)?;
                let chain = calls::effects(effects, &self.profile.slot.effects)?;
                self.update(|p| p.slots[slot].effects = chain);
            }
            Event::SetSlotGuide { index, guide, guide_offset_ms } => {
                let slot = self.slot(*index)?;
                if let Some(offset) = *guide_offset_ms {
                    self.update(|p| p.slots[slot].guide_offset_ms = offset);
                }
                self.guides[slot] = guide.clone();
                if live {
                    self.send_guide(slot);
                }
            }
            Event::ResetSlot { index } => {
                let slot = self.slot(*index)? as u8;
                if live {
                    self.send(Command::ResetSlot { slot });
                }
            }
            Event::Start {} => self.start()?,
            Event::Seek { to_ms } => {
                let to_frame = ms_to_frames(*to_ms);
                self.steer(|id| Command::Seek { id, to_frame });
            }
            Event::Pause {} => self.steer(|id| Command::Pause { id }),
            Event::Resume {} => self.steer(|id| Command::Resume { id }),
            Event::SetBackingKey { semitones } => {
                let semitones = *semitones;
                self.steer(|id| Command::SetKey { id, semitones });
            }
            Event::SetStemGain { kind, gain } => self.update(|p| match kind {
                StemKind::Instrumental => p.stems.instrumental = *gain,
                StemKind::GuideVocals => p.stems.guide = *gain,
            }),
            Event::BackingRendered { frames } => {
                if let Some(rendered) = self.rendered
                    && rendered != *frames
                {
                    self.notes.push(format!(
                        "the stems rendered to {rendered} frames, where the song's were {frames}"
                    ));
                }
            }
            Event::OutputChanged {} => {
                self.notes.push("the output moved mid-song, which replays as no move".to_owned());
            }
            Event::Error { message } => {
                self.notes.push(format!("the song reported an error: {message}"));
            }
            // The replay's own engine acks the commands it is sent
            Event::Ack {} | Event::BackingEnded {} | Event::End {} => {}
        }
        Ok(())
    }

    /// `start`: the graph swapped in with its guides, the backing started, and its stop armed.
    fn start(&mut self) -> Result<(), String> {
        let graph = self.graph.take().ok_or("events.jsonl starts the song twice")?;
        self.steer(|id| Command::Swap { id, graph });
        for slot in 0..self.guides.len() {
            if self.guides[slot].is_some() {
                self.send_guide(slot);
            }
        }
        self.steer(|id| Command::Start { id });
        if let Some(stop_at_ms) = self.bundle.manifest.backing.as_ref().and_then(|b| b.stop_at_ms) {
            let at_frame = ms_to_frames(stop_at_ms);
            let fade_frames = ms_to_frames(STOP_FADE_MS) as u32;
            self.steer(|id| Command::StopAt { id, at_frame, fade_frames });
        }
        Ok(())
    }

    /// The slot's guide to the live graph, or none to take it away.
    fn send_guide(&mut self, slot: usize) {
        let guide = self.guides[slot].as_ref().map(|guide| Box::new(calls::guide(guide)));
        self.send(Command::SetGuide { slot: slot as u8, guide });
    }

    fn steer(&mut self, command: impl FnOnce(u32) -> Command) {
        self.next_id += 1;
        self.send(command(self.next_id));
    }

    fn send(&mut self, command: Command) {
        self.steps.push((self.at, Step::Command(command)));
    }

    /// Changes the parameters as the live call did, and publishes the whole snapshot.
    fn update(&mut self, change: impl FnOnce(&mut Params)) {
        change(&mut self.params);
        self.steps.push((self.at, Step::Params(self.params)));
    }

    fn slot(&self, index: usize) -> Result<usize, String> {
        if index < self.guides.len() {
            Ok(index)
        } else {
            Err(format!("events.jsonl names slot {index}, which the manifest does not list"))
        }
    }
}
