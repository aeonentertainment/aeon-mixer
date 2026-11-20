//! How far a phone's take sounds from the reference mic's: the gap between their long-term
//! spectra with their levels matched (decision 0029, docs/design.md 8.9).

use super::frame::BANDS;
use super::heard::Heard;

/// The four bands the signed gap is read in, by their numbers' names and in words, with the third
/// octaves each holds.
pub const BANDS_READ: [(&str, &str, std::ops::Range<usize>); 4] = [
    ("pairLowDb", "low, 100 to 400 Hz", 0..6),
    ("pairMidDb", "mid, 400 Hz to 2 kHz", 6..13),
    ("pairPresenceDb", "presence, 2 to 6 kHz", 13..18),
    ("pairTopDb", "top, 6 to 12.5 kHz", 18..BANDS),
];

pub struct Gap {
    /// The RMS of the matched gap over the third octaves: 0 is the mic's tone.
    pub distance_db: f64,
    /// The matched gap's mean in each of `BANDS_READ`; positive is louder than the mic there.
    pub bands_db: [f64; 4],
}

impl Gap {
    pub fn between(phone: &Heard, mic: &Heard) -> Self {
        let gaps: [f64; BANDS] = std::array::from_fn(|b| phone.spectrum_db[b] - mic.spectrum_db[b]);
        let level = gaps.iter().sum::<f64>() / BANDS as f64;
        let matched = gaps.map(|gap| gap - level);
        let mean = |gaps: &[f64]| gaps.iter().sum::<f64>() / gaps.len() as f64;
        Self {
            distance_db: mean(&matched.map(|gap| gap * gap)).sqrt(),
            bands_db: BANDS_READ.map(|(_, _, bands)| mean(&matched[bands])),
        }
    }
}
