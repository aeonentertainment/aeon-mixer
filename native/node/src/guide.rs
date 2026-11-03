//! A melody guide as `src/` hands it over, already in frames: the one place milliseconds became
//! frames is the TypeScript edge.

use engine::slot::guide::{Guide, Note};
use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi(object)]
pub struct GuideInput {
    pub notes: Vec<NoteInput>,
    /// The key's pitch classes as bits, C in bit 0.
    pub key_mask: u32,
}

#[napi(object)]
pub struct NoteInput {
    pub midi: u32,
    pub start_frame: f64,
    pub end_frame: f64,
}

impl TryFrom<GuideInput> for Guide {
    type Error = Error;

    fn try_from(input: GuideInput) -> Result<Self> {
        let notes = input
            .notes
            .into_iter()
            .map(|note| {
                Ok(Note {
                    midi: u8::try_from(note.midi)
                        .map_err(|_| Error::from_reason("a note's midi is 0 to 127"))?,
                    start_frame: note.start_frame as u64,
                    end_frame: note.end_frame as u64,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let key_mask = u16::try_from(input.key_mask)
            .map_err(|_| Error::from_reason("a key mask is twelve bits"))?;
        Ok(Self { notes, key_mask })
    }
}
