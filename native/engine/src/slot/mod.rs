//! One mic input: its source, the effect chain, the meter, then gain and pan into the stereo
//! bus. The order is docs/design.md section 4; the ramps are one block.

pub mod guide;
pub mod meter;
pub mod source;

use std::f32::consts::FRAC_PI_4;

use crate::block::{BLOCK, Stereo};
use crate::command::{Event, EventProducer};
use crate::params::SlotParams;
use guide::Guide;
use meter::Meter;
use source::Source;

pub struct Slot {
    source: Box<dyn Source>,
    guide: Option<Box<Guide>>,
    meter: Meter,
    /// Left and right gains the last block ended on; `None` before the first block, which starts
    /// on its target instead of ramping up from silence.
    gains: Option<[f32; 2]>,
    input: [f32; BLOCK],
}

impl Slot {
    pub fn new(source: Box<dyn Source>) -> Self {
        Self { source, guide: None, meter: Meter::new(), gains: None, input: [0.0; BLOCK] }
    }

    /// Pulls one block, meters it, and adds it to `out` at the ramped gain. `any_solo` is whether
    /// any slot in the graph is soloed, which silences this one unless it is too.
    pub fn render(
        &mut self,
        index: u8,
        params: &SlotParams,
        any_solo: bool,
        out: &mut Stereo<'_>,
        events: &mut EventProducer,
    ) {
        self.source.pull(&mut self.input);
        // The effect chain (8.6) runs here, so the meter reads the corrected voice.
        for sample in self.input {
            if let Some(reading) = self.meter.push_frame(sample * sample, sample.abs()) {
                events.push(Event::Level {
                    slot: index,
                    rms_db: reading.rms_db,
                    peak_db: reading.peak_db,
                });
            }
        }

        let target = target_gains(params, any_solo);
        let [mut left, mut right] = self.gains.unwrap_or(target);
        let step = [(target[0] - left) / BLOCK as f32, (target[1] - right) / BLOCK as f32];
        for ((sample, l), r) in self.input.iter().zip(out.left.iter_mut()).zip(out.right.iter_mut())
        {
            left += step[0];
            right += step[1];
            *l += sample * left;
            *r += sample * right;
        }
        self.gains = Some(target);
    }

    /// Installs a guide and returns the one it displaces, for the retired ring.
    pub fn set_guide(&mut self, guide: Option<Box<Guide>>) -> Option<Box<Guide>> {
        std::mem::replace(&mut self.guide, guide)
    }

    pub fn guide(&self) -> Option<&Guide> {
        self.guide.as_deref()
    }

    pub fn reset(&mut self) {
        self.source.reset();
    }
}

/// Mute wins over solo; an unsoloed slot is silent while any other is soloed. Then the
/// constant-power pan law spreads what is left across the bus.
fn target_gains(params: &SlotParams, any_solo: bool) -> [f32; 2] {
    let audible = !params.mute && (params.solo || !any_solo);
    let gain = if audible { params.gain } else { 0.0 };
    let angle = (params.pan.clamp(-1.0, 1.0) + 1.0) * FRAC_PI_4;
    [gain * angle.cos(), gain * angle.sin()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pan_law_keeps_power_constant() {
        for pan in [-1.0, -0.5, 0.0, 0.3, 1.0] {
            let [l, r] = target_gains(&SlotParams { pan, ..SlotParams::default() }, false);
            assert!((l * l + r * r - 1.0).abs() < 1e-6, "pan {pan}: {l} {r}");
        }
        let hard_left = target_gains(&SlotParams { pan: -1.0, ..SlotParams::default() }, false);
        assert!((hard_left[0] - 1.0).abs() < 1e-6 && hard_left[1].abs() < 1e-6);
        let centre = target_gains(&SlotParams::default(), false);
        assert!((centre[0] - centre[1]).abs() < 1e-6);
    }

    #[test]
    fn mute_beats_solo_and_solo_silences_the_others() {
        let soloed = SlotParams { solo: true, ..SlotParams::default() };
        let plain = SlotParams::default();
        let muted_solo = SlotParams { mute: true, solo: true, ..SlotParams::default() };
        assert!(target_gains(&soloed, true)[0] > 0.0);
        assert_eq!(target_gains(&plain, true), [0.0, 0.0]);
        assert_eq!(target_gains(&muted_solo, true), [0.0, 0.0]);
        assert!(target_gains(&plain, false)[0] > 0.0);
    }
}
