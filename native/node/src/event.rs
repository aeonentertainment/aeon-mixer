//! One event as it crosses to JavaScript: the engine's `Event`, plus what the events thread adds
//! from the device's stats and the retired ring. Frame counts cross as f64, since a u64 is not a
//! JavaScript number.

use engine::command::Event;
use napi_derive::napi;

use crate::engine::OutputInfo;

#[napi(discriminant = "kind", discriminant_case = "camelCase")]
pub enum NativeEvent {
    Ack {
        id: u32,
        position_frames: f64,
    },
    Level {
        slot: u32,
        rms_db: f64,
        peak_db: f64,
    },
    MasterLevel {
        rms_db: f64,
        peak_db: f64,
    },
    Pitch {
        slot: u32,
        position_ms: f64,
        hz: f64,
        confidence: f64,
    },
    Position {
        frames: f64,
    },
    BackingEnded,
    Underrun,
    Fault,
    /// A graph the audio thread handed back has been dropped: what `destroy` waits for.
    Retired,
    /// A render job on the worker thread moved on: how much of it is done, 0 to 1.
    RenderProgress {
        job: u32,
        fraction: f64,
    },
    /// The render job is done: `installBacking` has its stems, or its error.
    RenderFinished {
        job: u32,
    },
    /// The engine moved to another output: where it plays now, and how long the room heard
    /// nothing.
    OutputChanged {
        output: OutputInfo,
        gap_ms: f64,
    },
}

impl From<Event> for NativeEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::Ack { id, position_frames } => {
                Self::Ack { id, position_frames: position_frames as f64 }
            }
            Event::Level { slot, rms_db, peak_db } => Self::Level {
                slot: u32::from(slot),
                rms_db: f64::from(rms_db),
                peak_db: f64::from(peak_db),
            },
            Event::MasterLevel { rms_db, peak_db } => {
                Self::MasterLevel { rms_db: f64::from(rms_db), peak_db: f64::from(peak_db) }
            }
            Event::Pitch { slot, position_ms, hz, confidence } => Self::Pitch {
                slot: u32::from(slot),
                position_ms,
                hz: f64::from(hz),
                confidence: f64::from(confidence),
            },
            Event::Position { frames } => Self::Position { frames: frames as f64 },
            Event::BackingEnded => Self::BackingEnded,
            Event::Underrun => Self::Underrun,
            Event::Fault => Self::Fault,
        }
    }
}
