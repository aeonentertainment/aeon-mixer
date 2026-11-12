//! The engine as JavaScript holds it: the control thread's ends of the crossings, the clock that
//! drives the audio thread, and the events thread that brings events back. Commands return the
//! id their ack will carry; `src/` turns that into a promise.

use engine::audio_thread::{AudioThread, Controls};
use engine::block::MAX_SLOTS;
use engine::command::{Command, CommandProducer};
use engine::driver::StreamInfo;
use engine::params::ParamsWriter;
use engine::reactions::ReactionsFeed;
use engine::slot::guide::Guide;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use transport::Network;

use crate::backing::{BackingInput, BackingRender, StemGainsInput};
use crate::clock::{Clock, OutputEntry};
use crate::event::NativeEvent;
use crate::events_thread::EventsThread;
use crate::guide::GuideInput;
use crate::mixer::Mixer;
use crate::rtp_return::RtpReturnInput;
use crate::slot_input::SlotInput;
use crate::slot_params::SlotParamsInput;

/// What drives the audio thread.
#[napi(string_enum = "lowercase")]
pub enum ClockKind {
    /// The offline driver, pumped from JavaScript.
    Offline,
    /// The timer clock, for a box with no sound card: no device, and nothing to pump.
    Timer,
    /// An OS output.
    Device,
}

#[napi(object)]
pub struct OpenOptions {
    // A literal union, so `src/` passes plain strings rather than a const enum it cannot import
    #[napi(ts_type = "'offline' | 'timer' | 'device'")]
    pub clock: ClockKind,
    /// A device clock's output: an id from `outputs` to pin, or none to follow the default output.
    pub device: Option<String>,
    pub period_frames: u32,
    /// Pins the jitter buffer's playout delay for every `rtp` and `l16` slot. Without it, each
    /// stream chooses its own.
    pub playout_delay_frames: Option<u32>,
}

#[napi(object)]
pub struct OutputInfo {
    pub sample_rate: u32,
    pub period_frames: u32,
    pub latency_ms: f64,
    /// The output playing now; none on the offline clock.
    pub device: Option<OutputDevice>,
}

#[napi(object)]
pub struct OutputDevice {
    pub id: String,
    pub name: String,
}

impl OutputInfo {
    pub fn new(info: StreamInfo, device: Option<devices::Output>) -> Self {
        Self {
            sample_rate: info.sample_rate,
            period_frames: info.period_frames,
            latency_ms: f64::from(info.output_latency_frames) * 1000.0
                / f64::from(info.sample_rate),
            device: device.map(|output| OutputDevice { id: output.id, name: output.name }),
        }
    }
}

