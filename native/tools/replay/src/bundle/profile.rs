//! The settings profile a bundle replays under (docs/design.md 8.12): its own `profile.json`, or
//! the one `--profile` names. Read as strictly as `src/profile/parse-profile.ts` reads one: an
//! unknown version, a missing field, an unknown field or a value out of its setter's range is
//! refused by name, so a misspelt setting never passes as one that did nothing.

use serde::Deserialize;
use serde_json::Value;

pub const VERSION: u64 = 1;

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    /// Checked before the typed read, and named here so the strict read lets it through.
    #[expect(dead_code, reason = "parse reads it from the JSON first")]
    version: u64,
    pub output: OutputProfile,
    pub ingest: IngestProfile,
    pub slot: SlotProfile,
    pub backing: BackingProfile,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct OutputProfile {
    pub period_frames: u32,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct IngestProfile {
    pub playout_delay_ms: PlayoutDelay,
}

/// A pinned delay in milliseconds, or `"auto"`: each stream chooses its own.
#[derive(Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum PlayoutDelay {
    Pinned(f64),
    Auto(Auto),
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Auto {
    Auto,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotProfile {
    pub gain: f32,
    pub pan: f32,
    pub effects: EffectsProfile,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectsProfile {
    #[serde(rename = "pitch-correction")]
    pub pitch_correction: CorrectionProfile,
    pub reverb: ReverbProfile,
    pub voice: VoiceProfile,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct CorrectionProfile {
    pub strength: f32,
    pub retune_ms: f32,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ReverbProfile {
    pub room_size: f32,
    pub mix: f32,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct VoiceProfile {
    pub formant_semitones: f32,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BackingProfile {
    pub stem_gains: StemGainsProfile,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct StemGainsProfile {
    pub instrumental: f32,
    pub guide_vocals: f32,
}

impl Profile {
    /// The profile in `text`, from the file `name`, which errors name.
    pub fn parse(name: &str, text: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(text).map_err(|e| format!("{name}: {e}"))?;
        match value.get("version").and_then(Value::as_u64) {
            Some(VERSION) => {}
            Some(version) => {
                return Err(format!(
                    "{name}: profile version {version}: this replay reads {VERSION}"
                ));
            }
            None => return Err(format!("{name}: a profile needs a version")),
        }
        let profile: Self = serde_json::from_value(value).map_err(|e| format!("{name}: {e}"))?;
        profile.check().map_err(|e| format!("{name}: {e}"))?;
        Ok(profile)
    }

    /// The ranges `src/validate.ts` holds each setting to.
    fn check(&self) -> Result<(), String> {
        if !matches!(self.output.period_frames, 64 | 128) {
            return Err(format!(
                "output.periodFrames must be 64 or 128, got {}",
                self.output.period_frames
            ));
        }
        if let PlayoutDelay::Pinned(ms) = self.ingest.playout_delay_ms {
            within("ingest.playoutDelayMs", ms, 5.0, 100.0)?;
        }
        let slot = &self.slot;
        let effects = &slot.effects;
        let stems = &self.backing.stem_gains;
        for (name, value, min, max) in [
            ("slot.gain", slot.gain, 0.0, 4.0),
            ("slot.pan", slot.pan, -1.0, 1.0),
            ("pitch-correction.strength", effects.pitch_correction.strength, 0.0, 1.0),
            ("pitch-correction.retuneMs", effects.pitch_correction.retune_ms, 0.0, 1000.0),
            ("reverb.roomSize", effects.reverb.room_size, 0.0, 1.0),
            ("reverb.mix", effects.reverb.mix, 0.0, 1.0),
            ("voice.formantSemitones", effects.voice.formant_semitones, -12.0, 12.0),
            ("stemGains.instrumental", stems.instrumental, 0.0, 4.0),
            ("stemGains.guideVocals", stems.guide_vocals, 0.0, 4.0),
        ] {
            within(name, f64::from(value), min, max)?;
        }
        Ok(())
    }
}

/// Refuses `value` outside `min` to `max` in `src/validate.ts`'s `finiteIn` words, the value
/// printed as its own type prints it, so an `f32` 0.1 reads 0.1 as JavaScript prints it.
pub(super) fn within<T: Into<f64> + Copy + std::fmt::Display>(
    name: &str,
    value: T,
    min: f64,
    max: f64,
) -> Result<(), String> {
    let number: f64 = value.into();
    if number.is_finite() && (min..=max).contains(&number) {
        Ok(())
    } else {
        Err(format!("{name} must be between {min} and {max}, got {value}"))
    }
}
