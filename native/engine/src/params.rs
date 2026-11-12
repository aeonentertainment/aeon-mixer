//! The parameter crossing of docs/design.md section 2: the control thread writes a whole
//! snapshot, the audio thread reads the latest one at the top of each block and ramps every
//! gain it finds there across the block. `Params` is `Copy` and fixed-size so the triple buffer
//! never allocates.

use triple_buffer::{Input, Output, TripleBuffer};

use crate::block::MAX_SLOTS;

/// Every knob the audio thread reads. Gains are linear amplitude and pan is -1 (left) to 1
/// (right); the binding converts units at its edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub slots: [SlotParams; MAX_SLOTS],
    pub stems: StemGains,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotParams {
    pub gain: f32,
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
    pub effects: EffectParams,
    pub guide_offset_ms: f32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EffectParams {
    pub corrector: CorrectorParams,
    pub reverb: ReverbParams,
    pub voice: VoiceParams,
    pub cleanup: CleanupParams,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CorrectorParams {
    pub enabled: bool,
    pub strength: f32,
    pub retune_ms: f32,
}

/// The high-pass, gate and compressor, together: they have no knobs a host needs yet.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CleanupParams {
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReverbParams {
    pub enabled: bool,
    pub room_size: f32,
    pub mix: f32,
}

/// A voice: a formant move in semitones from the singer's own, -12 to 12. Decision 0014 has the
/// presets a host builds from it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct VoiceParams {
    pub enabled: bool,
    pub formant_semitones: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StemGains {
    pub instrumental: f32,
    pub guide: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self { slots: [SlotParams::default(); MAX_SLOTS], stems: StemGains::default() }
    }
}

/// Unity gain, centre pan, audible, effects off.
impl Default for SlotParams {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            mute: false,
            solo: false,
            effects: EffectParams::default(),
            guide_offset_ms: 0.0,
        }
    }
}

impl Default for StemGains {
    fn default() -> Self {
        Self { instrumental: 1.0, guide: 1.0 }
    }
}

/// The two ends of the triple buffer. The writer keeps the control thread's own copy so a call
/// that sets one knob publishes a whole, consistent snapshot.
pub fn params_channel() -> (ParamsWriter, ParamsReader) {
    let current = Params::default();
    let (input, output) = TripleBuffer::new(&current).split();
    (ParamsWriter { input, current }, ParamsReader { output })
}

pub struct ParamsWriter {
    input: Input<Params>,
    current: Params,
}

impl ParamsWriter {
    pub fn current(&self) -> &Params {
        &self.current
    }

    pub fn update(&mut self, change: impl FnOnce(&mut Params)) {
        change(&mut self.current);
        self.input.write(self.current);
    }
}

pub struct ParamsReader {
    output: Output<Params>,
}

impl ParamsReader {
    /// The newest published snapshot. Allocation-free and lock-free: the audio thread's read.
    pub fn latest(&mut self) -> &Params {
        self.output.read()
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;

    #[test]
    fn the_reader_sees_the_last_whole_snapshot() {
        let (mut writer, mut reader) = params_channel();
        assert_eq!(*reader.latest(), Params::default());

        writer.update(|p| p.slots[2].gain = 0.5);
        writer.update(|p| p.slots[2].pan = -1.0);
        let seen = assert_no_alloc(|| *reader.latest());
        assert_eq!(seen.slots[2].gain, 0.5);
        assert_eq!(seen.slots[2].pan, -1.0);
        assert_eq!(seen.slots[3], SlotParams::default());
    }
}
