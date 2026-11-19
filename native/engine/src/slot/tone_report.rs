use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub use crate::slot::effects::ToneFindings;

/// What a slot's tone stage did, published by the audio thread once a block for the control
/// thread's `stats()`: nothing while the stage is off.
#[derive(Debug, Default)]
pub struct ToneReport {
    on: AtomicBool,
    de_ess_db: AtomicU32,
    plosives: AtomicU32,
}

impl ToneReport {
    pub fn publish(&self, findings: Option<ToneFindings>) {
        if let Some(findings) = findings {
            self.de_ess_db.store(findings.de_ess_db.to_bits(), Ordering::Relaxed);
            self.plosives.store(findings.plosives, Ordering::Relaxed);
        }
        self.on.store(findings.is_some(), Ordering::Relaxed);
    }

    /// One read may mix two blocks' fields; the cut is a second's most and the count only grows,
    /// so it does not matter.
    pub fn findings(&self) -> Option<ToneFindings> {
        self.on.load(Ordering::Relaxed).then(|| ToneFindings {
            de_ess_db: f32::from_bits(self.de_ess_db.load(Ordering::Relaxed)),
            plosives: self.plosives.load(Ordering::Relaxed),
        })
    }
}
