use engine::backing::Planar;
use engine::params::{CorrectorParams, ReverbParams};
use engine::slot::guide::Guide;

use crate::rtp::Arrival;

pub enum Input {
    Samples(Vec<f32>),
    /// Opus or L16 packets and the output frame each reaches the socket at, through the real
    /// packet ring and jitter buffer.
    Packets(Vec<Arrival>),
}

/// What runs through the chain.
pub struct Setup {
    pub input: Input,
    pub frames: usize,
    pub corrector: CorrectorParams,
    pub reverb: ReverbParams,
    pub guide: Option<Guide>,
    pub guide_offset_ms: f32,
    /// Slots beside the first, all the same voice: the CPU row's four singers.
    pub extra_voices: Vec<Vec<f32>>,
    /// Centre-panned at `mix_gain` over this backing, for a listening clip. `None` is the
    /// measuring setup: hard left over silence, so the left channel is the voice.
    pub mix: Option<Mix>,
}

pub struct Mix {
    pub backing: Option<Planar>,
    pub voice_gain: f32,
}

impl Setup {
    pub fn new(input: Input, frames: usize, corrector: CorrectorParams) -> Self {
        Self {
            input,
            frames,
            corrector,
            reverb: ReverbParams::default(),
            guide: None,
            guide_offset_ms: 0.0,
            extra_voices: Vec::new(),
            mix: None,
        }
    }
}
