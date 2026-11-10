//! A corrector variant, one thing a run compares. A later PR adds a line to `Variant::standard`,
//! or passes `--variant` for a one-off, and every row picks it up.

use engine::params::CorrectorParams;
use engine::slot::guide::Guide;
use kernels::EVERY_PITCH_CLASS;

use crate::music::key_mask;
use crate::sung_clip::SungClip;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Off,
    Key,
    Melody,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Variant {
    pub label: String,
    pub mode: Mode,
    pub strength: f32,
    pub retune_ms: f32,
}

impl Variant {
    fn new(label: &str, mode: Mode, strength: f32, retune_ms: f32) -> Self {
        Self { label: label.to_owned(), mode, strength, retune_ms }
    }

    /// The baseline's three, and key mode at the settings the blind listen hears.
    pub fn standard() -> Vec<Self> {
        vec![
            Self::off(),
            Self::new("melody-0.6", Mode::Melody, 0.6, 40.0),
            Self::new("melody-1.0", Mode::Melody, 1.0, 40.0),
            Self::new("key-0.6", Mode::Key, 0.6, 40.0),
        ]
    }

    pub fn off() -> Self {
        Self::new("off", Mode::Off, 0.0, 0.0)
    }

    /// `label:mode:strength:retune_ms`, as `--variant fast:melody:1:10`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = || format!("{text} is not label:off|key|melody:strength:retune_ms");
        let fields: Vec<&str> = text.split(':').collect();
        let [label, mode, strength, retune_ms] = fields[..] else { return Err(bad()) };
        let mode = match mode {
            "off" => Mode::Off,
            "key" => Mode::Key,
            "melody" => Mode::Melody,
            _ => return Err(bad()),
        };
        Ok(Self::new(
            label,
            mode,
            strength.parse().map_err(|_| bad())?,
            retune_ms.parse().map_err(|_| bad())?,
        ))
    }

    pub fn params(&self) -> CorrectorParams {
        CorrectorParams {
            enabled: self.mode != Mode::Off,
            strength: self.strength,
            retune_ms: self.retune_ms,
        }
    }

    /// The guide this variant follows on `clip`. Melody mode pulls to every semitone between
    /// notes, as the harness's override with no key did, so the row compares like with like. The
    /// blind listen's melody take pulls to the clip's key between notes instead.
    pub fn guide(&self, clip: &SungClip) -> Result<Guide, String> {
        Ok(match self.mode {
            Mode::Melody | Mode::Off => clip.guide(EVERY_PITCH_CLASS),
            Mode::Key => Guide { notes: Vec::new(), key_mask: key_mask(&clip.key)? },
        })
    }
}
