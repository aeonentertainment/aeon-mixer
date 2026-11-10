use kernels::VoiceShifter;

use crate::block::{BLOCK, RATE};
use crate::params::VoiceParams;

/// The slot's voice stage: the voice shifter moving the singer's formants, with no transposition,
/// so it holds nothing back and adds no sample. Off, or on at 0, it hands the voice over bit for
/// bit. docs/decisions/0014-the-first-voices.md has the voices, and why a pitch voice waits for the
/// corrector's shifter.
pub struct Voice {
    kernel: VoiceShifter,
    /// The kernel reads and writes separate buffers; the slot's block is copied through this.
    scratch: [f32; BLOCK],
}

impl Voice {
    pub fn new() -> Self {
        Self { kernel: VoiceShifter::new(RATE), scratch: [0.0; BLOCK] }
    }

    /// Forgets the singer it was hearing, for a new stream on the slot.
    pub fn reset(&mut self) {
        self.kernel.reset();
    }

    /// One run of the slot's voice, in place.
    pub fn process(&mut self, params: &VoiceParams, block: &mut [f32]) {
        let formant_semitones = if params.enabled { params.formant_semitones } else { 0.0 };
        self.kernel.set(0.0, 2f32.powf(formant_semitones / 12.0));
        for run in block.chunks_mut(BLOCK) {
            let scratch = &mut self.scratch[..run.len()];
            scratch.copy_from_slice(run);
            self.kernel.process(scratch, run);
        }
    }
}

impl Default for Voice {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::f64::consts::{PI, TAU};

    use assert_no_alloc::assert_no_alloc;
    use kernels::YinDetector;

    use super::*;

    /// A vowel near /a/: where the formants sit, each with its bandwidth.
    const FORMANTS: [(f64, f64); 4] =
        [(700.0, 90.0), (1220.0, 110.0), (2600.0, 170.0), (3500.0, 250.0)];
    const SR: f64 = RATE as f64;

    fn on(formant_semitones: f32) -> VoiceParams {
        VoiceParams { enabled: true, formant_semitones }
    }

    /// A vocal tract's gain at `hz`: resonators in series over a source falling 6 dB an octave.
    fn tract(hz: f64) -> f64 {
        let w = TAU * hz;
        let mut magnitude = 1.0 / (1.0 + (hz / 100.0).powi(2)).sqrt();
        for (centre, bandwidth) in FORMANTS {
            let (sigma, omega) = (PI * bandwidth, TAU * centre);
            let pole_power = sigma * sigma + omega * omega;
            magnitude *= pole_power / (pole_power - w * w).hypot(2.0 * sigma * w);
        }
        magnitude
    }

    /// A sung vowel at `hz`, every harmonic up to 10 kHz weighted by the tract.
    fn vowel(hz: f64, frames: usize) -> Vec<f32> {
        let harmonics = (10_000.0 / hz) as usize;
        let gains: Vec<f64> = (1..=harmonics).map(|h| tract(h as f64 * hz)).collect();
        let peak: f64 = gains.iter().sum();
        (0..frames)
            .map(|i| {
                let phase = TAU * hz * i as f64 / SR;
                let sample: f64 =
                    gains.iter().enumerate().map(|(h, g)| g * ((h + 1) as f64 * phase).sin()).sum();
                (0.5 * sample / peak) as f32
            })
            .collect()
    }

    /// `input` through a new stage at `params`, a block at a time, under the allocation guard.
    fn voiced(params: VoiceParams, input: &[f32]) -> Vec<f32> {
        let mut voice = Voice::new();
        let mut output = input.to_vec();
        assert_no_alloc(|| {
            output.chunks_mut(BLOCK).for_each(|block| voice.process(&params, block))
        });
        output
    }

    /// A harmonic's level in dB, from a Hann-windowed projection onto `hz`.
    fn level_db(signal: &[f32], hz: f64) -> f64 {
        let n = signal.len() as f64;
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in signal.iter().enumerate() {
            let window = 0.5 - 0.5 * (TAU * i as f64 / n).cos();
            let phase = TAU * hz * i as f64 / SR;
            re += f64::from(*s) * window * phase.cos();
            im += f64::from(*s) * window * phase.sin();
        }
        10.0 * (re * re + im * im).max(1e-30).log10()
    }

    /// How far, in dB RMS, the harmonics of `hz` under 5 kHz sit from the tract's envelope with its
    /// formants moved by `factor`, over those within 30 dB of its loudest, after their mean level
    /// is taken out: the kernel matches level, and the shape is the question.
    fn envelope_error_db(signal: &[f32], hz: f64, factor: f64) -> f64 {
        let harmonics: Vec<(f64, f64)> = (1..)
            .map(|h| h as f64 * hz)
            .take_while(|f| *f < 5000.0)
            .map(|f| (level_db(signal, f), 20.0 * tract(f / factor).log10()))
            .collect();
        let loudest = harmonics.iter().map(|(_, asked)| *asked).fold(f64::MIN, f64::max);
        let errors: Vec<f64> = harmonics
            .iter()
            .filter(|(_, asked)| *asked > loudest - 30.0)
            .map(|(measured, asked)| measured - asked)
            .collect();
        let mean = errors.iter().sum::<f64>() / errors.len() as f64;
        (errors.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / errors.len() as f64).sqrt()
    }

