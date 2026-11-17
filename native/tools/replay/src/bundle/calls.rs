//! A host's calls as `src/mixer.ts` turns them into what the engine takes, so a replay sets the
//! slot the live song set. A change to how a call is resolved there changes this in the same PR
//! (docs/design.md 8.11).

use engine::params::{
    CleanupParams, CorrectorParams, EchoCancelParams, EffectParams, HowlGuardParams, ReverbParams,
    SlotParams, VoiceParams,
};
use engine::slot::guide::Note;

use super::line::{Effect, Guide, SlotSettings};
use super::profile::{EffectsProfile, SlotProfile};

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
/// an effect leaves out is the profile's.
pub fn set_slot_effects(params: &mut SlotParams, effects: &[Effect], profile: &EffectsProfile) {
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
        }
    }
    params.effects = chain;
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
