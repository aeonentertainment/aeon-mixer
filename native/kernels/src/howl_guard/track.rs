use super::analyser::Peak;

/// Analyses a tone must outlast before its growth counts: 107 ms, past a sung note's attack. It is
/// also how long a fall must hold before the tone is judged from where it fell to.
const SETTLED: u32 = 10;
/// How far a settled tone must climb to be a loop running away: a singer's held note does not
/// grow 6 dB on one pitch with nothing else in the spectrum, and a howl does nothing else.
pub const RUNAWAY_DB: f32 = 6.0;
/// Analyses a tone may go unseen and still be the same tone: a singer's partial passing beside a
/// howl can hide its shape for a moment, as IPMP's three-in-five allows.
const MISSES: u32 = 2;

/// One tone followed across analyses, the temporal half of the howl detector. Its growth is
/// measured from the quietest it has held for `SETTLED` analyses, so a tone that settled loud and
/// fell is judged from where it fell to.
#[derive(Clone, Copy, Debug)]
pub struct Track {
    bin: usize,
    analyses: u32,
    settled_db: f32,
    /// The last `SETTLED` levels, a ring the newest of which is at `analyses % SETTLED`.
    recent_db: [f32; SETTLED as usize],
    missed: u32,
}

impl Track {
    pub fn new(peak: Peak) -> Self {
        let mut recent_db = [f32::MIN; SETTLED as usize];
        recent_db[1] = peak.db;
        Self { bin: peak.bin, analyses: 1, settled_db: peak.db, recent_db, missed: 0 }
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
        self.recent_db[(self.analyses % SETTLED) as usize] = peak.db;
        if self.analyses <= SETTLED {
            self.settled_db = peak.db;
            return false;
        }
        // A fall that has held for `SETTLED` analyses moves the baseline down to the loudest of
        // them; a dip shorter than that, a singer's tremolo, leaves it where it was
        let held = self.recent_db.iter().copied().fold(f32::MIN, f32::max);
        self.settled_db = self.settled_db.min(held);
        peak.db - self.settled_db >= RUNAWAY_DB
    }
}
