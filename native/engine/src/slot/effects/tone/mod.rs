//! The tone stage: what makes a phone sound like a good vocal mic. Decision 0029 has each part's
//! reasons and numbers, and docs/design.md 8.6 the contract.

mod band;
mod band_ratio;
mod biquad;
mod cascade;
mod cut;
mod de_esser;
mod findings;
mod follower;
mod plosive_guard;
mod section;
mod shelf;
#[cfg(test)]
pub(crate) mod tests;

use crate::params::ToneParams;
pub use biquad::Biquad;
pub use cascade::{MOST_LATE_MS, MOST_LIFT_DB, SECTIONS};
use de_esser::DeEsser;
pub use findings::ToneFindings;
use plosive_guard::PlosiveGuard;
pub use section::Section;

/// The slot's tone stage, after the meter and before the compressor: the plosive guard, the bands,
/// the air shelf and the de-esser, in that order, so the de-esser judges the sibilance that leaves.
/// No part holds a sample or reads one ahead. A change of bands or of on and off glides across one
/// block. Off, or on with nothing asked, it touches no sample.
pub struct Tone {
    /// What the stage was last asked, off folded to the default, and the cascade built from it.
    asked: ToneParams,
    cascade: [Biquad; SECTIONS],
    sections: [Section; SECTIONS],
    guard: PlosiveGuard,
    de_esser: DeEsser,
}

impl Tone {
    pub fn new() -> Self {
        Self {
            asked: ToneParams::default(),
            cascade: [Biquad::IDENTITY; SECTIONS],
            sections: std::array::from_fn(|_| Section::new()),
            guard: PlosiveGuard::new(),
            de_esser: DeEsser::new(),
        }
    }

    /// One run of the slot's voice, in place.
    pub fn process(&mut self, params: &ToneParams, run: &mut [f32]) {
        let asked = if params.enabled { *params } else { ToneParams::default() };
        if asked != self.asked {
            self.asked = asked;
            self.cascade = asked.cascade();
        }
        self.guard.process(asked.plosive_db, run);
        for (section, to) in self.sections.iter_mut().zip(self.cascade) {
            section.process(to, run);
        }
        self.de_esser.process(asked.de_ess_db, run);
    }

    /// None while the stage is off.
    pub fn findings(&self) -> Option<ToneFindings> {
        self.asked.enabled.then(|| ToneFindings {
            de_ess_db: self.de_esser.most_db(),
            plosives: self.guard.catches(),
        })
    }
}

impl Default for Tone {
    fn default() -> Self {
        Self::new()
    }
}
