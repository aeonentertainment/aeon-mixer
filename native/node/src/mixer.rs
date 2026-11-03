//! One mixing graph, built on the control thread by `Engine::create_mixer` and handed to the
//! engine by its `swap`. The mixer keeps the control thread's end of every slot's ingest: the
//! push feeds the host pushes into, and the ports the network thread listens on for the rest;
//! and the end of its return stream, when it has one.

use engine::block::MAX_SLOTS;
use engine::graph::Graph;
use engine::slot::Slot;
use napi::bindgen_prelude::*;
use napi_derive::napi;
use transport::Network;

use crate::backing::BackingRender;
use crate::ingest::{Ingest, SlotStatsInfo};
use crate::rtp_return::{RtpReturn, RtpReturnInput, RtpReturnStats};

#[napi]
pub struct Mixer {
    graph: Option<Box<Graph>>,
    slots: Vec<Ingest>,
    rtp_return: Option<RtpReturn>,
}

impl Mixer {
    /// One slot per kind, in order: `"rtp"`, `"l16"` or `"push"`.
    pub(crate) fn build(
        kinds: &[String],
        rtp_return: Option<&RtpReturnInput>,
        playout_delay_frames: u32,
        network: &Network,
    ) -> Result<Self> {
        if kinds.len() > MAX_SLOTS {
            return Err(Error::from_reason(format!("a mixer holds at most {MAX_SLOTS} slots")));
        }
        let (ingests, slots): (Vec<_>, Vec<_>) = kinds
            .iter()
            .map(|kind| {
                let (ingest, source) = Ingest::open(kind, playout_delay_frames, network)?;
                Ok((ingest, Slot::new(source)))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .unzip();
        let mut graph = Box::new(Graph::new(slots));
        let rtp_return =
            rtp_return.map(|input| RtpReturn::open(input, &mut graph, network)).transpose()?;
        Ok(Self { graph: Some(graph), slots: ingests, rtp_return })
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

    /// The UDP port a slot listens on, or null for a push slot.
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

    /// Every slot's ingest counters, as the audio thread last published them.
    #[napi]
    pub fn stats(&self) -> Vec<SlotStatsInfo> {
        self.slots.iter().map(|slot| slot.stats().into()).collect()
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
