//! The napi binding: the only crate that knows Node. Control-thread code only: it builds engines
//! and mixers, forwards calls into the crossings of docs/design.md section 2 and runs the events
//! thread. Audio never comes through here. `src/` in the package root turns this into the public
//! TypeScript API.

mod backing;
mod capture_input;
mod clock;
mod echo_info;
mod engine;
mod event;
mod events_thread;
mod guide;
mod hanging_start;
mod ingest;
mod listing;
mod mixer;
mod opening;
mod output_seen;
mod rtp_return;
mod slot_input;
mod slot_params;
mod slot_stats_info;
mod test_os;

pub use backing::{BackingInput, BackingRender, StemGainsInput};
pub use clock::OutputEntry;
pub use echo_info::EchoInfo;
pub use engine::{Engine, OpenOptions, OutputDevice, OutputInfo, RefusedOutput};
pub use event::NativeEvent;
pub use guide::{GuideInput, NoteInput};
pub use listing::{inputs, outputs};
pub use mixer::Mixer;
pub use rtp_return::{RtpReturnInput, RtpReturnStats};
pub use slot_input::SlotInput;
pub use slot_params::{CorrectorInput, ReverbInput, SlotParamsInput, SwitchInput, VoiceInput};
pub use slot_stats_info::SlotStatsInfo;
