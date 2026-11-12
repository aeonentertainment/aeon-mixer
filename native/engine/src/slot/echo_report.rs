use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

pub use kernels::EchoFindings;

/// What a slot's echo stages have found, published by the audio thread once a block for the
/// control thread's `stats()`: the canceller's findings while it has them, and how often the
/// howl guard has ducked.
#[derive(Debug, Default)]
pub struct EchoReport {
    found: AtomicBool,
    path_frames: AtomicU32,
    bleed_db: AtomicU32,
    removed_db: AtomicU32,
    howl_catches: AtomicU32,
}

impl EchoReport {
    pub fn publish(&self, findings: Option<EchoFindings>) {
        if let Some(findings) = findings {
            self.path_frames.store(findings.path_frames, Ordering::Relaxed);
            self.bleed_db.store(findings.bleed_db.to_bits(), Ordering::Relaxed);
            self.removed_db.store(findings.removed_db.to_bits(), Ordering::Relaxed);
        }
        self.found.store(findings.is_some(), Ordering::Relaxed);
    }

    pub fn publish_howl_catches(&self, catches: u32) {
        self.howl_catches.store(catches, Ordering::Relaxed);
    }

    /// One read may mix two blocks' fields; each is a one-second average, so it does not matter.
    pub fn findings(&self) -> Option<EchoFindings> {
        self.found.load(Ordering::Relaxed).then(|| EchoFindings {
            path_frames: self.path_frames.load(Ordering::Relaxed),
            bleed_db: f32::from_bits(self.bleed_db.load(Ordering::Relaxed)),
            removed_db: f32::from_bits(self.removed_db.load(Ordering::Relaxed)),
        })
    }

    pub fn howl_catches(&self) -> u32 {
        self.howl_catches.load(Ordering::Relaxed)
    }
}