    fn pitch_of(signal: &[f32]) -> f64 {
        let window = &signal[signal.len() - 2048..];
        f64::from(YinDetector::new(RATE, 2048).analyze(window).hz)
    }

    fn cents(hz: f64, reference: f64) -> f64 {
        1200.0 * (hz / reference).log2()
    }

    /// Where `signal` is loudest from `from` on.
    fn loudest_from(signal: &[f32], from: usize) -> usize {
        (from..signal.len()).max_by(|a, b| signal[*a].abs().total_cmp(&signal[*b].abs())).unwrap()
    }

    #[test]
    fn off_is_the_voice_itself() {
        let input = vowel(147.0, 24_000);
        for params in [VoiceParams { enabled: false, formant_semitones: 2.0 }, on(0.0)] {
            assert_eq!(voiced(params, &input), input, "{params:?}");
        }
    }

    // Each voice on a vowel from a low voice to a high one. Pitch must stay within decision 0004's
    // 5 cents of the singer's. The envelope is held to what the kernel reaches on these voices,
    // with room for a small change, and must sit nearer the envelope asked for than the singer's
    // own, so the formants moved the asked way. A voice over 220 Hz has too few harmonics to show
    // where its formants are.
    #[test]
    fn each_first_voice_keeps_its_pitch_and_moves_its_formants() {
        const PITCH_CENTS: f64 = 5.0;
        const ENVELOPE_DB: f64 = 2.5;
        for formant_semitones in [-2.0_f32, 2.0] {
            let factor = 2f64.powf(f64::from(formant_semitones) / 12.0);
            for hz in [82.4, 110.0, 165.0, 220.0] {
                let output = voiced(on(formant_semitones), &vowel(hz, 24_000));
                let settled = &output[12_000..];
                let off = cents(pitch_of(settled), hz);
                assert!(off.abs() < PITCH_CENTS, "{formant_semitones} at {hz} Hz: {off:+.2} cents");
                let to_asked = envelope_error_db(settled, hz, factor);
                let to_own = envelope_error_db(settled, hz, 1.0);
                assert!(
                    to_asked < ENVELOPE_DB && to_asked < to_own,
                    "{formant_semitones} at {hz} Hz: {to_asked:.2} dB from the asked envelope, \
                     {to_own:.2} from the singer's own"
                );
            }
        }
    }

    // A click after a phrase, so the filter is shaping a voice when it arrives, leaves on the very
    // sample it entered, at every pitch and a sample at a time as well as a block at a time.
    #[test]
    fn a_click_leaves_on_the_sample_it_entered() {
        for formant_semitones in [-2.0_f32, 2.0] {
            for hz in [110.0, 220.0, 440.0] {
                for run in [1, BLOCK] {
                    let click = 12_000 + 7_200;
                    let mut signal = vowel(hz, 12_000);
                    signal.resize(click + 4_800, 0.0);
                    signal[click] = 0.8;
                    let mut voice = Voice::new();
                    let params = on(formant_semitones);
                    assert_no_alloc(|| {
                        signal.chunks_mut(run).for_each(|block| voice.process(&params, block))
                    });
                    assert_eq!(
                        loudest_from(&signal, click),
                        click,
                        "{formant_semitones} at {hz} Hz, runs of {run}"
                    );
                }
            }
        }
    }

    // A tone after silence starts on the sample it entered, so the stage holds nothing back. Once
    // it sounds, the output lines up with the input best at a lag of a few samples at most: not a
    // held sample but the filter's phase near the formants it moves, as any resonance has. Brighter
    // measured 0 at every pitch here and deeper 2 to 9 samples, under 0.2 ms.
    #[test]
    fn a_tone_leaves_on_the_sample_it_entered() {
        const PHASE_SAMPLES: usize = 10;
        const LAGS: usize = 64;
        for formant_semitones in [-2.0_f32, 2.0] {
            for hz in [82.4, 110.0, 165.0, 220.0, 330.0, 440.0] {
                let onset = 4_800 + 37;
                let mut input = vec![0.0; onset];
                input.extend(vowel(hz, 24_000));
                let output = voiced(on(formant_semitones), &input);
                let starts = |signal: &[f32]| signal.iter().position(|s| *s != 0.0);
                assert_eq!(starts(&output), starts(&input), "{formant_semitones} at {hz} Hz");

                let window = &input[onset + 12_000..onset + 22_000];
                let lined_up = |lag: usize| -> f64 {
                    let heard = &output[onset + 12_000 + lag..onset + 22_000 + lag];
                    window.iter().zip(heard).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum()
                };
                let best = (0..LAGS).max_by(|a, b| lined_up(*a).total_cmp(&lined_up(*b)));
                assert!(
                    best.is_some_and(|lag| lag <= PHASE_SAMPLES),
                    "{formant_semitones} at {hz} Hz: lines up at {best:?}"
                );
            }
        }
    }
}
