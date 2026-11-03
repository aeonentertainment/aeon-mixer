//! The engine as JavaScript holds it: the control thread's ends of the crossings, the clock that
//! drives the audio thread, and the events thread that brings events back. Commands return the
//! id their ack will carry; `src/` turns that into a promise.

use engine::audio_thread::{AudioThread, Controls};
use engine::block::MAX_SLOTS;
use engine::command::{Command, CommandProducer};
use engine::params::ParamsWriter;
use engine::reactions::ReactionsFeed;
use engine::slot::guide::Guide;
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::clock::{Clock, OutputEntry};
use crate::event::NativeEvent;
use crate::events_thread::EventsThread;
use crate::guide::GuideInput;
use crate::mixer::Mixer;
use crate::slot_params::SlotParamsInput;

#[napi(object)]
pub struct OpenOptions {
    /// The offline driver, pumped from JavaScript, instead of a device.
    pub offline: bool,
    /// An id from `outputs`, or the default output.
    pub device: Option<String>,
    pub period_frames: u32,
}

#[napi(object)]
pub struct OutputInfo {
    pub sample_rate: u32,
    pub period_frames: u32,
    pub latency_ms: f64,
}

#[napi]
pub struct Engine {
    params: ParamsWriter,
    commands: CommandProducer,
    reactions: ReactionsFeed,
    clock: Option<Clock>,
    events: Option<EventsThread>,
    next_id: u32,
}

#[napi]
impl Engine {
    /// Starts the clock on the silent graph and the events thread, which calls `on_events` with
    /// every batch it drains.
    #[napi(constructor)]
    pub fn new(
        options: OpenOptions,
        on_events: Function<'_, Vec<NativeEvent>, ()>,
    ) -> Result<Self> {
        let (audio, controls) = AudioThread::new();
        let Controls { params, commands, events, retired, reactions } = controls;
        let clock = if options.offline {
            Clock::Offline(Box::new(engine::driver::OfflineDriver::new(audio.into_render())))
        } else {
            Clock::open(options.device, options.period_frames, audio.into_render())?
        };
        let deliver = on_events.build_threadsafe_function().build()?;
        let events = EventsThread::spawn(events, retired, clock.stats(), deliver);
        Ok(Self {
            params,
            commands,
            reactions,
            clock: Some(clock),
            events: Some(events),
            next_id: 0,
        })
    }

    #[napi(getter)]
    pub fn output(&self) -> Result<OutputInfo> {
        let info = self.clock()?.info();
        Ok(OutputInfo {
            sample_rate: info.sample_rate,
            period_frames: info.period_frames,
            latency_ms: f64::from(info.output_latency_frames) * 1000.0
                / f64::from(info.sample_rate),
        })
    }

    /// The outputs this OS offers, the default marked. Empty on the offline clock.
    #[napi]
    pub fn outputs(&self) -> Result<Vec<OutputEntry>> {
        self.clock()?.outputs()
    }

    /// Offline clock only: renders `blocks` blocks and returns them as interleaved stereo.
    #[napi]
    pub fn pump(&mut self, blocks: u32) -> Result<Float32Array> {
        match self.clock.as_mut().ok_or_else(closed)? {
            Clock::Offline(driver) => Ok(Float32Array::new(driver.pump(blocks as usize))),
            Clock::Device { .. } => Err(Error::from_reason("a device engine keeps its own clock")),
        }
    }

    /// Writes one slot of the snapshot and publishes the whole of it.
    #[napi]
    pub fn set_slot_params(&mut self, index: u32, params: SlotParamsInput) -> Result<()> {
        let index = slot_index(index)?;
        self.params.update(|snapshot| snapshot.slots[index] = params.into());
        Ok(())
    }

    /// Moves `mixer`'s graph to the audio thread and returns the swap's id. The graph it replaces
    /// comes back through the retired ring as a `retired` event.
    #[napi]
    pub fn swap(&mut self, mixer: &mut Mixer) -> Result<u32> {
        let mut graph = mixer.take_graph()?;
        // The guide clock subtracts the output latency, so a singer is corrected toward the note
        // they hear (docs/design.md section 3)
        graph.set_backing_offset_frames(self.clock()?.info().output_latency_frames);
        let id = self.next_id();
        if let Err(Command::Swap { graph, .. }) = self.send(Command::Swap { id, graph }) {
            mixer.give_back(graph);
            return Err(ring_full());
        }
        Ok(id)
    }

    /// Swaps the silent graph in: the end of a mixer.
    #[napi]
    pub fn swap_silent(&mut self) -> Result<u32> {
        let id = self.next_id();
        self.send(Command::Swap { id, graph: Box::new(engine::graph::Graph::silent()) })
            .map_err(|_| ring_full())?;
        Ok(id)
    }

    #[napi]
    pub fn start(&mut self) -> Result<u32> {
        let id = self.next_id();
        self.send(Command::Start { id }).map_err(|_| ring_full())?;
        Ok(id)
    }

    /// Gives the live graph's slot a guide, or takes it away with nothing. No ack.
    #[napi]
    pub fn set_guide(&mut self, slot: u32, guide: Option<GuideInput>) -> Result<()> {
        let slot = slot_index(slot)? as u8;
        let guide = guide.map(Guide::try_from).transpose()?.map(Box::new);
        self.send(Command::SetGuide { slot, guide }).map_err(|_| ring_full())
    }

    /// The live graph's slot forgets the stream it was hearing. No ack.
    #[napi]
    pub fn reset_slot(&mut self, slot: u32) -> Result<()> {
        let slot = slot_index(slot)? as u8;
        self.send(Command::ResetSlot { slot }).map_err(|_| ring_full())
    }

    /// Plays interleaved stereo on a free reaction voice; false when four already play.
    #[napi]
    pub fn play_reaction(&mut self, stereo: Float32ArraySlice<'_>) -> bool {
        self.reactions.play(&stereo)
    }

    /// Stops the clock, then the events thread. Whatever graph is live is dropped here.
    #[napi]
    pub fn close(&mut self) {
        self.clock = None;
        if let Some(events) = self.events.take() {
            events.stop();
        }
    }

    fn clock(&self) -> Result<&Clock> {
        self.clock.as_ref().ok_or_else(closed)
    }

    fn send(&mut self, command: Command) -> std::result::Result<(), Command> {
        if self.clock.is_none() {
            return Err(command);
        }
        self.commands.push(command)
    }

    fn next_id(&mut self) -> u32 {
        self.next_id = self.next_id.wrapping_add(1);
        self.next_id
    }
}

fn slot_index(index: u32) -> Result<usize> {
    usize::try_from(index)
        .ok()
        .filter(|index| *index < MAX_SLOTS)
        .ok_or_else(|| Error::from_reason(format!("slot {index} is past the engine's {MAX_SLOTS}")))
}

fn closed() -> Error {
    Error::from_reason("the engine is closed")
}

fn ring_full() -> Error {
    Error::from_reason("the command ring is full; the audio thread is not draining it")
}
