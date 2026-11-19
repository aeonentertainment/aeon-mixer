//! What one take of a pair sounds like, in the numbers decision 0029 judges the tone stage by:
//! its long-term spectrum while singing, its sibilance, its pops and its room
//! (docs/design.md 8.9).

use engine::block::RATE;
use engine::slot::effects::tone::{Biquad, Section};

use super::classes::Classes;
use super::frame::{BANDS, Frame, db};

/// Pops are what stands over the singing under this.
const POP_HZ: f64 = 100.0;
/// The pops' reading: 10 ms.
const POP_WINDOW: usize = 480;

pub struct Heard {
    /// The voiced frames' mean power in each third octave, in dB.
    pub spectrum_db: [f64; BANDS],
    /// Power over 5 kHz in the unvoiced frames against the voiced level; none with no unvoiced
    /// frame.
    pub sibilance_db: Option<f64>,
    pub plosives_per_minute: f64,
    /// The frames between lines against the voiced level.
    pub noise_floor_db: f64,
}

impl Heard {
    /// `samples`, cut into `frames`, read on the reference's `classes`.
    pub fn of(samples: &[f32], frames: &[Frame], classes: &Classes) -> Self {
        let mean = |class: &[bool], power: &dyn Fn(&Frame) -> f64| {
            let picked: Vec<f64> = frames
                .iter()
                .zip(class)
                .filter(|&(_, &is)| is)
                .map(|(frame, _)| power(frame))
                .collect();
            (!picked.is_empty()).then(|| picked.iter().sum::<f64>() / picked.len() as f64)
        };
        // Classes always hold a voiced frame and a frame between lines
        let voiced = mean(&classes.voiced, &|frame| frame.power).unwrap_or_default();
        let between_lines = mean(&classes.between_lines, &|frame| frame.power).unwrap_or_default();
        Self {
            spectrum_db: std::array::from_fn(|band| {
                db(mean(&classes.voiced, &|frame| frame.bands[band]).unwrap_or_default())
            }),
            sibilance_db: mean(&classes.unvoiced, &|frame| frame.over)
                .map(|over| db(over) - db(voiced)),
            plosives_per_minute: plosives_per_minute(samples, voiced),
            noise_floor_db: db(between_lines) - db(voiced),
        }
    }
}

/// Each run of 10 ms readings under 100 Hz, at 24 dB an octave, that stands over `voiced`, a
/// minute.
fn plosives_per_minute(samples: &[f32], voiced: f64) -> f64 {
    let low = Biquad::low_pass(POP_HZ);
    let mut sections = [Section::new(), Section::new()];
    let (mut bursts, mut over) = (0u32, false);
    for window in samples.as_chunks::<POP_WINDOW>().0 {
        let power = window
            .iter()
            .map(|&x| {
                sections.iter_mut().fold(f64::from(x), |y, section| section.tick(&low, y)).powi(2)
            })
            .sum::<f64>()
            / POP_WINDOW as f64;
        if power > voiced && !over {
            bursts += 1;
        }
        over = power > voiced;
    }
    let minutes = samples.len() as f64 / f64::from(RATE) / 60.0;
    f64::from(bursts) / minutes
}
