use std::fmt;

use crate::block::RATE;
use crate::params::{TONE_BANDS, ToneParams, ToneRefusal};
use crate::slot::effects::tone::biquad::Biquad;
use crate::slot::effects::tone::shelf::Shelf;

/// The bands, then the air shelf.
pub const SECTIONS: usize = TONE_BANDS + 1;
/// Where the air shelf lifts.
const AIR_HZ: f64 = 10_000.0;
/// The phase decision 0014's voice stage already costs, the most this repository has shipped as
/// no added delay.
pub const MOST_LATE_MS: f64 = 0.2;
/// Cleanup's makeup cap, decision 0012: together the two bring one frequency 12 dB toward a howl.
pub const MOST_LIFT_DB: f64 = 6.0;
/// The span judged: cleanup's high-pass corner, just under the lowest voice the corrector reads,
/// to the top of what a phone captures.
const LOWEST_HZ: f64 = 80.0;
const HIGHEST_HZ: f64 = 16_000.0;
/// Log-spaced points across the span, 0.26 % apart, a twentieth of the narrowest band's width.
/// Each band's own frequency is judged too.
const POINTS: usize = 2048;

impl ToneParams {
    /// The filters the bands and the air run as, in order, the identity where there is none. The
    /// stage runs these and `check` judges these, so the two cannot drift apart.
    pub fn cascade(&self) -> [Biquad; SECTIONS] {
        let mut cascade = [Biquad::IDENTITY; SECTIONS];
        for (section, band) in cascade.iter_mut().zip(self.bands.as_slice()) {
            *section = Biquad::band(band);
        }
        if self.air_db != 0.0 {
            cascade[TONE_BANDS] = Shelf::high(AIR_HZ).at(f64::from(self.air_db));
        }
        cascade
    }

    /// Refuses bands and air that together leave a frequency from 80 Hz to 16 kHz more than
    /// 0.2 ms late or lift one more than 6 dB, naming the worst. Runs on the control thread before
    /// the params cross, on or off, since a host may turn the stage on later.
    pub fn check(&self) -> Result<(), ToneRefusal> {
        let cascade = self.cascade();
        let sections = || cascade.iter().filter(|section| **section != Biquad::IDENTITY);
        let span = HIGHEST_HZ / LOWEST_HZ;
        let grid = (0..POINTS).map(|i| LOWEST_HZ * span.powf(i as f64 / (POINTS - 1) as f64));
        let centres = self.bands.as_slice().iter().map(|band| f64::from(band.hz()));
        let mut late = (LOWEST_HZ, f64::MIN);
        let mut lifted = (LOWEST_HZ, f64::MIN);
        for hz in grid.chain(centres.filter(|hz| (LOWEST_HZ..=HIGHEST_HZ).contains(hz))) {
            let (gain_db, delay) = sections().fold((0.0, 0.0), |(gain, delay), section| {
                let (g, d) = section.response(hz);
                (gain + g, delay + d)
            });
            let delay_ms = delay * 1000.0 / f64::from(RATE);
            if delay_ms > late.1 {
                late = (hz, delay_ms);
            }
            if gain_db > lifted.1 {
                lifted = (hz, gain_db);
            }
        }
        if late.1 > MOST_LATE_MS {
            return Err(ToneRefusal::Late { hz: late.0 as f32, delay_ms: late.1 as f32 });
        }
        if lifted.1 > MOST_LIFT_DB {
            return Err(ToneRefusal::Lifted { hz: lifted.0 as f32, lift_db: lifted.1 as f32 });
        }
        Ok(())
    }
}

/// What `setSlotEffects` throws: the frequency, the figure and the cap it breaks.
impl fmt::Display for ToneRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Late { hz, delay_ms } => write!(
                f,
                "the tone stage's bands and air leave {hz:.0} Hz {delay_ms:.3} ms late, over the \
                 {MOST_LATE_MS} ms cap"
            ),
            Self::Lifted { hz, lift_db } => write!(
                f,
                "the tone stage's bands and air lift {hz:.0} Hz by {lift_db:.2} dB, over the \
                 {MOST_LIFT_DB} dB cap"
            ),
        }
    }
}
