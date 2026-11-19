use crate::block::RATE;
use crate::slot::effects::tone::band::Band;
use crate::slot::effects::tone::band_ratio::BandRatio;
use crate::slot::effects::tone::biquad::Biquad;
use crate::slot::effects::tone::cut::Cut;
use crate::slot::effects::tone::section::Section;
use crate::slot::effects::tone::shelf::Shelf;

/// Where the shelf turns over and where the detector splits the voice. At its deepest, 12 dB, the
/// shelf leaves 2.3 kHz 0.04 ms late while it cuts.
const SPLIT_HZ: f64 = 5_000.0;

/// The de-esser: a high shelf from 5 kHz that cuts, dB for dB, by how far the voice over 5 kHz
/// stands above the rest of it, to at most the slot's `deEssDb`. A level against the voice's own
/// needs no threshold per phone and treats a quiet singer as it does a loud one. With no
/// lookahead, the first millisecond of an s passes.
pub struct DeEsser {
    shelf: Shelf,
    section: Section,
    detector: BandRatio,
    cut: Cut,
    /// The deepest cut this second and the whole of the last, for `stats()`.
    most_db: [f32; 2],
    frames: u32,
    listening: bool,
}

impl DeEsser {
    pub fn new() -> Self {
        Self {
            shelf: Shelf::high(SPLIT_HZ),
            section: Section::new(),
            detector: BandRatio::new(
                Band::new(Biquad::high_pass(SPLIT_HZ), false),
                Band::new(Biquad::low_pass(SPLIT_HZ), false),
            ),
            cut: Cut::new(),
            most_db: [0.0; 2],
            frames: 0,
            listening: false,
        }
    }

    /// The most it took off in the last second.
    pub fn most_db(&self) -> f32 {
        self.most_db[0].max(self.most_db[1])
    }

    /// One run in place, cutting at most `most_db`. At 0 it lets go of any cut across the run and
    /// then does no work.
    pub fn process(&mut self, most_db: f32, run: &mut [f32]) {
        if most_db <= 0.0 {
            if self.listening {
                self.section.process(Biquad::IDENTITY, run);
                *self = Self::new();
            }
            return;
        }
        self.listening = true;
        for sample in run {
            let x = f64::from(*sample);
            let cut_db = self.cut.follow(self.detector.stands_db(x).clamp(0.0, most_db));
            if !self.cut.at_rest() {
                let at = self.shelf.at(-f64::from(cut_db));
                *sample = self.section.tick(&at, x) as f32;
                self.most_db[0] = self.most_db[0].max(cut_db);
            } else {
                self.section.pass(x);
            }
            self.frames += 1;
            if self.frames == RATE {
                self.most_db = [0.0, self.most_db[0]];
                self.frames = 0;
            }
        }
    }
}

impl Default for DeEsser {
    fn default() -> Self {
        Self::new()
    }
}
