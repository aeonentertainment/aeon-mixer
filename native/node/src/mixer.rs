//! One mixing graph, built on the control thread by `Engine::create_mixer` and handed to the
//! engine by its `swap`. The mixer keeps the control thread's end of every slot's ingest: the
//! push feeds the host pushes into, the ports the network thread listens on, and the capture
//! slots' input streams; and the end of its return stream, when it has one.

use std::sync::Arc;

use engine::block::MAX_SLOTS;
use engine::graph::Graph;
use engine::slot::Slot;
use engine::slot::corrector_delay::CorrectorDelay;
use engine::slot::echo_report::EchoReport;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use transport::Network;

use crate::backing::BackingRender;
use crate::ingest::Ingest;
use crate::rtp_return::{RtpReturn, RtpReturnInput, RtpReturnStats};
use crate::slot_input::SlotInput;
use crate::slot_stats_info::SlotStatsInfo;

#[napi]
pub struct Mixer {
    graph: Option<Box<Graph>>,
    slots: Vec<Ingest>,
    /// Slot for slot with `slots`: what each corrector last published.
    corrector_delays: Vec<Arc<CorrectorDelay>>,
    /// And what each slot's echo stages last published.
    echo_reports: Vec<Arc<EchoReport>>,
    rtp_return: Option<RtpReturn>,
}

impl Mixer {
    /// One slot per input, in order: `"rtp"`, `"l16"`, `"push"` or `"capture"`. A capture slot's
    /// input runs at `input_period_frames`, the output's period, or as near as its device allows.
    pub(crate) fn build(
        inputs: Vec<SlotInput>,
        rtp_return: Option<&RtpReturnInput>,
        playout_delay_frames: Option<u32>,
        input_period_frames: u32,
        network: &Network,
    ) -> Result<Self> {
        if inputs.len() > MAX_SLOTS {
            return Err(Error::from_reason(format!("a mixer holds at most {MAX_SLOTS} slots")));
        }
        let (ingests, slots): (Vec<_>, Vec<_>) = inputs
            .into_iter()
            .map(|input| {
                let (ingest, source) = match input.kind.as_str() {
                    "capture" => {
                        let channel = u16::try_from(input.channel.unwrap_or(0))
                            .map_err(|_| Error::from_reason("a channel is at most 65535"))?;
                        Ingest::capture(input.device, channel, input_period_frames)?
                    }
                    kind => Ingest::open(kind, playout_delay_frames, network)?,
                };
                Ok((ingest, Slot::new(source)))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unzip();
        let corrector_delays = slots.iter().map(Slot::corrector_delay).collect();
        let echo_reports = slots.iter().map(Slot::echo_report).collect();
        let mut graph = Box::new(Graph::new(slots));
        let rtp_return =
            rtp_return.map(|input| RtpReturn::open(input, &mut graph, network)).transpose()?;
        Ok(Self { graph: Some(graph), slots: ingests, corrector_delays, echo_reports, rtp_return })
    }

    /// The graph, once: `swap` moves it to the audio thread.
    pub(crate) fn take_graph(&mut self) -> Result<Box<Graph>> {
        self.graph.take().ok_or_else(|| Error::from_reason("this mixer has already started"))
    }

    pub(crate) fn give_back(&mut self, graph: Box<Graph>) {
        self.graph = Some(graph);
    }
}

#[napi]
impl Mixer {
    /// Moves a finished render's stems into the graph as its backing path, before `swap`.
    #[napi]
    pub fn install_backing(&mut self, render: &mut BackingRender) -> Result<()> {
        let graph = self
            .graph
            .as_mut()
            .ok_or_else(|| Error::from_reason("this mixer has already started"))?;
        graph.set_backing(render.take_backing()?);
        Ok(())
    }

    /// The UDP port a slot listens on, or null for a push or capture slot.
    #[napi]
    pub fn port(&self, index: u32) -> Result<Option<u16>> {
        self.slot(index).map(Ingest::port)
    }

    /// Hands interleaved stereo PCM to a push slot, which folds it to mono as every slot is
    /// (docs/design.md 8.4), and returns how many frames it took.
    #[napi]
    pub fn push(&mut self, index: u32, stereo: Float32ArraySlice<'_>) -> Result<u32> {
        let feed = self
            .slots
            .get_mut(index as usize)
            .ok_or_else(|| no_slot(index))?
            .feed()
            .ok_or_else(|| Error::from_reason(format!("slot {index} is not a push slot")))?;
        Ok(feed.push(&stereo) as u32)
    }

    /// Records what reaches an `rtp` or `l16` slot's port to a new pcap file at `path`, or stops
    /// recording it for null.
    #[napi]
    pub fn record_slot(&self, index: u32, path: Option<String>) -> Result<()> {
        self.slot(index)?.record(path.as_deref())
    }

    /// Every slot's ingest counters, corrector delay and echo findings, as the audio thread last
    /// published them.
    #[napi]
    pub fn stats(&self) -> Vec<SlotStatsInfo> {
        self.slots
            .iter()
            .zip(&self.corrector_delays)
            .zip(&self.echo_reports)
            .map(|((slot, delay), echo)| SlotStatsInfo::new(slot.stats(), delay.frames(), echo))
            .collect()
    }

    /// The return stream's counters, or null for a mixer without one.
    #[napi]
    pub fn return_stats(&self) -> Option<RtpReturnStats> {
        self.rtp_return.as_ref().map(RtpReturn::stats)
    }

    /// The end of the mixer on this thread: closes the slots' ports, drops the push feeds, stops
    /// the return stream, and drops a graph that never started.
    #[napi]
    pub fn release(&mut self) {
        self.slots.clear();
        self.corrector_delays.clear();
        self.echo_reports.clear();
        self.rtp_return = None;
        self.graph = None;
    }

    fn slot(&self, index: u32) -> Result<&Ingest> {
        self.slots.get(index as usize).ok_or_else(|| no_slot(index))
    }
}

fn no_slot(index: u32) -> Error {
    Error::from_reason(format!("no slot {index}"))
}
