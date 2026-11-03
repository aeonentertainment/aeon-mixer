//! The napi binding: the only crate that knows Node. Control-thread code only: it builds engines
//! and mixers, forwards calls into the crossings of docs/design.md section 2 and runs the events
//! thread. Audio never comes through here. `src/` in the package root turns this into the public
//! TypeScript API.

mod backing;
mod clock;
mod engine;
mod event;
mod events_thread;
mod guide;
mod ingest;
mod mixer;
mod slot_params;

pub use backing::{BackingInput, BackingRender, StemGainsInput};
pub use clock::OutputEntry;
pub use engine::{Engine, OpenOptions, OutputInfo};
pub use event::NativeEvent;
pub use guide::{GuideInput, NoteInput};
pub use ingest::SlotStatsInfo;
pub use mixer::Mixer;
pub use slot_params::{CorrectorInput, ReverbInput, SlotParamsInput};
