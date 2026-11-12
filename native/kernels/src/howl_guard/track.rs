use super::analyser::Peak;

/// Analyses a tone must outlast before its growth counts: 107 ms, past a sung note's attack.
const SETTLED: u32 = 10;
/// How far a settled tone must climb to be a loop running away: a singer's held note does not
/// grow 6 dB on one pitch with nothing else in the spectrum, and a howl does nothing else.
pub const RUNAWAY_DB: f32 = 6.0;
/// Analyses a tone may go unseen and still be the same tone: a singer's partial passing beside a
/// howl can hide its shape for a moment, as IPMP's three-in-five allows.
const MISSES: u32 = 2;

/// One tone followed across analyses, the temporal half of the howl detector.
#[derive(Clone, Copy, Debug)]
pub struct Track {
    bin: usize,
    analyses: u32,
    settled_db: f32,
    missed: u32,
}

impl Track {
    pub fn new(peak: Peak) -> Self {
        Self { bin: peak.bin, analyses: 1, settled_db: peak.db, missed: 0 }
    }

    /// Whether `peak` is this tone again, give or take a bin.
    pub fn continues(&self, peak: &Peak) -> bool {
        self.bin.abs_diff(peak.bin) <= 1
    }

    /// An analysis that did not see this tone; false once it has gone unseen too long.
    pub fn miss(&mut self) -> bool {
        self.missed += 1;
        self.analyses += 1;
        self.missed <= MISSES
    }

    /// Takes the next analysis of this tone; true when it is running away.
    pub fn extend(&mut self, peak: Peak) -> bool {
        self.bin = peak.bin;
        self.missed = 0;
        self.analyses += 1;
        if self.analyses <= SETTLED {
            self.settled_db = peak.db;
            return false;
        }
        peak.db - self.settled_db >= RUNAWAY_DB
    }
}
