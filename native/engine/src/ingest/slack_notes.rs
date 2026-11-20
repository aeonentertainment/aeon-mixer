//! What a stream's packets show of its delay (docs/design.md 8.4). Every half second it notes how
//! close to their due times the half second's latest and earliest packets came, and keeps the last
//! ten seconds of notes. The earliest packets' slack is the stream's delay, which `stats()`
//! reports; the chooser moves the delay from the same notes, so the number reported is the one
//! the moves bound.

use crate::block::RATE;

/// One note per half second: a stall spans at most a few, and neither a frame's length nor how
/// many packets a stall catches changes what it counts for.
const INTERVAL: i64 = RATE as i64 / 2;

/// Ten seconds of notes. The worst of 20 covers 86 percent of half seconds with 95 percent
/// confidence whatever the distribution, and a later half second beats it about once in ten
/// seconds. Decision 0007 has what the chooser's margin buys on top.
pub const NOTES: usize = 20;

/// The slack of one half second's latest and earliest packets against their due times.
#[derive(Clone, Copy)]
struct Note {
    worst: i64,
    best: i64,
}

impl Note {
    const EMPTY: Self = Self { worst: i64::MAX, best: i64::MIN };

    fn is_empty(&self) -> bool {
        self.best == Self::EMPTY.best
    }
}

pub struct SlackNotes {
    current: Note,
    ends_at: i64,
    notes: [Note; NOTES],
    /// Where the next note goes, and how many have been taken, up to all of them.
    next: usize,
    filled: usize,
}

impl SlackNotes {
    pub fn new(now: i64) -> Self {
        Self {
            current: Note::EMPTY,
            ends_at: now + INTERVAL,
            notes: [Note::EMPTY; NOTES],
            next: 0,
            filled: 0,
        }
    }

    /// A packet's slack against its due time, whether it made it or not.
    pub fn observe(&mut self, slack: i64) {
        self.current.worst = self.current.worst.min(slack);
        self.current.best = self.current.best.max(slack);
    }

    /// Once a block: closes the half second when it has ended, and says whether it did.
    pub fn tick(&mut self, now: i64) -> bool {
        if now < self.ends_at {
            return false;
        }
        self.ends_at = now + INTERVAL;
        // A sender in DTX or a stall that outlasts a half second leaves nothing to note
        if !self.current.is_empty() {
            self.notes[self.next] = self.current;
            self.next = (self.next + 1) % NOTES;
            self.filled = (self.filled + 1).min(NOTES);
            self.current = Note::EMPTY;
        }
        true
    }

    /// The anchor moved: every slack noted moved with it.
    pub fn shift(&mut self, by: i64) {
        for note in self.notes.iter_mut().chain([&mut self.current]) {
            if !note.is_empty() {
                note.worst += by;
                note.best += by;
            }
        }
    }

    /// Whether all ten seconds of notes are in.
    pub fn full(&self) -> bool {
        self.filled == NOTES
    }

    /// The least slack in the notes taken: the latest packet's.
    pub fn worst(&self) -> Option<i64> {
        self.taken().map(|note| note.worst).min()
    }

    /// The most slack in the notes taken: what the earliest packets wait, the stream's delay.
    /// `None` before a packet.
    pub fn best(&self) -> Option<i64> {
        self.taken().map(|note| note.best).max()
    }

    /// Notes taken whose latest packet was late.
    pub fn late(&self) -> usize {
        self.taken().filter(|note| note.worst < 0).count()
    }

    /// The notes taken and the half second being noted. A choice is made only on the block that
    /// closes a half second, when the one being noted is still empty, so the chooser reads ten
    /// seconds of notes while the stat also reads the packets since.
    fn taken(&self) -> impl Iterator<Item = &Note> {
        self.notes.iter().chain([&self.current]).filter(|note| !note.is_empty())
    }
}