#[napi]
pub struct Engine {
    params: ParamsWriter,
    commands: CommandProducer,
    reactions: ReactionsFeed,
    clock: Option<Clock>,
    events: Option<EventsThread>,
    /// The thread behind every slot's socket, spawned at open so a mixer never fails for it.
    network: Option<Network>,
    playout_delay_frames: Option<u32>,
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
        let network = Network::spawn()?;
        let (audio, controls) = AudioThread::new();
        let Controls { params, commands, events, retired, reactions } = controls;
        let clock = match options.clock {
            ClockKind::Offline => {
                Clock::Offline(Box::new(engine::driver::OfflineDriver::new(audio.into_render())))
            }
            ClockKind::Timer => Clock::timer(audio.into_render())?,
            ClockKind::Device => Clock::open(options.device, options.period_frames, audio)?,
        };
        let deliver = on_events.build_threadsafe_function().build()?;
        let events = EventsThread::spawn(events, retired, clock.stats(), clock.current(), deliver);
        Ok(Self {
            params,
            commands,
            reactions,
            clock: Some(clock),
            events: Some(events),
            network: Some(network),
            playout_delay_frames: options.playout_delay_frames,
            next_id: 0,
        })
    }

    /// Builds a mixer with one slot per input, in order: `"rtp"`, `"l16"`, `"push"` or
    /// `"capture"`. The rtp and l16 slots bind their ports here and report them through
    /// `Mixer.port`, the capture slots start their inputs here, and a return stream gets its socket
    /// and send task here too.
    #[napi]
    pub fn create_mixer(
        &self,
        slots: Vec<SlotInput>,
        rtp_return: Option<RtpReturnInput>,
    ) -> Result<Mixer> {
        let network = self.network.as_ref().ok_or_else(closed)?;
        let clock = self.clock()?;
        if matches!(clock, Clock::Offline(_)) && slots.iter().any(|slot| slot.kind == "capture") {
            return Err(Error::from_reason(
                "an offline engine opens no input, so has no capture slot",
            ));
        }
        let period = clock.info().period_frames;
        Mixer::build(slots, rtp_return.as_ref(), self.playout_delay_frames, period, network)
    }

    /// The output playing now. After a move the events thread sends the same as `outputChanged`.
    #[napi(getter)]
    pub fn output(&self) -> Result<OutputInfo> {
        let clock = self.clock()?;
        let device = clock.playing().map(|playing| playing.output);
        Ok(OutputInfo::new(clock.info(), device))
    }

    /// The outputs this OS offers, the default marked. Empty on the offline and timer clocks.
    #[napi]
    pub fn outputs(&self) -> Result<Vec<OutputEntry>> {
        self.clock()?.outputs()
    }

    /// The inputs this OS offers, the default marked: the devices a `capture` slot may name. Empty
    /// on the offline clock, which opens none. A box on the timer clock plays nothing locally but
    /// may still have a mic plugged in.
    #[napi]
    pub fn inputs(&self) -> Result<Vec<OutputEntry>> {
        match self.clock()? {
            Clock::Offline(_) => Ok(Vec::new()),
            Clock::Timer { .. } | Clock::Device { .. } => devices::inputs()
                .map(|inputs| inputs.into_iter().map(OutputEntry::from).collect())
                .map_err(|error| Error::from_reason(error.to_string())),
        }
    }

    /// Offline clock only: renders `blocks` blocks and returns them as interleaved stereo.
    #[napi]
    pub fn pump(&mut self, blocks: u32) -> Result<Float32Array> {
        match self.clock.as_mut().ok_or_else(closed)? {
            Clock::Offline(driver) => Ok(Float32Array::new(driver.pump(blocks as usize))),
            Clock::Timer { .. } | Clock::Device { .. } => {
                Err(Error::from_reason("this engine keeps its own clock"))
            }
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
        self.steer(|id| Command::Start { id })
    }

    /// Starts decoding and rendering a song's stems on the worker thread. The events thread
    /// reports the job's progress under the id the result carries.
    #[napi]
    pub fn start_render(&mut self, input: BackingInput) -> Result<BackingRender> {
        let id = self.next_id();
        let events = self.events.as_ref().ok_or_else(closed)?;
        let render = BackingRender::start(id, input)?;
        events.watch(id, render.job());
        Ok(render)
    }

    #[napi]
    pub fn seek(&mut self, to_frame: f64) -> Result<u32> {
        self.steer(|id| Command::Seek { id, to_frame: frames(to_frame) })
    }

    #[napi]
    pub fn pause(&mut self) -> Result<u32> {
        self.steer(|id| Command::Pause { id })
    }

    #[napi]
    pub fn resume(&mut self) -> Result<u32> {
        self.steer(|id| Command::Resume { id })
    }

    #[napi]
    pub fn set_key(&mut self, semitones: i32) -> Result<u32> {
        let semitones = i8::try_from(semitones)
            .map_err(|_| Error::from_reason("semitones must be between -128 and 127"))?;
        self.steer(|id| Command::SetKey { id, semitones })
    }

    #[napi]
    pub fn stop_at(&mut self, at_frame: f64, fade_frames: u32) -> Result<u32> {
        self.steer(|id| Command::StopAt { id, at_frame: frames(at_frame), fade_frames })
    }

    /// Writes the stem gains of the snapshot and publishes the whole of it.
    #[napi]
    pub fn set_stem_gains(&mut self, gains: StemGainsInput) {
        self.params.update(|snapshot| snapshot.stems = gains.into());
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

    /// Stops the clock, then the network thread with every port still on it, then the events
    /// thread. Whatever graph is live is dropped here.
    #[napi]
    pub fn close(&mut self) {
        self.clock = None;
        self.network = None;
        if let Some(events) = self.events.take() {
            events.stop();
        }
    }

    fn clock(&self) -> Result<&Clock> {
        self.clock.as_ref().ok_or_else(closed)
    }

    /// Sends a command built around a fresh id and returns that id, which its ack will carry.
    fn steer(&mut self, command: impl FnOnce(u32) -> Command) -> Result<u32> {
        let id = self.next_id();
        self.send(command(id)).map_err(|_| ring_full())?;
        Ok(id)
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

/// `src/` sends whole, finite, non-negative frame counts; the cast saturates on anything else.
fn frames(count: f64) -> u64 {
    count as u64
}

fn closed() -> Error {
    Error::from_reason("the engine is closed")
}

fn ring_full() -> Error {
    Error::from_reason("the command ring is full; the audio thread is not draining it")
}
