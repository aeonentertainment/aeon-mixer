//! One line of `events.jsonl` or `stats.jsonl`, as `src/bundle/bundle-event.ts` and
//! `src/bundle/bundle.ts` write them. Every line carries the clock pair; an event line also
//! carries the call or report, with what the call left out left out (docs/design.md 8.12).

use serde::Deserialize;

use super::manifest::StemKind;

#[derive(Deserialize)]
pub struct Line<E> {
    pub frame: f64,
    pub us: f64,
    #[serde(flatten)]
    pub body: E,
}

/// A stats line: the replay reads only its clock pair.
#[derive(Deserialize)]
pub struct Stats {}

#[derive(Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Event {
    SetSlot { index: usize, settings: SlotSettings },
    SetSlotEffects { index: usize, effects: Vec<Effect> },
    SetSlotGuide { index: usize, guide: Option<Guide>, guide_offset_ms: Option<f32> },
    ResetSlot { index: usize },
    Start {},
    Seek { to_ms: f64 },
    Pause {},
    Resume {},
    SetBackingKey { semitones: i8 },
    SetStemGain { kind: StemKind, gain: f32 },
    OutputChanged {},
    Ack {},
    BackingRendered { frames: u64 },
    BackingEnded {},
    Error { message: String },
    End {},
}

#[derive(Deserialize, PartialEq)]
pub struct SlotSettings {
    pub gain: Option<f32>,
    pub pan: Option<f32>,
    pub mute: Option<bool>,
    pub solo: Option<bool>,
}

/// One effect of a `setSlotEffects` call, a parameter it left out as `None`.
#[derive(Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum Effect {
    PitchCorrection {
        enabled: bool,
        strength: Option<f32>,
        retune_ms: Option<f32>,
    },
    Reverb {
        enabled: bool,
        room_size: Option<f32>,
        mix: Option<f32>,
    },
    Voice {
        enabled: bool,
        formant_semitones: Option<f32>,
    },
    EchoCancel {
        enabled: bool,
    },
    Cleanup {
        enabled: bool,
    },
    HowlGuard {
        enabled: bool,
    },
    Tone {
        enabled: bool,
        bands: Option<Vec<ToneBand>>,
        plosive_db: Option<f32>,
        air_db: Option<f32>,
        de_ess_db: Option<f32>,
    },
}

/// One band of a tone effect. A bundle holds only calls the mixer took, so never `"match"`,
/// which the mixer refuses; a line that holds one fails to read.
#[derive(Clone, Copy, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum ToneBand {
    Peak { hz: f32, gain_db: f32, q: f32 },
    LowShelf { hz: f32, gain_db: f32 },
    HighShelf { hz: f32, gain_db: f32 },
}

#[derive(Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Guide {
    pub notes: Vec<GuideNote>,
    pub key_mask: u16,
}

#[derive(Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GuideNote {
    pub midi: u8,
    pub start_ms: f64,
    pub end_ms: f64,
}

/// Every whole line of a JSON lines file. A process that dies mid-write can leave a last line
/// with no newline, which is left out: the bundle reads to its last whole line.
pub fn read_lines<E: for<'de> Deserialize<'de>>(
    name: &str,
    text: &str,
) -> Result<Vec<Line<E>>, String> {
    let whole = text.rfind('\n').map_or("", |end| &text[..end]);
    whole
        .lines()
        .enumerate()
        .map(|(n, line)| {
            serde_json::from_str(line).map_err(|e| format!("{name} line {}: {e}", n + 1))
        })
        .collect()
}
