//! A slot's effect stages: the pitch corrector, which follows the song's melody guide and reads
//! the singer's pitch, and the reverb. Each processes the slot's mono block in place on the audio
//! thread, reads its knobs from the slot's `EffectParams`, and allocates nothing.

mod corrector;
mod melody;
mod reading;
mod reverb;
mod target;

pub use corrector::Corrector;
pub use reading::Reading;
pub use reverb::Reverb;
pub use target::Target;
