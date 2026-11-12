//! A slot's effect stages: the high-pass, gate and compressor that clean the voice, the pitch
//! corrector, which follows the song's melody guide and reads the singer's pitch, the voice, and
//! the reverb. Each processes the slot's mono block in place on the audio thread, reads its knobs
//! from the slot's `EffectParams`, and allocates nothing.

mod compressor;
mod corrector;
mod gate;
mod high_pass;
mod level;
mod melody;
mod reading;
mod reverb;
mod target;
mod voice;

pub use compressor::Compressor;
pub use corrector::Corrector;
pub use gate::Gate;
pub use high_pass::HighPass;
pub use reading::Reading;
pub use reverb::Reverb;
pub use target::Target;
pub use voice::Voice;
