use super::analyser::Peak;

/// Analyses a tone must outlast before its growth counts: 107 ms, past a sung note's attack. It is
/// also how long a fall must hold before the tone is judged from where it fell to.
const SETTLED: u32 = 10;
/// How far a settled tone must climb to be a loop running away: a singer's held note does not
/// grow 6 dB on one pitch with nothing else in the spectrum, and a howl does nothing else.
pub const RUNAWAY_DB: f32 = 6.0;
/// How far a tone may move between analyses and still be the same tone, as a share of its
/// frequency: a semitone. A pitch corrector that hears a howl moves it to the next semitone and
/// on, more than a bin at 1 kHz, and a howl it moves must not start its growth over each time.
const SEMITONE: f32 = 0.059_463;
/// A move further than this, a quarter semitone, from a pitch held for `SETTLED` analyses is a new
/// note unless the tone is climbing as a loop does. The pitch held is the mean over
/// `PITCH_ANALYSES`, so a sung vibrato wider than a quarter semitone still holds its note, and a new
/// note must step that far past the vibrato's own swing. A sung or whistled melody steps between notes
/// it holds, and its growth across them is a crescendo, not a loop: each note starts its own
/// baseline. A howl a corrector moves never holds still that long, or climbs as it moves, so it
/// keeps the baseline it had.
const NEW_NOTE: f32 = 0.014_545;
/// The climb that keeps a baseline through a move: the newest `ENDS` levels over the oldest `ENDS`
/// of the last `PITCH_ANALYSES`, at 0.2 dB an analysis apart, about 19 dB a second: 3 dB across a
/// full 160 ms, less for a younger track. A loop 1.5 dB over unity with an 80 ms trip climbs that
/// fast; a singer's crescendo of 2 dB a note climbs about 6, and only between notes. Means of five,
/// so a vibrato's swing in level across bins averages out.
const CLIMBING_DB_PER_ANALYSIS: f32 = 0.2;
const ENDS: usize = 5;
/// The analyses a held pitch is the mean of, 213 ms: a whole cycle of a vibrato from 4.7 Hz to
/// 7 Hz, which then averages out, where a step to the next note does not.
const PITCH_ANALYSES: usize = 20;
/// Analyses a tone may go unseen and still be the same tone, 53 ms: a singer's partial passing
/// beside a howl can hide its shape for a moment, as IPMP's three-in-five allows, and a corrector
/// moving a howl breaks it for three or four analyses of each trip round the loop.
const MISSES: u32 = 5;

