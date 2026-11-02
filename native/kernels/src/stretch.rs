use std::ptr::NonNull;

use crate::ffi;

/// The most channels one stretcher takes: the pointer arrays `process` hands the shim live on the
/// stack.
const MAX_CHANNELS: usize = 8;

/// Signalsmith Stretch's two configurations. `Default` is its best quality, 120 ms blocks every
/// 30 ms; `Cheaper` is 100 ms every 40 ms with its work split evenly across calls, so a 2.67 ms
/// block never pays for a whole spectrum at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Default,
    Cheaper,
}

/// Signalsmith Stretch: transposes and time-stretches planar audio in the frequency domain. Built
/// on the control thread; `process` allocates nothing.
pub struct Stretch {
    raw: NonNull<ffi::Stretch>,
    channels: usize,
}

// The C++ object has no thread affinity and this wrapper is its only owner.
unsafe impl Send for Stretch {}

impl Stretch {
    /// Panics unless `channels` is 1 to 8.
    pub fn new(channels: usize, rate: u32, preset: Preset) -> Self {
        assert!((1..=MAX_CHANNELS).contains(&channels), "1 to {MAX_CHANNELS} channels");
        let preset = match preset {
            Preset::Default => 0,
            Preset::Cheaper => 1,
        };
        // SAFETY: the shim returns an owned object or null, and takes plain numbers
        let raw = unsafe { ffi::aeon_stretch_new(channels as i32, rate as f32, preset) };
        Self { raw: NonNull::new(raw).expect("the stretcher could not allocate"), channels }
    }

    /// The transposition from the next `process` on.
    pub fn set_transpose_semitones(&mut self, semitones: f32) {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_stretch_set_transpose_semitones(self.raw.as_ptr(), semitones) }
    }

    /// One slice per channel in and out. Every input channel is one length and every output
    /// channel another; they differ to stretch time and match to keep it. Panics on any other
    /// shape.
    pub fn process(&mut self, input: &[&[f32]], output: &mut [&mut [f32]]) {
        assert_eq!(input.len(), self.channels, "one input slice per channel");
        assert_eq!(output.len(), self.channels, "one output slice per channel");
        let input_frames = input[0].len();
        let output_frames = output[0].len();
        assert!(input.iter().all(|c| c.len() == input_frames), "input channels of one length");
        assert!(output.iter().all(|c| c.len() == output_frames), "output channels of one length");

        let mut inputs = [std::ptr::null::<f32>(); MAX_CHANNELS];
        let mut outputs = [std::ptr::null_mut::<f32>(); MAX_CHANNELS];
        for (pointer, channel) in inputs.iter_mut().zip(input) {
            *pointer = channel.as_ptr();
        }
        for (pointer, channel) in outputs.iter_mut().zip(output.iter_mut()) {
            *pointer = channel.as_mut_ptr();
        }
        let input_frames = i32::try_from(input_frames).expect("a run that fits an i32");
        let output_frames = i32::try_from(output_frames).expect("a run that fits an i32");
        // SAFETY: the first `channels` pointers each address that many frames, outputs are
        // `&mut` so none aliases an input, and the stretcher reads only `channels` of them
        unsafe {
            ffi::aeon_stretch_process(
                self.raw.as_ptr(),
                inputs.as_ptr(),
                input_frames,
                outputs.as_ptr(),
                output_frames,
            )
        }
    }

    /// Forgets what it has heard; allocates nothing.
    pub fn reset(&mut self) {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_stretch_reset(self.raw.as_ptr()) }
    }

    /// How far the output trails the input, in frames.
    pub fn latency_frames(&self) -> usize {
        // SAFETY: `raw` is live for as long as `self`
        unsafe { ffi::aeon_stretch_latency_frames(self.raw.as_ptr()) as usize }
    }
}

impl Drop for Stretch {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `aeon_stretch_new` and is freed once, here
        unsafe { ffi::aeon_stretch_free(self.raw.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::test_support::{RATE, cents_between, harmonic_tone, median_pitch};

    /// A stereo tone through a stretcher in engine blocks, under the allocation guard.
    fn transposed(preset: Preset, semitones: f32, tone: &[f32]) -> [Vec<f32>; 2] {
        let mut stretch = Stretch::new(2, RATE, preset);
        stretch.set_transpose_semitones(semitones);
        let mut left = vec![0.0; tone.len()];
        let mut right = vec![0.0; tone.len()];
        assert_no_alloc(|| {
            for ((input, left), right) in
                tone.chunks(128).zip(left.chunks_mut(128)).zip(right.chunks_mut(128))
            {
                stretch.process(&[input, input], &mut [left, right]);
            }
        });
        [left, right]
    }

    // The backing path's live key: a tone moves by what is asked, on both channels
    #[test]
    fn moves_a_tone_by_the_asked_semitones() {
        let tone = harmonic_tone(220.0, 2.0);
        for preset in [Preset::Default, Preset::Cheaper] {
            for semitones in [-5.0f32, 3.0, 7.0] {
                let expected = 220.0 * 2f64.powf(f64::from(semitones) / 12.0);
                for channel in transposed(preset, semitones, &tone) {
                    let cents = cents_between(median_pitch(&channel, 0.5, 2.0), expected);
                    assert!(
                        cents.abs() < 5.0,
                        "{preset:?} {semitones:+} semitones came out {cents:+.1} cents off"
                    );
                }
            }
        }
    }

    // What the backing path subtracts from its position: a tone that starts after silence comes
    // out `latency_frames` later, to within 5 ms
    #[test]
    fn a_tone_comes_out_latency_frames_late() {
        let onset = RATE as usize / 2;
        let mut input = vec![0.0; onset];
        input.extend(harmonic_tone(220.0, 1.0));
        for preset in [Preset::Default, Preset::Cheaper] {
            let latency = Stretch::new(2, RATE, preset).latency_frames();
            let [output, _] = transposed(preset, 0.0, &input);
            // Where the output's 5 ms RMS first reaches half the steady tone's
            let window = 240;
            let rms = |at: usize| {
                (output[at..at + window].iter().map(|s| s * s).sum::<f32>() / window as f32).sqrt()
            };
            let steady = rms(output.len() - window);
            let heard =
                (0..output.len() - window).find(|at| rms(*at) > steady / 2.0).unwrap() + window / 2;
            let drift = heard as f64 - (onset + latency) as f64;
            assert!(
                drift.abs() < 240.0,
                "{preset:?}: heard at {heard}, onset plus latency {}",
                onset + latency
            );
        }
    }
}
