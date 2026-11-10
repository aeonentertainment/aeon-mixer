//! One mic input: its source, the effect chain, the meter, then gain and pan into the stereo
//! bus. The order is docs/design.md section 4; the ramps are one block.

pub mod cue;
pub mod effects;
pub mod guide;
pub mod meter;
pub mod source;

use std::f32::consts::FRAC_PI_4;

use crate::block::{BLOCK, RATE, Stereo};
use crate::command::{Event, EventProducer};
use crate::params::{CorrectorParams, SlotParams};
use cue::Cue;
use effects::{Corrector, Reverb};
use guide::Guide;
use meter::Meter;
use source::Source;

pub struct Slot {
    source: Box<dyn Source>,
    guide: Option<Box<Guide>>,
    corrector: Corrector,
    /// Whether the corrector does what its params ask. `new` leaves this off until #44.
    engaged: bool,
    reverb: Reverb,
    meter: Meter,
    /// Left and right gains the last block ended on; `None` before the first block, which starts
    /// on its target instead of ramping up from silence.
    gains: Option<[f32; 2]>,
    input: [f32; BLOCK],
}

impl Slot {
    pub fn new(source: Box<dyn Source>) -> Self {
        Self {
            source,
            guide: None,
            corrector: Corrector::new(),
            engaged: false,
            reverb: Reverb::new(),
            meter: Meter::new(),
            gains: None,
            input: [0.0; BLOCK],
        }
    }

    /// A slot whose corrector shifts the voice as its params ask, where `new` holds it at strength
    /// zero. Only measure-corrector builds one: it is how the engaged corrector gets measured on
    /// the real slot chain before #44 turns it on for singers.
    pub fn engaged(source: Box<dyn Source>) -> Self {
        Self { engaged: true, ..Self::new(source) }
    }

    /// How far the corrector makes the voice trail what `source` handed over, now; 0 while
    /// bypassed.
    pub fn corrector_latency_frames(&self) -> usize {
        self.corrector.latency_frames()
    }

    /// Pulls one block, corrects it toward the guide at `cue`, meters it, adds reverb, and adds it
    /// to `out` at the ramped gain. `any_solo` is whether any slot in the graph is soloed, which
    /// silences this one unless it is too.
    pub fn render(
        &mut self,
        index: u8,
        params: &SlotParams,
        any_solo: bool,
        cue: Cue,
        out: &mut Stereo<'_>,
        events: &mut EventProducer,
    ) {
        self.source.pull(&mut self.input);
        let guide_offset =
            (f64::from(params.guide_offset_ms) * f64::from(RATE) / 1000.0).round() as i64;
        let frame = cue.frame - i64::from(self.source.latency_frames()) + guide_offset;
        // Correction stays bypassed on the mic path: engaged, the shifter delays the voice by up
        // to 15 ms, which breaks the latency budget on slower devices until measure-latency shows
        // otherwise. Bypassed it adds no delay and still reads pitch.
        let corrector = if self.engaged {
            params.effects.corrector
        } else {
            CorrectorParams { strength: 0.0, ..params.effects.corrector }
        };
        self.corrector.process(
            &corrector,
            self.guide.as_deref(),
            frame,
            cue.semitones,
            &mut self.input,
            |reading| {
                events.push(Event::Pitch {
                    slot: index,
                    position_ms: reading.position_ms,
                    hz: reading.hz,
                    confidence: reading.confidence,
                });
            },
        );
        // The meter reads the corrected voice before the reverb, so the level is the singing
        for sample in self.input {
            if let Some(reading) = self.meter.push_frame(sample * sample, sample.abs()) {
                events.push(Event::Level {
                    slot: index,
                    rms_db: reading.rms_db,
                    peak_db: reading.peak_db,
                });
            }
        }
        self.reverb.process(&params.effects.reverb, &mut self.input);

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

    /// A new singer: the source forgets its stream and the corrector what it heard.
    pub fn reset(&mut self) {
        self.source.reset();
        self.corrector.reset();
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
    use std::f64::consts::PI;

    use assert_no_alloc::assert_no_alloc;
    use kernels::YinDetector;

    use super::*;
    use crate::command::event_ring;
    use crate::params::{CorrectorParams, EffectParams};

    /// A voice held 40 cents above A3.
    struct SharpVoice {
        at: usize,
    }

    fn sharp_a3(frame: usize) -> f32 {
        let hz = 220.0 * 2f64.powf(40.0 / 1200.0);
        let t = frame as f64 / f64::from(RATE);
        let sample: f64 = (1..=12).map(|h| (2.0 * PI * hz * h as f64 * t).sin() / h as f64).sum();
        (0.25 * sample) as f32
    }

    impl Source for SharpVoice {
        fn pull(&mut self, out: &mut [f32]) {
            for sample in out {
                *sample = sharp_a3(self.at);
                self.at += 1;
            }
        }
    }

    /// One second of the voice through `slot` at full strength, hard left so the left channel is
    /// the slot's mono output, under the allocation guard.
    fn one_second(mut slot: Slot) -> (Vec<f32>, usize) {
        let params = SlotParams {
            pan: -1.0,
            effects: EffectParams {
                corrector: CorrectorParams { enabled: true, strength: 1.0, retune_ms: 0.0 },
                ..EffectParams::default()
            },
            ..SlotParams::default()
        };
        let (mut events, _drain) = event_ring();
        let mut output = Vec::with_capacity(RATE as usize);
        for _ in 0..RATE as usize / BLOCK {
            let mut left = [0.0; BLOCK];
            let mut right = [0.0; BLOCK];
            assert_no_alloc(|| {
                let mut out = Stereo { left: &mut left, right: &mut right };
                slot.render(0, &params, false, Cue::default(), &mut out, &mut events);
            });
            output.extend_from_slice(&left);
        }
        (output, slot.corrector_latency_frames())
    }

    // What ships: a slot from `new` asked for full strength hands the voice over untouched, with
    // no delay. #44 deletes this test along with `Slot::engaged`.
    #[test]
    fn a_new_slot_stays_bypassed_whatever_its_params_ask() {
        let (output, latency) = one_second(Slot::new(Box::new(SharpVoice { at: 0 })));
        for (frame, sample) in output.iter().enumerate() {
            assert_eq!(*sample, sharp_a3(frame), "frame {frame} changed");
        }
        assert_eq!(latency, 0);
    }

    // What the tool measures: the same voice on an engaged slot lands on A3 within the 10 cents
    // the corrector's own tests allow, and trails by the shifter's delay.
    #[test]
    fn an_engaged_slot_corrects_and_reports_its_delay() {
        let (output, latency) = one_second(Slot::engaged(Box::new(SharpVoice { at: 0 })));
        let window = 2048;
        let read = YinDetector::new(RATE, window).analyze(&output[output.len() - window..]);
        let cents = 1200.0 * (f64::from(read.hz) / 220.0).log2();
        assert!(cents.abs() < 10.0, "came out {cents:+.1} cents from A3");
        // The shifter's floor, and at most an eighth over one period of the voice
        let bound = 64 + (1.125 * f64::from(RATE) / 220.0) as usize;
        assert!(latency > 0 && latency <= bound, "latency {latency}, bound {bound}");
    }

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
