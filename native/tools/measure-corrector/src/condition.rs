//! What happened to the voice before the slot, the other thing a run compares. A later PR adds a
//! line to `Condition::standard` and every row picks it up.

use crate::chain::Input;
use crate::music::Lcg;
use crate::rtp::{self, Impairment};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Condition {
    Direct,
    /// Room-like noise, falling with frequency, at this many dB under the singing.
    Noise {
        snr_db: f32,
    },
    /// A phone's Opus and the real jitter buffer. This is vocadito through the codec path unless
    /// the clip really is a phone's.
    Opus {
        impairment: Impairment,
        dtx: bool,
    },
}

/// A clip made ready for the chain.
pub struct Prepared {
    pub input: Input,
    /// How late the voice comes out against the clip's own time.
    pub lag_frames: i64,
    /// What lines the guide clock up with the voice, as a host's `guideOffsetMs` would.
    pub guide_offset_ms: f32,
}

impl Condition {
    pub fn standard() -> Vec<Self> {
        vec![
            Self::Direct,
            Self::Noise { snr_db: 20.0 },
            Self::Opus { impairment: Impairment::NONE, dtx: true },
            Self::Opus { impairment: Impairment::NONE, dtx: false },
            Self::Opus { impairment: Impairment::WIFI, dtx: true },
        ]
    }

    pub fn label(&self) -> String {
        match self {
            Self::Direct => "direct".to_owned(),
            Self::Noise { snr_db } => format!("noise-{snr_db:.0}db"),
            Self::Opus { impairment, dtx } => {
                let network = if *impairment == Impairment::NONE { "lan" } else { "wifi" };
                let suffix = if *dtx { "" } else { "-no-dtx" };
                format!("opus-{network}{suffix}")
            }
        }
    }

    pub fn prepare(&self, samples: &[f32]) -> Prepared {
        match self {
            Self::Direct => Prepared {
                input: Input::Samples(samples.to_vec()),
                lag_frames: 0,
                guide_offset_ms: 0.0,
            },
            Self::Noise { snr_db } => Prepared {
                input: Input::Samples(with_noise(samples, *snr_db)),
                lag_frames: 0,
                guide_offset_ms: 0.0,
            },
            Self::Opus { impairment, dtx } => {
                let encoded = rtp::encode(samples, *impairment, *dtx, 7);
                let error = encoded.claimed_frames - encoded.lag_frames;
                Prepared {
                    input: Input::Packets(encoded.arrivals),
                    lag_frames: encoded.lag_frames,
                    guide_offset_ms: error as f32 * 1000.0 / engine::block::RATE as f32,
                }
            }
        }
    }
}

/// `samples` plus one-pole low-passed noise, scaled against the singing's level: the RMS of the
/// 10 ms frames within 40 dB of the loudest. Synthetic and steady, so it says how a detector
/// handles noise, not how a given room sounds.
pub fn with_noise(samples: &[f32], snr_db: f32) -> Vec<f32> {
    let frame = 480;
    let energies: Vec<f32> = samples
        .chunks(frame)
        .map(|chunk| chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32)
        .collect();
    let loudest = energies.iter().copied().fold(0.0f32, f32::max);
    let active: Vec<f32> = energies.into_iter().filter(|e| *e > loudest * 1e-4).collect();
    let level = (active.iter().sum::<f32>() / active.len().max(1) as f32).sqrt();

    let mut rng = Lcg(0x5EED);
    let mut low = 0.0f32;
    let mut noise: Vec<f32> = samples
        .iter()
        .map(|_| {
            low += 0.2 * (rng.signed() - low);
            low
        })
        .collect();
    let noise_rms = (noise.iter().map(|s| s * s).sum::<f32>() / noise.len().max(1) as f32).sqrt();
    let gain = level * 10f32.powf(-snr_db / 20.0) / noise_rms.max(1e-9);
    noise.iter_mut().zip(samples).for_each(|(n, s)| *n = *s + *n * gain);
    noise
}
