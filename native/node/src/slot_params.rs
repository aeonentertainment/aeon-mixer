//! A slot's whole `SlotParams`, as `src/` hands it over after validating it. One call writes one
//! slot of the snapshot; the snapshot is published whole.

use engine::params::{
    CleanupParams, CorrectorParams, EffectParams, ReverbParams, SlotParams, VoiceParams,
};
use napi_derive::napi;

#[napi(object)]
pub struct SlotParamsInput {
    /// Linear amplitude.
    pub gain: f64,
    /// -1 left to 1 right.
    pub pan: f64,
    pub mute: bool,
    pub solo: bool,
    pub corrector: CorrectorInput,
    pub reverb: ReverbInput,
    pub voice: VoiceInput,
    pub cleanup: CleanupInput,
    pub guide_offset_ms: f64,
}

#[napi(object)]
pub struct CorrectorInput {
    pub enabled: bool,
    pub strength: f64,
    pub retune_ms: f64,
}

#[napi(object)]
pub struct CleanupInput {
    pub enabled: bool,
}

#[napi(object)]
pub struct ReverbInput {
    pub enabled: bool,
    pub room_size: f64,
    pub mix: f64,
}

#[napi(object)]
pub struct VoiceInput {
    pub enabled: bool,
    pub formant_semitones: f64,
}

impl From<SlotParamsInput> for SlotParams {
    fn from(input: SlotParamsInput) -> Self {
        Self {
            gain: input.gain as f32,
            pan: input.pan as f32,
            mute: input.mute,
            solo: input.solo,
            effects: EffectParams {
                corrector: CorrectorParams {
                    enabled: input.corrector.enabled,
                    strength: input.corrector.strength as f32,
                    retune_ms: input.corrector.retune_ms as f32,
                },
                reverb: ReverbParams {
                    enabled: input.reverb.enabled,
                    room_size: input.reverb.room_size as f32,
                    mix: input.reverb.mix as f32,
                },
                voice: VoiceParams {
                    enabled: input.voice.enabled,
                    formant_semitones: input.voice.formant_semitones as f32,
                },
                cleanup: CleanupParams { enabled: input.cleanup.enabled },
            },
            guide_offset_ms: input.guide_offset_ms as f32,
        }
    }
}
