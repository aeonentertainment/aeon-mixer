use std::f64::consts::TAU;

use crate::block::RATE;
use crate::slot::effects::level::amplitude;
use crate::slot::effects::tone::band::Band;
use crate::slot::effects::tone::band_ratio::BandRatio;
use crate::slot::effects::tone::biquad::Biquad;
use crate::slot::effects::tone::cut::Cut;

/// The shelf's pole. At 12 dB it takes 10 dB off 50 Hz and 8 off 80 Hz, and leaves 233 Hz 0.17 ms
/// late while it does. Decision 0029 has why it is first order and not the usual 150 Hz second.
const CORNER_HZ: f64 = 250.0;
/// The detector weighs the band under 60 Hz, two Butterworth sections at 24 dB an octave, against
/// the voice over 300 Hz, one section. A pop is mostly under 60 Hz; an E2's fundamental at 82 is
/// already 13 dB down in that band.
const UNDER_HZ: f64 = 60.0;
const VOICE_HZ: f64 = 300.0;
/// The guard ducks by the whole `plosiveDb` while the band under 60 Hz stands this far over the
/// voice. Sung E2s and D2s, bright, dark and with a 5 ms onset, peak 14.5 dB over, the first
/// period of a note included, before its harmonics over 300 Hz arrive as their first pulse: 6.5 dB
/// of room. Nearer the notes it would catch more of a pop, but a low note that ducks is worse than
/// a pop that gets through.
const OVER_DB: f32 = 21.0;
/// A pop is one run of the detector firing, with gaps shorter than the cut's own release: it counts
/// as the detector fires after this long quiet, whatever depth `plosiveDb` lets the cut reach.
const POP_GAP_FRAMES: u32 = RATE / 20;

/// The plosive guard: a first-order low shelf that ducks by the slot's `plosiveDb` while a burst
/// stands under the voice, and lets go over 50 ms after it. With no lookahead, and a detector
/// whose band under 60 Hz takes about 7 ms to rise, a pop's first milliseconds pass, which the
/// master's soft clipper stands behind.
pub struct PlosiveGuard {
    /// The shelf as `x + (g - 1) low(x)`, where `low` is a fixed one-pole at the corner, so a cut
    /// that moves scales a filter that does not.
    low: [f64; 2],
    low_state: (f64, f64),
    detector: BandRatio,
    cut: Cut,
    /// Frames since the detector last fired, up to `POP_GAP_FRAMES`.
    quiet: u32,
    listening: bool,
    catches: u32,
}

impl PlosiveGuard {
    pub fn new() -> Self {
        // The bilinear transform of w / (s + w), unwarped as decision 0029 computed it
        let w = TAU * CORNER_HZ;
        let k = 2.0 * f64::from(RATE);
        Self {
            low: [w / (k + w), (w - k) / (k + w)],
            low_state: (0.0, 0.0),
            detector: BandRatio::new(
                Band::new(Biquad::low_pass(UNDER_HZ), true),
                Band::new(Biquad::high_pass(VOICE_HZ), false),
            ),
            cut: Cut::new(),
            quiet: POP_GAP_FRAMES,
            listening: false,
            catches: 0,
        }
    }

    /// Pops ducked since the slot was built.
    pub fn catches(&self) -> u32 {
        self.catches
    }

    /// One run in place, cutting at most `most_db`. At 0 it lets go of any cut across the run and
    /// then does no work.
    pub fn process(&mut self, most_db: f32, run: &mut [f32]) {
        if most_db <= 0.0 {
            if self.listening {
                self.let_go(run);
            }
            return;
        }
        self.listening = true;
        let [b, a] = self.low;
        for sample in run {
            let x = f64::from(*sample);
            let (x1, y1) = self.low_state;
            let low = b * (x + x1) - a * y1;
            self.low_state = (x, low);

            let over = self.detector.stands_db(x);
            let caught = over > OVER_DB;
            let cut_db = self.cut.follow(if caught { most_db } else { 0.0 });
            if caught {
                self.catches += u32::from(self.quiet == POP_GAP_FRAMES);
                self.quiet = 0;
            } else {
                self.quiet = (self.quiet + 1).min(POP_GAP_FRAMES);
            }
            if !self.cut.at_rest() {
                let gain = f64::from(amplitude(-cut_db));
                *sample = (x + (gain - 1.0) * low) as f32;
            }
        }
    }

    /// Eases the cut to nothing across `run`, then forgets what it heard.
    fn let_go(&mut self, run: &mut [f32]) {
        if !self.cut.at_rest() {
            let [b, a] = self.low;
            let from = f64::from(amplitude(-self.cut.db())) - 1.0;
            let frames = run.len() as f64;
            for (i, sample) in run.iter_mut().enumerate() {
                let x = f64::from(*sample);
                let (x1, y1) = self.low_state;
                let low = b * (x + x1) - a * y1;
                self.low_state = (x, low);
                *sample = (x + from * (1.0 - (i + 1) as f64 / frames) * low) as f32;
            }
        }
        *self = Self { catches: self.catches, ..Self::new() };
    }
}

impl Default for PlosiveGuard {
    fn default() -> Self {
        Self::new()
    }
}
