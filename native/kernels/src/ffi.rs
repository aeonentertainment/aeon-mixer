//! The shims in cpp/shim, declared again. Each opaque type is the C++ object itself, so a
//! pointer from `*_new` is owned by exactly one wrapper and freed by its `Drop`.

use std::marker::{PhantomData, PhantomPinned};

use crate::pitch::Pitch;

macro_rules! opaque {
    ($name:ident) => {
        #[repr(C)]
        pub struct $name {
            _data: [u8; 0],
            _marker: PhantomData<(*mut u8, PhantomPinned)>,
        }
    };
}

opaque!(Corrector);
opaque!(Yin);
opaque!(Stretch);

unsafe extern "C" {
    pub fn aeon_corrector_new(rate: f64) -> *mut Corrector;
    pub fn aeon_corrector_free(corrector: *mut Corrector);
    pub fn aeon_corrector_set_target(
        corrector: *mut Corrector,
        mask: u16,
        strength: f32,
        retune_ms: f32,
    );
    pub fn aeon_corrector_process(
        corrector: *mut Corrector,
        input: *const f32,
        output: *mut f32,
        frames: i32,
    );
    pub fn aeon_corrector_reset(corrector: *mut Corrector);
    pub fn aeon_corrector_detected(corrector: *const Corrector) -> Pitch;
    pub fn aeon_corrector_frames_until_reading(corrector: *const Corrector) -> i32;
    pub fn aeon_corrector_reading_lag_samples(corrector: *const Corrector) -> i32;
    pub fn aeon_corrector_error_semitones(corrector: *const Corrector) -> f32;
    pub fn aeon_corrector_latency_samples(corrector: *const Corrector) -> i32;

    pub fn aeon_yin_new(rate: f64, window_samples: i32) -> *mut Yin;
    pub fn aeon_yin_free(yin: *mut Yin);
    pub fn aeon_yin_analyze(yin: *mut Yin, window: *const f32) -> Pitch;

    pub fn aeon_stretch_new(channels: i32, rate: f32, preset: i32) -> *mut Stretch;
    pub fn aeon_stretch_free(stretch: *mut Stretch);
    pub fn aeon_stretch_set_transpose_semitones(stretch: *mut Stretch, semitones: f32);
    pub fn aeon_stretch_process(
        stretch: *mut Stretch,
        input: *const *const f32,
        input_frames: i32,
        output: *const *mut f32,
        output_frames: i32,
    );
    pub fn aeon_stretch_reset(stretch: *mut Stretch);
    pub fn aeon_stretch_latency_frames(stretch: *const Stretch) -> i32;
}
