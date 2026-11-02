mod allpass;
mod comb;

use allpass::Allpass;
use comb::Comb;

/// Jezar's tunings at 44.1 kHz, scaled to the stream's rate: eight parallel combs, then four
/// allpasses in series. The left channel's; a slot is mono.
const COMB_LENGTHS: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
const ALLPASS_LENGTHS: [usize; 4] = [556, 441, 341, 225];
const INPUT_GAIN: f32 = 0.015;
const WET_SCALE: f32 = 3.0;
const DAMP: f32 = 0.2;

/// Freeverb, ported to Rust and made mono, because a slot is mono until its pan. `set` takes
/// effect across the next `process` call, so a knob never clicks; at mix 0 the input passes
/// untouched and the tank does no work.
pub struct Reverb {
    combs: [Comb; 8],
    allpasses: [Allpass; 4],
    feedback: f32,
    mix: f32,
    target_mix: f32,
}

impl Reverb {
    pub fn new(rate: u32) -> Self {
        let scale = |length: usize| length * rate as usize / 44_100;
        Self {
            combs: COMB_LENGTHS.map(|length| Comb::new(scale(length))),
            allpasses: ALLPASS_LENGTHS.map(|length| Allpass::new(scale(length))),
            feedback: 0.0,
            mix: 0.0,
            target_mix: 0.0,
        }
    }

    /// `room_size` from 0 to 1 sets how long the tail rings; `mix` from 0 to 1 how much of it is
    /// heard, the dry signal falling as it rises.
    pub fn set(&mut self, room_size: f32, mix: f32) {
        self.feedback = room_size.clamp(0.0, 1.0) * 0.28 + 0.7;
        self.target_mix = mix.clamp(0.0, 1.0);
    }

    /// One run of mono audio, in place, with the mix ramped from its last value to the newest
    /// `set` across it.
    pub fn process(&mut self, block: &mut [f32]) {
        if self.mix == 0.0 && self.target_mix == 0.0 {
            return;
        }
        if self.mix == 0.0 {
            // Back from bypass: a tail left from before would ring out of nowhere
            self.combs.iter_mut().for_each(Comb::clear);
            self.allpasses.iter_mut().for_each(Allpass::clear);
        }
        let step = (self.target_mix - self.mix) / block.len().max(1) as f32;
        for sample in block.iter_mut() {
            self.mix += step;
            let input = *sample * INPUT_GAIN;
            let tank: f32 =
                self.combs.iter_mut().map(|comb| comb.process(input, self.feedback, DAMP)).sum();
            let wet =
                self.allpasses.iter_mut().fold(tank, |signal, allpass| allpass.process(signal));
            *sample = *sample * (1.0 - self.mix) + wet * self.mix * WET_SCALE;
        }
        self.mix = self.target_mix;
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::test_support::{RATE, harmonic_tone};

    /// The wet signal an impulse leaves, in 128-frame blocks under the allocation guard.
    fn impulse_response(room_size: f32, seconds: f64) -> Vec<f32> {
        let mut reverb = Reverb::new(RATE);
        reverb.set(room_size, 1.0);
        let mut signal = vec![0.0; (seconds * f64::from(RATE)) as usize];
        signal[0] = 1.0;
        // The first block ramps the mix in; ramp it before the impulse arrives
        reverb.process(&mut [0.0; 128]);
        assert_no_alloc(|| signal.chunks_mut(128).for_each(|block| reverb.process(block)));
        signal
    }

    fn energy(signal: &[f32]) -> f32 {
        signal.iter().map(|s| s * s).sum()
    }

    #[test]
    fn mix_zero_is_the_input_itself() {
        let input = harmonic_tone(220.0, 0.1);
        let mut output = input.clone();
        let mut reverb = Reverb::new(RATE);
        reverb.set(0.8, 0.0);
        assert_no_alloc(|| output.chunks_mut(128).for_each(|block| reverb.process(block)));
        assert_eq!(output, input);
    }

    #[test]
    fn an_impulse_rings_and_decays() {
        let second = RATE as usize;
        let response = impulse_response(0.5, 2.0);
        let early = energy(&response[..second / 2]);
        let late = energy(&response[second..]);
        assert!(early > 0.0, "the impulse left no tail");
        assert!(late < early / 100.0, "the tail did not decay: {late} late against {early} early");
    }

    #[test]
    fn a_bigger_room_rings_longer() {
        let tail = |room_size| {
            let response = impulse_response(room_size, 1.0);
            energy(&response[RATE as usize / 2..]) / energy(&response)
        };
        assert!(tail(0.9) > 2.0 * tail(0.2), "room 0.9 rang no longer than room 0.2");
    }

    #[test]
    fn a_mix_change_ramps_across_the_block() {
        let mut reverb = Reverb::new(RATE);
        reverb.set(0.5, 1.0);
        let mut block = [1.0f32; 128];
        reverb.process(&mut block);
        // The dry share falls from all of it to none across the block, with no step
        let steps = block.windows(2).map(|pair| (pair[1] - pair[0]).abs());
        assert!(steps.fold(0.0, f32::max) < 0.05, "the mix stepped inside the ramp");
        assert!(block[0] > 0.95 && block[127].abs() < 0.1);
    }
}
