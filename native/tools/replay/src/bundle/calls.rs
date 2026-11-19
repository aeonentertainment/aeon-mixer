//! A host's calls as `src/mixer.ts` turns them into what the engine takes, so a replay sets the
//! slot the live song set. A change to how a call is resolved there changes this in the same PR
//! (docs/design.md 8.11).

use engine::params::{
    CleanupParams, CorrectorParams, EchoCancelParams, EffectParams, HowlGuardParams, ReverbParams,
    SlotParams, ToneBands, ToneParams, VoiceParams,
};
use engine::slot::guide::Note;

use super::line::{Effect, Guide, SlotSettings, ToneBand};
use super::profile::{EffectsProfile, SlotProfile, within};

/// A slot as a mixer starts it: the profile's gain and pan, every effect off.
pub fn slot_start(profile: &SlotProfile) -> SlotParams {
    SlotParams {
        gain: profile.gain,
        pan: profile.pan,
        effects: EffectParams::default(),
        ..SlotParams::default()
    }
}

/// `setSlot`: what the call names changes, the rest stays.
pub fn set_slot(params: &mut SlotParams, settings: &SlotSettings) {
    params.gain = settings.gain.unwrap_or(params.gain);
    params.pan = settings.pan.unwrap_or(params.pan);
    params.mute = settings.mute.unwrap_or(params.mute);
    params.solo = settings.solo.unwrap_or(params.solo);
}

/// `setSlotEffects`: the slot's whole chain. An effect not in `effects` is off, and a parameter
/// an effect leaves out is the profile's, or for the tone stage, which is not in the profile, 0.
pub fn effects(effects: &[Effect], profile: &EffectsProfile) -> Result<EffectParams, String> {
    let mut chain = EffectParams::default();
    for effect in effects {
        match *effect {
            Effect::PitchCorrection { enabled, strength, retune_ms } => {
                let default = &profile.pitch_correction;
                chain.corrector = CorrectorParams {
                    enabled,
                    strength: strength.unwrap_or(default.strength),
                    retune_ms: retune_ms.unwrap_or(default.retune_ms),
                };
            }
            Effect::Reverb { enabled, room_size, mix } => {
                let default = &profile.reverb;
                chain.reverb = ReverbParams {
                    enabled,
                    room_size: room_size.unwrap_or(default.room_size),
                    mix: mix.unwrap_or(default.mix),
                };
            }
            Effect::Voice { enabled, formant_semitones } => {
                chain.voice = VoiceParams {
                    enabled,
                    formant_semitones: formant_semitones.unwrap_or(profile.voice.formant_semitones),
                };
            }
            Effect::EchoCancel { enabled } => chain.echo_cancel = EchoCancelParams { enabled },
            Effect::Cleanup { enabled } => chain.cleanup = CleanupParams { enabled },
            Effect::HowlGuard { enabled } => chain.howl_guard = HowlGuardParams { enabled },
            Effect::Tone { enabled, ref bands, plosive_db, air_db, de_ess_db } => {
                // `src/tone.ts` holds each value to its range before the engine's caps judge the
                // whole, in this order and these words
                for (i, band) in bands.iter().flatten().enumerate() {
                    let (hz, gain_db, q) = match *band {
                        ToneBand::Peak { hz, gain_db, q } => (hz, gain_db, Some(q)),
                        ToneBand::LowShelf { hz, gain_db }
                        | ToneBand::HighShelf { hz, gain_db } => (hz, gain_db, None),
                    };
                    within(&format!("bands[{i}].hz"), hz, 80.0, 16_000.0)?;
                    within(&format!("bands[{i}].gainDb"), gain_db, -12.0, 6.0)?;
                    if let Some(q) = q {
                        within(&format!("bands[{i}].q"), q, 0.3, 8.0)?;
                    }
                }
                within("plosiveDb", plosive_db.unwrap_or(0.0), 0.0, 12.0)?;
                within("airDb", air_db.unwrap_or(0.0), 0.0, 6.0)?;
                within("deEssDb", de_ess_db.unwrap_or(0.0), 0.0, 12.0)?;
                let bands: Vec<_> = bands.iter().flatten().map(tone_band).collect();
                chain.tone = ToneParams {
                    enabled,
                    bands: ToneBands::given(&bands)
                        .ok_or_else(|| format!("a tone effect holds {} bands", bands.len()))?,
                    plosive_db: plosive_db.unwrap_or(0.0),
                    air_db: air_db.unwrap_or(0.0),
                    de_ess_db: de_ess_db.unwrap_or(0.0),
                };
                // The live engine refuses these in `setSlotEffects`, so a bundle the mixer wrote
                // never holds them; one written by hand may, and would score a stage no host plays
                chain.tone.check().map_err(|refusal| refusal.to_string())?;
            }
        }
    }
    Ok(chain)
}

fn tone_band(band: &ToneBand) -> engine::params::ToneBand {
    match *band {
        ToneBand::Peak { hz, gain_db, q } => engine::params::ToneBand::Peak { hz, gain_db, q },
        ToneBand::LowShelf { hz, gain_db } => engine::params::ToneBand::LowShelf { hz, gain_db },
        ToneBand::HighShelf { hz, gain_db } => engine::params::ToneBand::HighShelf { hz, gain_db },
    }
}

/// A melody guide in frames, as `toGuideInput` hands it to the engine.
pub fn guide(guide: &Guide) -> engine::slot::guide::Guide {
    engine::slot::guide::Guide {
        notes: guide
            .notes
            .iter()
            .map(|note| Note {
                midi: note.midi,
                start_frame: ms_to_frames(note.start_ms),
                end_frame: ms_to_frames(note.end_ms),
            })
            .collect(),
        key_mask: guide.key_mask,
    }
}

/// `msToFrames`: 48 frames a millisecond, rounded.
pub fn ms_to_frames(ms: f64) -> u64 {
    (ms * 48.0).round() as u64
}
