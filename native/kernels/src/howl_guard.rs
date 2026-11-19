mod analyser;
#[cfg(test)]
mod tests;
mod track;

use analyser::{Analyser, LENGTH, Peak};
use track::Track;

/// How far a catch ducks the slot. A loop that runs away sits a few dB over unity gain; this
/// takes it well under, and a second catch ducks further.
const DUCK_DB: f32 = -12.0;
/// After a catch the duck holds this long, then lifts at `RELEASE_DB_PER_SECOND`, but only to
/// `CEILING_DB` under the gain that howled: lifting past it would only howl again.
const HOLD_SECONDS: f32 = 0.5;
const RELEASE_DB_PER_SECOND: f32 = 3.0;
const CEILING_DB: f32 = -6.0;
/// The ceiling itself lifts this fast, so a singer who moved their phone away gets their level
/// back within a minute or two.
const CEILING_RELEASE_DB_PER_SECOND: f32 = 0.1;
/// The narrow peaks each analysis hands the tones it follows, loudest first, and how many tones
/// it follows at once. With one peak a singer's louder partial hides a howl's sightings; with one
/// tone, a singer's fundamental that stands alone holds it while a howl grows beneath. Four and
/// two are the fewest that hold the guard's numbers; eight and four hold the same.
const PEAKS: usize = 4;
const TRACKS: usize = 2;
/// Frames between analyses: 10.7 ms.
const HOP: usize = 512;

/// Ducks a slot whose loop through the speakers runs away. It detects a howl as notch-based
/// suppressors do, a narrow tone with no harmonic and no fundamental that persists and keeps
/// growing, following the two loudest such tones at once so a singer's lone fundamental does not
/// hide a howl under it, and answers as a gain-reduction method does, by turning the whole slot down, which stabilises
/// any loop and adds no delay: the analysis looks at the signal on the side and the gain moves
/// within the block it is decided in. Off, or never caught, it hands the voice over bit for bit.
/// Built on the control thread; `process` allocates nothing.
pub struct HowlGuard {
    analyser: Analyser,
    recent: Vec<f32>,
    written: usize,
    since_analysis: usize,
    tracks: [Option<Track>; TRACKS],
    peaks: [Peak; PEAKS],
    gain: f32,
    target: f32,
    /// The most the release lifts the gain to, under the gain the last howl was caught at.
    ceiling: f32,
    hold_frames: usize,
    release_per_frame: f32,
    ceiling_release_per_frame: f32,
    hold_after_catch: usize,
    catches: u32,
}

impl HowlGuard {
    pub fn new(rate: u32) -> Self {
        Self {
            analyser: Analyser::new(rate),
            recent: vec![0.0; LENGTH],
            written: 0,
            since_analysis: 0,
            tracks: [None; TRACKS],
            peaks: [Peak { bin: 0, at: 0.0, db: f32::MIN, alone: false }; PEAKS],
            gain: 1.0,
            target: 1.0,
            ceiling: 1.0,
            hold_frames: 0,
            release_per_frame: 10f32.powf(RELEASE_DB_PER_SECOND / 20.0 / rate as f32),
            ceiling_release_per_frame: 10f32
                .powf(CEILING_RELEASE_DB_PER_SECOND / 20.0 / rate as f32),
            hold_after_catch: (HOLD_SECONDS * rate as f32) as usize,
            catches: 0,
        }
    }

    /// Times the guard has ducked since it was built.
    pub fn catches(&self) -> u32 {
        self.catches
    }

    /// Forgets the tone it was following and lifts any duck at once.
    pub fn reset(&mut self) {
        self.recent.fill(0.0);
        self.since_analysis = 0;
        self.tracks = [None; TRACKS];
        self.gain = 1.0;
        self.target = 1.0;
        self.ceiling = 1.0;
        self.hold_frames = 0;
    }

    /// One block of the slot's voice, in place.
    pub fn process(&mut self, block: &mut [f32]) {
        for &sample in block.iter() {
            self.recent[self.written % LENGTH] = sample;
            self.written += 1;
        }
        self.since_analysis += block.len();
        if self.since_analysis >= HOP {
            self.since_analysis -= HOP;
            let start = self.written % LENGTH;
            let recent = self.recent[start..].iter().chain(&self.recent[..start]).copied();
            let found = self.analyser.analyse(recent, &mut self.peaks);
            if self.follow(found) {
                self.catch();
            }
        }
        self.release(block.len());
        self.apply(block);
    }

    /// Follows each tone to the nearest of the `found` peaks that continues it, and starts tones on
    /// the loudest of the rest that stand alone, in place of the quietest tone when it has no room.
    /// True when a tone runs away while standing alone.
    fn follow(&mut self, found: usize) -> bool {
        let peaks = &self.peaks[..found];
        let mut taken = [false; PEAKS];
        let mut caught = false;
        for slot in &mut self.tracks {
            let Some(track) = slot else { continue };
            let next = (0..found)
                .filter(|&i| !taken[i] && track.continues(&peaks[i]))
                .min_by_key(|&i| track.bin().abs_diff(peaks[i].bin));
            match next.map(|i| (i, peaks[i])) {
                // Something at its harmonics: still this tone if it has not moved, but how loud it
                // is says nothing about a loop
                Some((i, peak)) if !peak.alone && track.holds(&peak) => {
                    taken[i] = true;
                    track.keep(&peak);
                }
                Some((i, peak)) if peak.alone => {
                    taken[i] = true;
                    // A new note starts its own track at once, not after the old one has gone unseen
                    if track.is_new_note(&peak) {
                        *slot = Some(Track::new(peak));
                    } else {
                        caught |= track.extend(peak);
                    }
                }
                _ if track.miss() => {}
                _ => *slot = None,
            }
        }
        for (i, &peak) in peaks.iter().enumerate() {
            if taken[i] || !peak.alone {
                continue;
            }
            let level = |slot: &Option<Track>| slot.map_or(f32::MIN, |track| track.level());
            let quietest = self.tracks.iter_mut().min_by(|a, b| level(a).total_cmp(&level(b)));
            if let Some(slot) = quietest.filter(|slot| level(slot) < peak.db) {
                *slot = Some(Track::new(peak));
            }
        }
        caught
    }

    fn catch(&mut self) {
        self.catches += 1;
        let howled_at = self.gain.min(self.target);
        self.ceiling = howled_at * 10f32.powf(CEILING_DB / 20.0);
        self.target = (howled_at * 10f32.powf(DUCK_DB / 20.0)).max(1e-3);
        self.hold_frames = self.hold_after_catch;
        // The next catch needs the loop to run away again from where the duck leaves it
        self.tracks = [None; TRACKS];
    }

    fn release(&mut self, frames: usize) {
        if self.target >= 1.0 {
            return;
        }
        self.ceiling = (self.ceiling * self.ceiling_release_per_frame.powi(frames as i32)).min(1.0);
        if self.hold_frames > 0 {
            self.hold_frames = self.hold_frames.saturating_sub(frames);
            return;
        }
        self.target = (self.target * self.release_per_frame.powi(frames as i32)).min(self.ceiling);
        if self.ceiling >= 1.0 && self.target >= 1.0 {
            // Exactly unity again, so the voice passes bit for bit
            self.target = 1.0;
        }
    }

    fn apply(&mut self, block: &mut [f32]) {
        if self.gain == 1.0 && self.target == 1.0 {
            return;
        }
        let step = (self.target - self.gain) / block.len() as f32;
        for sample in block.iter_mut() {
            self.gain += step;
            *sample *= self.gain;
        }
        self.gain = self.target;
    }
}
