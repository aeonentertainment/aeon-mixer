//! One mixing graph, built on the control thread and handed to the engine by its `swap`. The
//! mixer keeps the feeds of its push slots so the host can push into them before and while the
//! graph is live.

use engine::block::{MAX_SLOTS, RATE};
use engine::graph::Graph;
use engine::ingest::push::{DEFAULT_CAPACITY_FRAMES, PushProducer, push_voice_with_depth};
use engine::slot::Slot;
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::backing::BackingRender;

/// 30 ms: the most a push slot plays behind the newest frame it holds (docs/design.md 8.4). Three
/// 10 ms chunks, room for a host two chunks late; a default until `measure-latency` gives the push
/// path a number.
pub const PUSH_SLOT_DEPTH: usize = 30 * RATE as usize / 1000;

#[napi]
pub struct Mixer {
    graph: Option<Box<Graph>>,
    feeds: Vec<PushProducer>,
}

#[napi]
impl Mixer {
    /// `push_slots` push slots, in order. The other kinds arrive with the ingest workstream.
    #[napi(constructor)]
    pub fn new(push_slots: u32) -> Result<Self> {
        let count =
            usize::try_from(push_slots).ok().filter(|count| *count <= MAX_SLOTS).ok_or_else(
                || Error::from_reason(format!("a mixer holds at most {MAX_SLOTS} slots")),
            )?;
        let (feeds, slots): (Vec<_>, Vec<_>) = (0..count)
            .map(|_| {
                let (feed, voice) = push_voice_with_depth(DEFAULT_CAPACITY_FRAMES, PUSH_SLOT_DEPTH);
                (feed, Slot::new(Box::new(voice)))
            })
            .unzip();
        Ok(Self { graph: Some(Box::new(Graph::new(slots))), feeds })
    }

    /// Hands interleaved stereo PCM to a push slot, which folds it to mono as every slot is
    /// (docs/design.md 8.4), and returns how many frames it took.
    #[napi]
    pub fn push(&mut self, index: u32, stereo: Float32ArraySlice<'_>) -> Result<u32> {
        let feed = self
            .feeds
            .get_mut(index as usize)
            .ok_or_else(|| Error::from_reason(format!("no slot {index}")))?;
        Ok(feed.push(&stereo) as u32)
    }

    /// How many times a push slot dropped its oldest audio to stay within the depth.
    #[napi]
    pub fn resyncs(&self, index: u32) -> Result<u32> {
        self.feeds
            .get(index as usize)
            .map(|feed| feed.stats().resyncs)
            .ok_or_else(|| Error::from_reason(format!("no slot {index}")))
    }

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

    /// The graph, once: `swap` moves it to the audio thread.
    pub(crate) fn take_graph(&mut self) -> Result<Box<Graph>> {
        self.graph.take().ok_or_else(|| Error::from_reason("this mixer has already started"))
    }

    pub(crate) fn give_back(&mut self, graph: Box<Graph>) {
        self.graph = Some(graph);
    }
}
