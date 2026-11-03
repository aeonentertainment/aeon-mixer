//! One mixing graph, built on the control thread and handed to the engine by its `swap`. The
//! mixer keeps the feeds of its push slots so the host can push into them before and while the
//! graph is live.

use engine::block::{MAX_SLOTS, RATE};
use engine::graph::Graph;
use engine::ingest::{PushFeed, push_voice};
use engine::slot::Slot;
use napi::bindgen_prelude::*;
use napi_derive::napi;

/// A second of mono PCM a push slot accepts at the edge, so a burst is never refused there. What
/// does not fit is dropped, and `push` reports how much was taken.
pub const PUSH_SLOT_CAPACITY: usize = RATE as usize;

/// 30 ms: the most a push slot plays behind the newest frame it holds (docs/design.md 8.4). Three
/// 10 ms chunks, room for a host two chunks late; a default until `measure-latency` gives the push
/// path a number.
pub const PUSH_SLOT_DEPTH: usize = 30 * RATE as usize / 1000;

#[napi]
pub struct Mixer {
    graph: Option<Box<Graph>>,
    feeds: Vec<PushFeed>,
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
                let (feed, voice) = push_voice(PUSH_SLOT_CAPACITY, PUSH_SLOT_DEPTH);
                (feed, Slot::new(Box::new(voice)))
            })
            .unzip();
        Ok(Self { graph: Some(Box::new(Graph::new(slots))), feeds })
    }

    /// Hands interleaved stereo PCM to a push slot, folded to mono as a slot is (docs/design.md
    /// 8.4), and returns how many frames it took.
    #[napi]
    pub fn push(&mut self, index: u32, stereo: Float32ArraySlice<'_>) -> Result<u32> {
        let feed = self
            .feeds
            .get_mut(index as usize)
            .ok_or_else(|| Error::from_reason(format!("no slot {index}")))?;
        let (frames, _) = stereo.as_chunks::<2>();
        let mono: Vec<f32> = frames.iter().map(|[left, right]| (left + right) / 2.0).collect();
        Ok(feed.push(&mono) as u32)
    }

    /// How many times a push slot dropped its oldest audio to stay within the depth.
    #[napi]
    pub fn resyncs(&self, index: u32) -> Result<u32> {
        self.feeds
            .get(index as usize)
            .map(PushFeed::resyncs)
            .ok_or_else(|| Error::from_reason(format!("no slot {index}")))
    }

    /// The graph, once: `swap` moves it to the audio thread.
    pub(crate) fn take_graph(&mut self) -> Result<Box<Graph>> {
        self.graph.take().ok_or_else(|| Error::from_reason("this mixer has already started"))
    }

    pub(crate) fn give_back(&mut self, graph: Box<Graph>) {
        self.graph = Some(graph);
    }
}
