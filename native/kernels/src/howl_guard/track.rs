use super::analyser::Peak;

/// Analyses a tone must outlast before its growth counts: 107 ms, past a sung note's attack.
const SETTLED: u32 = 10;
/// How far a settled tone must climb to be a loop running away: a singer's held note does not
/// grow 6 dB on one pitch with nothing else in the spectrum, and a howl does nothing else.
pub const RUNAWAY_DB: f32 = 6.0;
/// How far a tone may move between analyses and still be the same tone, as a share of its
/// frequency: a semitone. A pitch corrector that hears a howl moves it to the next semitone and
/// on, more than a bin at 1 kHz, and a howl it moves must not start its growth over each time.
const SEMITONE: f32 = 0.059_463;
/// A move further than this, a quarter semitone, is a step to a new note, if the tone held the
/// note it left without growing as a loop does. A held pitch is the mean over `PITCH_ANALYSES`,
/// so a sung vibrato wider than a quarter semitone still holds its note, and a new note must step
/// that far past the vibrato's own swing. A sung or whistled melody steps between notes it holds,
/// and its growth across them is a crescendo, not a loop: each note starts its own baseline. A
/// howl a corrector moves never holds still that long, or climbs as it moves, so it keeps the
/// baseline it had.
const NEW_NOTE: f32 = 0.014_545;
/// The climb of a loop: the newest `ENDS` levels over the oldest `ENDS` of the last
/// `PITCH_ANALYSES`, at 0.2 dB an analysis apart, about 19 dB a second: 3 dB across a full
/// 160 ms, less for a younger track. A loop 1.5 dB over unity with an 80 ms trip climbs that
/// fast; a singer's crescendo of 2 dB a note climbs about 6, and only between notes. Means of
/// five, so a vibrato's swing in level across bins averages out.
const CLIMBING_DB_PER_ANALYSIS: f32 = 0.2;
const ENDS: usize = 5;
/// The analyses a held pitch is the mean of, 213 ms: a whole cycle of a vibrato from 4.7 Hz to
/// 7 Hz, which then averages out, where a step to the next note does not.
const PITCH_ANALYSES: usize = 20;
/// The sightings a note of a fast run is held for before it steps, 43 ms: a run of 94 ms notes
/// holds each for six or seven, a vibrato never holds a quarter semitone that long.
const NOTE_SIGHTINGS: u32 = 4;
/// Analyses a tone may go unseen and still be the same tone, 53 ms: a singer's partial passing
/// beside a howl can hide its shape for a moment, as IPMP's three-in-five allows, and a corrector
/// moving a howl breaks it for three or four analyses of each trip round the loop.
const MISSES: u32 = 5;
/// How far apart the last `SETTLED` levels may sit and still be a tone that settled again: a
/// tone that fell and holds steady is judged from there, and a note fading into the next one's
/// attack, which never holds steady, is not.
const STEADY_DB: f32 = 3.0;

/// One tone followed across analyses, the temporal half of the howl detector. Every level it
/// keeps is one it was seen at: an analysis that missed it says nothing about how loud it was,
/// so the window that happened to straddle the gap before the next syllable never becomes the
/// level growth is measured from, wherever the analyses fall.
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
    /// The note of a fast run it holds now: where it sits, for how many sightings, and the level
    /// it came in at.
    note_at: f32,
    note_sightings: u32,
    note_db: f32,
    analyses: u32,
    /// The level growth is measured from, once the attack is over.
    settled_db: Option<f32>,
    missed: u32,
}

