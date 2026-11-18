use crate::params::CleanupParams;
use crate::slot::effects::tone::{Biquad, Section};

/// Where the filter turns over. A sung E2 fundamental, 82 Hz, loses 2.8 dB; handling thumps at
/// 50 Hz lose 9 and at 25 Hz about 20. Decision 0011 has why it is not higher or steeper.
const CORNER_HZ: f64 = 80.0;

/// The slot's high-pass: a 12 dB per octave Butterworth that takes handling noise, stand rumble
/// and the low end of plosives out of the voice. A causal IIR, so it buffers nothing: a click comes
/// out on the sample it went in. Its phase shift is still a cost on the mic path: it delays a
/// 220 Hz fundamental by 0.41 ms, a 110 Hz one by 1.8 ms and an 82 Hz one by 2.7 ms.
pub struct HighPass {
    pass: Biquad,
    section: Section,
    /// How much of the filtered voice this block ends on, 0 or 1; on and off ramp across a block.
    wet: f32,
}

impl HighPass {
    pub fn new() -> Self {
        Self { pass: Biquad::high_pass(CORNER_HZ), section: Section::new(), wet: 0.0 }
    }

    /// One run of the slot's voice, in place. Turned on or off, it crossfades across one block;
    /// off, it then costs nothing.
    pub fn process(&mut self, params: &CleanupParams, block: &mut [f32]) {
        let target = if params.enabled { 1.0 } else { 0.0 };
        if target == 0.0 && self.wet == 0.0 {
            return;
        }
        let step = (target - self.wet) / block.len().max(1) as f32;
        for sample in block {
            let filtered = self.section.tick(&self.pass, f64::from(*sample));
            self.wet += step;
            *sample += self.wet * (filtered as f32 - *sample);
        }
        self.wet = target;
        if target == 0.0 {
            self.section = Section::new();
        }
    }
}

impl Default for HighPass {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::TAU;

    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::block::{BLOCK, RATE};

    const ON: CleanupParams = CleanupParams { enabled: true };

    fn sine_gain_db(hz: f32) -> f32 {
        let mut filter = HighPass::new();
        let frames = RATE as usize;
        let input: Vec<f32> =
            (0..frames).map(|i| (TAU * hz * i as f32 / RATE as f32).sin() * 0.5).collect();
        let mut output = input.clone();
        assert_no_alloc(|| output.chunks_mut(BLOCK).for_each(|block| filter.process(&ON, block)));
        let rms = |s: &[f32]| (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt();
        // The last half second, past the filter's settling
        20.0 * (rms(&output[frames / 2..]) / rms(&input[frames / 2..])).log10()
    }

    #[test]
    fn a_click_comes_out_on_the_sample_it_went_in() {
        let mut filter = HighPass::new();
        filter.process(&ON, &mut [0.0; BLOCK]);
        let mut block = [0.0f32; BLOCK];
        block[37] = 1.0;
        assert_no_alloc(|| filter.process(&ON, &mut block));
        let loudest = block
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(at, _)| at);
        assert_eq!(loudest, Some(37));
        assert!(block[..37].iter().all(|s| *s == 0.0), "output before the click");
    }

    #[test]
    fn rumble_goes_and_the_voice_stays() {
        // Butterworth at 80 Hz: -20.7 dB at 25 Hz, -8.8 at 50, -2.8 at 82, -0.1 at 220
        assert!(sine_gain_db(25.0) < -19.0, "25 Hz: {}", sine_gain_db(25.0));
        assert!(sine_gain_db(50.0) < -8.0, "50 Hz: {}", sine_gain_db(50.0));
        assert!(sine_gain_db(82.0) > -3.5, "82 Hz: {}", sine_gain_db(82.0));
        assert!(sine_gain_db(220.0).abs() < 0.2, "220 Hz: {}", sine_gain_db(220.0));
    }

    #[test]
    fn off_is_the_voice_itself() {
        let mut filter = HighPass::new();
        let input: Vec<f32> = (0..4 * BLOCK).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut output = input.clone();
        let off = CleanupParams { enabled: false };
        assert_no_alloc(|| output.chunks_mut(BLOCK).for_each(|block| filter.process(&off, block)));
        assert_eq!(output, input);
    }
}