/// One tone followed across analyses, the temporal half of the howl detector. Its growth is
/// measured from the quietest it has held for `SETTLED` analyses, so a tone that settled loud and
/// fell is judged from where it fell to.
#[derive(Clone, Copy, Debug)]
pub struct Track {
    bin: usize,
    /// Where between bins it was, and how loud, over the last `PITCH_ANALYSES` it was seen: rings
    /// the newest of which is at `seen % PITCH_ANALYSES`, NaN where it was not yet seen.
    recent_at: [f32; PITCH_ANALYSES],
    seen_db: [f32; PITCH_ANALYSES],
    seen: usize,
    /// The mean of `recent_at` when its hold began, and the analyses the mean has stayed within a
    /// quarter semitone of it.
    anchor: f32,
    held: u32,
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
        let mut recent_at = [f32::NAN; PITCH_ANALYSES];
        recent_at[0] = peak.at;
        let mut seen_db = [f32::NAN; PITCH_ANALYSES];
        seen_db[0] = peak.db;
        Self {
            bin: peak.bin,
            recent_at,
            seen_db,
            seen: 0,
            anchor: peak.at,
            held: 0,
            analyses: 1,
            settled_db: peak.db,
            recent_db,
            missed: 0,
        }
    }

    /// Whether `peak` is this tone again, give or take a semitone and never less than a bin.
    pub fn continues(&self, peak: &Peak) -> bool {
        let reach = ((self.bin as f32 * SEMITONE).ceil() as usize).max(1);
        self.bin.abs_diff(peak.bin) <= reach
    }

    /// Whether `peak` is a new note rather than this tone: a move of more than a quarter semitone
    /// from a pitch it had held, while not climbing as a loop does.
    pub fn is_new_note(&self, peak: &Peak) -> bool {
        let centre = self.centre();
        let moved = (peak.at - centre).abs() > NEW_NOTE * centre + self.swing(centre);
        moved && self.held >= SETTLED && !self.climbing()
    }

    /// How far a vibrato takes the tone either way of `centre`, in bins: the mean distance from it,
    /// over the 0.64 a sine's mean distance is of its peak.
    fn swing(&self, centre: f32) -> f32 {
        let (sum, count) = self
            .recent_at
            .iter()
            .filter(|at| !at.is_nan())
            .fold((0.0, 0.0), |(sum, count), at| (sum + (at - centre).abs(), count + 1.0));
        sum / count / std::f32::consts::FRAC_2_PI
    }

    /// Whether the newest levels it was seen at climb from the oldest, of up to the last
    /// `PITCH_ANALYSES`, as a loop does. A track too young to tell is not climbing.
    fn climbing(&self) -> bool {
        let count = (self.seen + 1).min(PITCH_ANALYSES);
        if count < 2 * ENDS {
            return false;
        }
        let oldest = self.seen + 1 - count;
        let at = |k: usize| self.seen_db[(oldest + k) % PITCH_ANALYSES];
        let mean = |range: std::ops::Range<usize>| range.map(at).sum::<f32>() / ENDS as f32;
        let apart = (count - ENDS) as f32;
        mean(count - ENDS..count) - mean(0..ENDS) >= CLIMBING_DB_PER_ANALYSIS * apart
    }

    /// Where between bins the tone has been, on average over the analyses it was last seen in.
    fn centre(&self) -> f32 {
        let (sum, count) = self
            .recent_at
            .iter()
            .filter(|at| !at.is_nan())
            .fold((0.0, 0.0), |(sum, count), at| (sum + at, count + 1.0));
        sum / count
    }

    /// An analysis that did not see this tone; false once it has gone unseen too long.
    pub fn miss(&mut self) -> bool {
        self.missed += 1;
        self.analyses += 1;
        // Unseen counts as quiet, so a tone that comes back lower is judged from there
        self.recent_db[(self.analyses % SETTLED) as usize] = f32::MIN;
        self.missed <= MISSES
    }

    /// The level a settling tone is judged from: the middle of the levels it was seen at in its
    /// last five analyses, so a note's edge falling on the analysis it settles at does not set the
    /// baseline at the dip, nor does a miss there. With fewer than three seen, the newest.
    fn settling_level(&self, newest: f32) -> f32 {
        let mut known = [f32::MIN; 5];
        let mut count = 0;
        for k in 0..known.len() {
            let db =
                self.recent_db[(self.analyses as usize + SETTLED as usize - k) % SETTLED as usize];
            if db > f32::MIN {
                known[count] = db;
                count += 1;
            }
        }
        if count < 3 {
            return newest;
        }
        known[..count].sort_unstable_by(f32::total_cmp);
        known[count / 2]
    }

    /// Takes the next analysis of this tone; true when it is running away.
    pub fn extend(&mut self, peak: Peak) -> bool {
        self.seen += 1;
        self.recent_at[self.seen % PITCH_ANALYSES] = peak.at;
        self.seen_db[self.seen % PITCH_ANALYSES] = peak.db;
        let centre = self.centre();
        if (centre - self.anchor).abs() > NEW_NOTE * self.anchor {
            self.anchor = centre;
            self.held = 0;
        } else {
            self.held += 1;
        }
        self.bin = peak.bin;
        self.missed = 0;
        self.analyses += 1;
        self.recent_db[(self.analyses % SETTLED) as usize] = peak.db;
        if self.analyses <= SETTLED {
            self.settled_db = self.settling_level(peak.db);
            return false;
        }
        // A fall that has held for `SETTLED` analyses moves the baseline down to the loudest of
        // them; a dip shorter than that, a singer's tremolo, leaves it where it was
        let held = self.recent_db.iter().copied().fold(f32::MIN, f32::max);
        self.settled_db = self.settled_db.min(held);
        peak.db - self.settled_db >= RUNAWAY_DB
    }
}