impl Track {
    pub fn new(peak: Peak) -> Self {
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
            note_at: peak.at,
            note_sightings: 1,
            note_db: peak.db,
            analyses: 1,
            settled_db: None,
            missed: 0,
        }
    }

    /// Whether `peak`, seen with something at its harmonics, is still this tone: it has settled,
    /// standing alone through its attack, and `peak` sits within half a semitone of where it
    /// stood alone. A howl that a singer's partial lands on keeps its place; a voice's partial
    /// glides on with its harmonics, and a tone that has not shown it stands alone is not followed
    /// on through them.
    pub fn holds(&self, peak: &Peak) -> bool {
        if self.settled_db.is_none() {
            return false;
        }
        let centre = self.centre();
        (peak.at - centre).abs() <= 2.0 * NEW_NOTE * centre
    }

    /// A sighting with something at its harmonics: still this tone, but its level says nothing,
    /// so it neither counts toward the attack nor can show a runaway.
    pub fn keep(&mut self, peak: &Peak) {
        self.bin = peak.bin;
        self.missed = 0;
    }

    /// The bin it was last seen at.
    pub fn bin(&self) -> usize {
        self.bin
    }

    /// Whether `peak` is this tone again, give or take a semitone and never less than a bin.
    pub fn continues(&self, peak: &Peak) -> bool {
        let reach = ((self.bin as f32 * SEMITONE).ceil() as usize).max(1);
        self.bin.abs_diff(peak.bin) <= reach
    }

    /// Whether `peak` is a new note rather than this tone: it comes in at the tone's level, not
    /// faded 6 dB under it, as a note's tail is wherever its pitch wanders, and either leaves a
    /// pitch held for `SETTLED` analyses or steps at once from a run's note held flat.
    pub fn is_new_note(&self, peak: &Peak) -> bool {
        peak.db >= self.level() - RUNAWAY_DB
            && (self.leaves_a_held_pitch(peak) || self.steps_from_a_flat_note(peak))
    }

    /// A move of more than a quarter semitone past the swing of a pitch held for `SETTLED`
    /// analyses, while not climbing as a loop does.
    fn leaves_a_held_pitch(&self, peak: &Peak) -> bool {
        let centre = self.centre();
        let moved = (peak.at - centre).abs() > NEW_NOTE * centre + self.swing(centre);
        moved && self.held >= SETTLED && !self.climbing()
    }

    /// A step of more than a quarter semitone within one analysis, from a note held for
    /// `NOTE_SIGHTINGS` that neither climbed nor fell as fast as a loop climbs. A run's notes are
    /// too short for a held pitch, and its crescendo grows only at the steps; a loop grows while
    /// it holds a pitch, and a gliding voice never steps that far at once.
    fn steps_from_a_flat_note(&self, peak: &Peak) -> bool {
        let last = self.seen % PITCH_ANALYSES;
        let stepped = (peak.at - self.recent_at[last]).abs() > NEW_NOTE * self.recent_at[last]
            && (peak.at - self.note_at).abs() > NEW_NOTE * self.note_at;
        let grown = self.seen_db[last] - self.note_db;
        let flat = grown.abs() < CLIMBING_DB_PER_ANALYSIS * (self.note_sightings - 1) as f32;
        stepped && self.note_sightings >= NOTE_SIGHTINGS && flat
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

    /// The levels it was seen at in its newest `count` sightings, or as many as it has had.
    fn newest(&self, count: usize) -> impl Iterator<Item = f32> + '_ {
        (0..count.min(self.seen + 1))
            .map(|k| self.seen_db[(self.seen + PITCH_ANALYSES - k) % PITCH_ANALYSES])
    }

    /// The middle of the levels it was seen at in its newest `count` sightings, up to five, the
    /// quieter of the two middles when they are even.
    fn middle(&self, count: usize) -> f32 {
        let mut levels = [0.0f32; 5];
        let count = count.min(5).min(self.seen + 1);
        for (slot, level) in levels.iter_mut().zip(self.newest(count)) {
            *slot = level;
        }
        levels[..count].sort_unstable_by(f32::total_cmp);
        levels[(count - 1) / 2]
    }

    /// How loud the tone is now: the middle of its newest three sightings, which one window
    /// straddling a note's edge does not move.
    pub fn level(&self) -> f32 {
        self.middle(3)
    }

    /// An analysis that did not see this tone; false once it has gone unseen too long.
    pub fn miss(&mut self) -> bool {
        self.missed += 1;
        self.analyses += 1;
        self.missed <= MISSES
    }

    /// Takes the next analysis of this tone; true when it is running away.
    pub fn extend(&mut self, peak: Peak) -> bool {
        if (peak.at - self.note_at).abs() > NEW_NOTE * self.note_at {
            (self.note_at, self.note_sightings, self.note_db) = (peak.at, 1, peak.db);
        } else {
            let sightings = self.note_sightings as f32;
            self.note_at = (self.note_at * sightings + peak.at) / (sightings + 1.0);
            self.note_sightings += 1;
        }
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
        let Some(settled) = self.settled_db else {
            // The attack is over after `SETTLED` analyses with three sightings in them. It is
            // judged from the middle of its last five sightings, so neither a note's edge nor a
            // window straddling a breath sets the baseline at the dip.
            if self.analyses >= SETTLED && self.seen >= 2 {
                self.settled_db = Some(self.middle(5));
            }
            return false;
        };
        // A tone that fell and settled again, its last `SETTLED` levels within `STEADY_DB`, is
        // judged from the loudest of them
        if self.seen + 1 >= SETTLED as usize {
            let (low, high) = self
                .newest(SETTLED as usize)
                .fold((f32::MAX, f32::MIN), |(low, high), db| (low.min(db), high.max(db)));
            if high - low <= STEADY_DB && high < settled {
                self.settled_db = Some(high);
            }
        }
        peak.db - self.settled_db.unwrap_or(settled) >= RUNAWAY_DB
    }
}
