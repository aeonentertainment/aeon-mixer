//! A slot's corrector where its parameters move under it: a ramp interrupted by another, and
//! correction engaged on a voice already sounding. Each renders through a real slot on the offline
//! driver's blocks, under the allocation guard.

use assert_no_alloc::assert_no_alloc;
use kernels::fixtures::{BUZZ, harmonic_voice};

use super::*;
use crate::command::event_ring;
use crate::params::{CorrectorParams, EffectParams};
use crate::slot::effects::Corrector;

const ON: CorrectorParams = CorrectorParams { enabled: true, strength: 1.0, retune_ms: 40.0 };

/// Hands over `samples`, then silence.
struct Clip {
    samples: Vec<f32>,
    at: usize,
}

impl Source for Clip {
    fn pull(&mut self, out: &mut [f32]) {
        for sample in out {
            *sample = self.samples.get(self.at).copied().unwrap_or(0.0);
            self.at += 1;
        }
    }
}

/// `input` through a new slot, hard left, with block `n`'s corrector from `corrector_at(n)`.
/// Returns the left channel and the delay stats read after each block.
fn through(input: &[f32], corrector_at: impl Fn(usize) -> CorrectorParams) -> (Vec<f32>, Vec<u32>) {
    let mut slot = Slot::new(Box::new(Clip { samples: input.to_vec(), at: 0 }));
    let delay = slot.corrector_delay();
    let (mut events, _drain) = event_ring();
    let history = EchoHistory::new();
    let (mut output, mut delays) = (Vec::new(), Vec::new());
    for block in 0..input.len() / BLOCK {
        let params = SlotParams {
            pan: -1.0,
            effects: EffectParams { corrector: corrector_at(block), ..EffectParams::default() },
            ..SlotParams::default()
        };
        let (mut left, mut right) = ([0.0; BLOCK], [0.0; BLOCK]);
        assert_no_alloc(|| {
            let mut out = Stereo { left: &mut left, right: &mut right };
            let shared = Shared { any_solo: false, cue: Cue::default(), history: &history };
            slot.render(0, &params, shared, &mut out, &mut events);
        });
        output.extend_from_slice(&left);
        delays.push(delay.frames());
    }
    (output, delays)
}

fn peak(signal: &[f32]) -> f32 {
    signal.iter().fold(0.0, |most, sample| most.max(sample.abs()))
}

/// The largest step between two neighbouring samples.
fn steepest(signal: &[f32]) -> f32 {
    signal.windows(2).map(|pair| (pair[1] - pair[0]).abs()).fold(0.0, f32::max)
}

// A host that flips correction on and off every 8 ms, faster than its 20 ms crossfade, gets a voice
// no louder than the singer's and a delay inside the ceiling; once it settles on off, the slot is
// the input bit for bit and reads no delay
#[test]
fn a_correction_ramp_interrupted_by_another_settles_on_the_last() {
    let frames = 2 * RATE as usize;
    let input = harmonic_voice(frames, &BUZZ, |_| 112.6);
    let flipping = frames / BLOCK * 3 / 4;
    let (output, delays) = through(&input, |block| match block {
        _ if block >= flipping => CorrectorParams { enabled: false, ..ON },
        _ if (block / 3) % 2 == 0 => ON,
        _ => CorrectorParams { strength: 0.0, ..ON },
    });
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(peak(&output) <= 1.3 * peak(&input), "{} against {}", peak(&output), peak(&input));
    let most = delays.iter().copied().max().unwrap();
    assert!(most <= Corrector::MAX_LATENCY_FRAMES, "the delay reached {most}");
    assert_eq!(*delays.last().unwrap(), 0, "off reads no delay");
    let settled = (flipping + 20) * BLOCK;
    for frame in settled..output.len() {
        assert_eq!(output[frame].to_bits(), input[frame].to_bits(), "off changed frame {frame}");
    }
}

// A singer who turns correction on mid-note hears it fade in over its crossfade: no step in the
// first 50 ms is steeper than the voice's own
#[test]
fn correction_engaged_on_a_voice_already_sounding_fades_in() {
    let frames = RATE as usize / 2;
    let input = harmonic_voice(frames + 12_345, &BUZZ, |_| 225.2).split_off(12_345);
    let (output, delays) = through(&input, |_| ON);
    assert!(peak(&output) <= 1.3 * peak(&input));
    assert!(delays.iter().all(|delay| *delay <= Corrector::MAX_LATENCY_FRAMES));
    let onset = &output[..RATE as usize / 20];
    assert!(steepest(onset) <= 1.5 * steepest(&input), "a step of {}", steepest(onset));
    assert!(*delays.last().unwrap() > 0, "correction engaged");
}
