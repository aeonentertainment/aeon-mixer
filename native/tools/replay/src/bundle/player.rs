//! A bundle's calls and packets, handed to the engine block by block on whatever clock calls it:
//! `song::play` steps it from the offline driver, and a render on a real clock can step it too
//! (docs/design.md 8.9). Everything is made before the first block, so handing it over allocates
//! nothing.

use std::iter::Peekable;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::vec::IntoIter;

use engine::command::{Command, CommandProducer};
use engine::ingest::PacketProducer;
use engine::params::{Params, ParamsWriter};

use crate::stream::Arrival;

/// What a call sends the engine: a whole parameter snapshot, or a command.
#[expect(
    clippy::large_enum_variant,
    reason = "a boxed snapshot would be freed on the audio thread as it is handed over"
)]
pub enum Step {
    Params(Params),
    Command(Command),
}

pub struct Player {
    params: ParamsWriter,
    commands: CommandProducer,
    /// Each step at the frame its call was made, in the bundle's order.
    steps: Peekable<IntoIter<(i64, Step)>>,
    /// Each packet with its slot, at the frame it reached the slot, in frame order.
    arrivals: Peekable<IntoIter<(usize, Arrival)>>,
    rings: Vec<Option<PacketProducer>>,
    ring_full: Vec<usize>,
    /// The engine's count of frames rendered: the first frame of the block about to render.
    frames: Arc<AtomicU64>,
    /// A command the ring had no room for, kept so it is not dropped on the audio thread.
    jammed: Option<Command>,
}

impl Player {
    pub(super) fn new(
        params: ParamsWriter,
        commands: CommandProducer,
        steps: Vec<(i64, Step)>,
        arrivals: Vec<(usize, Arrival)>,
        rings: Vec<Option<PacketProducer>>,
        frames: Arc<AtomicU64>,
    ) -> Self {
        let ring_full = vec![0; rings.len()];
        Self {
            params,
            commands,
            steps: steps.into_iter().peekable(),
            arrivals: arrivals.into_iter().peekable(),
            rings,
            ring_full,
            frames,
            jammed: None,
        }
    }

    /// Hands the engine every step and packet due at or before the block about to render, the
    /// block `replay` takes each one up in. Call it before each block; a second call before the
    /// same block hands nothing new, and after a fault, when the engine's count stops, nothing
    /// more is handed. False once a command found the ring full: from then on it hands nothing,
    /// since the song is no longer the bundle's.
    pub fn before_block(&mut self) -> bool {
        if self.jammed.is_some() {
            return false;
        }
        let start = self.frames.load(Relaxed) as i64;
        while let Some((_, step)) = self.steps.next_if(|&(at, _)| at <= start) {
            match step {
                Step::Params(params) => self.params.update(|current| *current = params),
                Step::Command(command) => {
                    if let Err(command) = self.commands.push(command) {
                        self.jammed = Some(command);
                        return false;
                    }
                }
            }
        }
        while let Some((slot, arrival)) = self.arrivals.next_if(|(_, arrival)| arrival.at <= start)
        {
            if let Some(ring) = &mut self.rings[slot]
                && !ring.push(&arrival.packet)
            {
                self.ring_full[slot] += 1;
            }
        }
        true
    }

    /// Packets `slot`'s ring had no room for.
    pub fn ring_full(&self, slot: usize) -> usize {
        self.ring_full[slot]
    }
}
