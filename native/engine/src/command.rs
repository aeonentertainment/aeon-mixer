//! The command, event and retired crossings of docs/design.md section 2, each a fixed-size SPSC
//! ring with one producer and one consumer. A full ring never blocks: a command stays with the
//! control thread, an event is dropped and counted, and a retired graph waits for room.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::graph::Graph;
use crate::slot::guide::Guide;

pub const COMMAND_CAPACITY: usize = 64;
pub const EVENT_CAPACITY: usize = 1024;
pub const RETIRED_CAPACITY: usize = 16;

/// Control to audio. A command with an id is acknowledged with an `Event::Ack`; `SetGuide` and
/// `ResetSlot` carry none and their methods return at once.
pub enum Command {
    Swap { id: u32, graph: Box<Graph> },
    Start { id: u32 },
    Seek { id: u32, to_frame: u64 },
    Pause { id: u32 },
    Resume { id: u32 },
    SetKey { id: u32, semitones: i8 },
    StopAt { id: u32, at_frame: u64, fade_frames: u32 },
    SetGuide { slot: u8, guide: Option<Box<Guide>> },
    ResetSlot { slot: u8 },
}

impl Command {
    pub fn id(&self) -> Option<u32> {
        match self {
            Self::Swap { id, .. }
            | Self::Start { id }
            | Self::Seek { id, .. }
            | Self::Pause { id }
            | Self::Resume { id }
            | Self::SetKey { id, .. }
            | Self::StopAt { id, .. } => Some(*id),
            Self::SetGuide { .. } | Self::ResetSlot { .. } => None,
        }
    }
}

/// Audio to control. `Copy`, so the ring moves plain values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Ack { id: u32, position_frames: u64 },
    Level { slot: u8, rms_db: f32, peak_db: f32 },
    MasterLevel { rms_db: f32, peak_db: f32 },
    Pitch { slot: u8, position_ms: f64, hz: f32, confidence: f32 },
    Position { frames: u64 },
    BackingEnded,
    Underrun,
    Fault,
}

/// What the audio thread no longer holds, dropped on the control thread.
pub enum Retired {
    Graph(Box<Graph>),
    Guide(Box<Guide>),
}

pub fn command_ring() -> (CommandProducer, CommandConsumer) {
    let (producer, consumer) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    (CommandProducer(producer), CommandConsumer(consumer))
}

pub struct CommandProducer(rtrb::Producer<Command>);

impl CommandProducer {
    /// Hands the command back when the ring is full, so a boxed graph is never lost.
    pub fn push(&mut self, command: Command) -> Result<(), Command> {
        self.0.push(command).map_err(|rtrb::PushError::Full(command)| command)
    }
}

pub struct CommandConsumer(rtrb::Consumer<Command>);

impl CommandConsumer {
    pub fn peek(&self) -> Option<&Command> {
        self.0.peek().ok()
    }

    pub fn pop(&mut self) -> Option<Command> {
        self.0.pop().ok()
    }
}

pub fn event_ring() -> (EventProducer, EventConsumer) {
    let (producer, consumer) = rtrb::RingBuffer::new(EVENT_CAPACITY);
    let dropped = Arc::new(AtomicU32::new(0));
    (
        EventProducer { ring: producer, dropped: Arc::clone(&dropped) },
        EventConsumer { ring: consumer, dropped },
    )
}

pub struct EventProducer {
    ring: rtrb::Producer<Event>,
    dropped: Arc<AtomicU32>,
}

impl EventProducer {
    /// Drops and counts when the ring is full; the audio thread never waits for a reader.
    pub fn push(&mut self, event: Event) {
        if self.ring.push(event).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Whether `event` went in. For the one event the caller will send again rather than lose, so
    /// a full ring is not counted as a drop.
    pub fn try_push(&mut self, event: Event) -> bool {
        self.ring.push(event).is_ok()
    }
}

pub struct EventConsumer {
    ring: rtrb::Consumer<Event>,
    dropped: Arc<AtomicU32>,
}

impl EventConsumer {
    pub fn pop(&mut self) -> Option<Event> {
        self.ring.pop().ok()
    }

    /// Events the audio thread could not push because the ring was full.
    pub fn dropped(&self) -> u32 {
        self.dropped.load(Ordering::Relaxed)
    }
}

pub fn retired_ring() -> (RetiredProducer, RetiredConsumer) {
    let (producer, consumer) = rtrb::RingBuffer::new(RETIRED_CAPACITY);
    (RetiredProducer(producer), RetiredConsumer(consumer))
}

pub struct RetiredProducer(rtrb::Producer<Retired>);

impl RetiredProducer {
    pub fn push(&mut self, retired: Retired) -> Result<(), Retired> {
        self.0.push(retired).map_err(|rtrb::PushError::Full(retired)| retired)
    }

    /// How many more can be pushed before the ring is full. The audio thread checks this before
    /// it takes a command whose box it may have to retire.
    pub fn room(&self) -> usize {
        self.0.slots()
    }
}

pub struct RetiredConsumer(rtrb::Consumer<Retired>);

impl RetiredConsumer {
    pub fn pop(&mut self) -> Option<Retired> {
        self.0.pop().ok()
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    #[test]
    fn a_full_event_ring_drops_and_counts() {
        let (mut producer, mut consumer) = event_ring();
        assert_no_alloc(|| {
            for _ in 0..EVENT_CAPACITY + 3 {
                producer.push(Event::Underrun);
            }
        });
        assert_eq!(consumer.dropped(), 3);
        assert_eq!((0..).map_while(|_| consumer.pop()).count(), EVENT_CAPACITY);
    }

    #[test]
    fn a_full_command_ring_hands_the_command_back() {
        let (mut producer, mut consumer) = command_ring();
        for id in 0..COMMAND_CAPACITY as u32 {
            assert!(producer.push(Command::Start { id }).is_ok());
        }
        let refused = producer.push(Command::Pause { id: 99 }).unwrap_err();
        assert_eq!(refused.id(), Some(99));
        assert_eq!(consumer.peek().and_then(Command::id), Some(0));
        assert_eq!(consumer.pop().and_then(|c| c.id()), Some(0));
    }
}
