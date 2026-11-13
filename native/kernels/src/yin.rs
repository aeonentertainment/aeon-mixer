use std::ptr::NonNull;

use crate::ffi;
use crate::pitch::Pitch;

/// YIN with the difference function taken by FFT over the whole window: the corrector's own
/// detector, for anything else that needs to read a pitch. Hand `analyze` the newest
/// `window_samples()` samples; a reading describes the window's centre. 2048 samples at 48 kHz
/// answer a pitch change in about 40 ms. Outside silence it always names its best period, and
/// the confidence says how periodic the window is.
pub struct YinDetector {
    raw: NonNull<ffi::Yin>,
    window_samples: usize,
}

// The C++ object has no thread affinity and this wrapper is its only owner.
unsafe impl Send for YinDetector {}

impl YinDetector {
    pub fn new(rate: u32, window_samples: usize) -> Self {
        let window = i32::try_from(window_samples).expect("a window that fits an i32");
        // SAFETY: the shim returns an owned object or null, and takes plain numbers
        let raw = unsafe { ffi::aeon_yin_new(f64::from(rate), window) };
        Self { raw: NonNull::new(raw).expect("the detector could not allocate"), window_samples }
    }

    /// The pitch of `window`, oldest sample first. Panics unless it is `window_samples()` long.
    pub fn analyze(&mut self, window: &[f32]) -> Pitch {
        assert_eq!(window.len(), self.window_samples, "a window of the detector's length");
        // SAFETY: the detector reads exactly `window_samples` samples, which `window` holds
        unsafe { ffi::aeon_yin_analyze(self.raw.as_ptr(), window.as_ptr()) }
    }

    pub fn window_samples(&self) -> usize {
        self.window_samples
    }
}

impl Drop for YinDetector {
    fn drop(&mut self) {
        // SAFETY: `raw` came from `aeon_yin_new` and is freed once, here
        unsafe { ffi::aeon_yin_free(self.raw.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use assert_no_alloc::assert_no_alloc;

    use super::*;
    use crate::test_support::{RATE, cents_between, harmonic_tone};

    // Both windows Aeon Karaoke measured: the corrector's 2048 across a singing range, and 1024
    // above the low voices, where little of its short window is left to compare
    #[test]
    fn reads_tones_across_the_range() {
        for (window, tones) in
            [(2048, &[82.4, 110.0, 196.0, 440.0, 880.0][..]), (1024, &[196.0, 440.0, 880.0])]
        {
            let mut yin = YinDetector::new(RATE, window);
            for &hz in tones {
                let tone = harmonic_tone(hz, 0.1);
                let pitch = assert_no_alloc(|| yin.analyze(&tone[tone.len() - window..]));
                let cents = cents_between(f64::from(pitch.hz), hz);
                assert!(cents.abs() < 3.0, "window {window} read {} Hz for {hz} Hz", pitch.hz);
                // The corrector's own bar: a clean tone anywhere in the range counts as a voice
                assert!(
                    pitch.confidence > 0.9,
                    "window {window}: confidence {} on {hz} Hz",
                    pitch.confidence
                );
            }
        }
    }

    // The dip that finds a period leans sharp, by up to 4 cents in a short window, and a corrector
    // would hand that lean to the voice. Whatever the window and wherever it falls on the wave, a
    // steady tone reads true within 0.4 cents
    #[test]
    fn a_steady_tone_reads_true_in_every_window() {
        for (window, tones) in [
            (512, &[207.0, 220.0, 330.0, 440.0, 660.0, 880.0][..]),
            (1024, &[104.0, 110.0, 150.0, 220.0, 440.0, 880.0]),
            (2048, &[82.4, 92.5, 110.0, 220.0, 440.0, 880.0]),
        ] {
            let mut yin = YinDetector::new(RATE, window);
            for &hz in tones {
                let tone = harmonic_tone(hz, 0.3);
                for start in (3000..7000).step_by(97) {
                    let read = yin.analyze(&tone[start..start + window]);
                    let cents = cents_between(f64::from(read.hz), hz);
                    assert!(
                        cents.abs() < 0.4,
                        "window {window} read {hz} Hz {cents:+.2} cents off"
                    );
                }
            }
        }
    }

    // Noise gets a best guess YIN is unsure of; silence gets nothing
    #[test]
    fn noise_is_unsure_and_silence_has_no_pitch() {
        let mut yin = YinDetector::new(RATE, 2048);
        let mut seed = 3u32;
        let noise: Vec<f32> = (0..2048)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed as f32 / 4_294_967_296.0 * 2.0 - 1.0) * 0.5
            })
            .collect();
        let confidence = yin.analyze(&noise).confidence;
        assert!(confidence < 0.5, "white noise read as a pitch with confidence {confidence}");
        assert_eq!(yin.analyze(&[0.0; 2048]).hz, 0.0, "silence read as a pitch");
    }
}
