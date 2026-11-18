//! A tone stage's settings as `src/` hands them over, and the check `setSlotEffects` runs before
//! they cross: the engine's own, so the filters judged are the filters that run.

use engine::params::{TONE_BANDS, ToneBand, ToneBands, ToneParams};
use napi::bindgen_prelude::*;
use napi_derive::napi;

/// One band, spelt as the public `ToneBand` is.
#[napi(discriminant = "kind", discriminant_case = "kebab-case")]
pub enum ToneBandInput {
    Peak { hz: f64, gain_db: f64, q: f64 },
    LowShelf { hz: f64, gain_db: f64 },
    HighShelf { hz: f64, gain_db: f64 },
}

#[napi(object)]
pub struct ToneInput {
    pub enabled: bool,
    pub bands: Vec<ToneBandInput>,
    pub plosive_db: f64,
    pub air_db: f64,
    pub de_ess_db: f64,
}

impl ToneInput {
    pub fn params(&self) -> Result<ToneParams> {
        let bands: Vec<ToneBand> = self.bands.iter().map(ToneBand::from).collect();
        let bands = ToneBands::given(&bands).ok_or_else(|| {
            Error::from_reason(format!("a tone stage takes at most {TONE_BANDS} bands"))
        })?;
        Ok(ToneParams {
            enabled: self.enabled,
            bands,
            plosive_db: self.plosive_db as f32,
            air_db: self.air_db as f32,
            de_ess_db: self.de_ess_db as f32,
        })
    }
}

impl From<&ToneBandInput> for ToneBand {
    fn from(band: &ToneBandInput) -> Self {
        match *band {
            ToneBandInput::Peak { hz, gain_db, q } => {
                Self::Peak { hz: hz as f32, gain_db: gain_db as f32, q: q as f32 }
            }
            ToneBandInput::LowShelf { hz, gain_db } => {
                Self::LowShelf { hz: hz as f32, gain_db: gain_db as f32 }
            }
            ToneBandInput::HighShelf { hz, gain_db } => {
                Self::HighShelf { hz: hz as f32, gain_db: gain_db as f32 }
            }
        }
    }
}

/// Throws when the bands and the air together leave a frequency from 80 Hz over 0.2 ms late or
/// lift one over 6 dB, naming the worst. Needs no engine, so an armed mixer checks at the call.
#[napi]
pub fn check_tone(input: ToneInput) -> Result<()> {
    input.params()?.check().map_err(|refusal| Error::from_reason(refusal.to_string()))
}
