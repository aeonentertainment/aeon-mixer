//! libasound, opened at run time. Linking it would stop the addon loading on a Linux with no
//! ALSA, where an offline engine still has to run, and would need its headers to build.

use std::ffi::{CStr, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::sync::OnceLock;

use engine::driver::DriverError;

#[repr(C)]
pub struct RawPcm {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct HwParams {
    _opaque: [u8; 0],
}

#[repr(C)]
pub struct SwParams {
    _opaque: [u8; 0],
}

// From alsa/pcm.h
pub const STREAM_PLAYBACK: c_int = 0;
pub const STREAM_CAPTURE: c_int = 1;
pub const ACCESS_RW_INTERLEAVED: c_int = 3;

macro_rules! asound {
    ($(fn $name:ident($($ty:ty),* $(,)?) $(-> $ret:ty)?;)*) => {
        /// The libasound functions the backend calls, each by its C name.
        pub struct Asound {
            $(pub $name: unsafe extern "C" fn($($ty),*) $(-> $ret)?,)*
        }

        impl Asound {
            fn load() -> Result<Self, String> {
                // SAFETY: a constant library name. The handle is never closed, so every
                // function pointer taken from it stays valid for the life of the process.
                let lib = unsafe {
                    libc::dlopen(c"libasound.so.2".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL)
                };
                if lib.is_null() {
                    return Err(dl_error());
                }
                Ok(Self {
                    $($name: {
                        let name = concat!(stringify!($name), "\0");
                        // SAFETY: `lib` is open and `name` is nul-terminated.
                        let symbol = unsafe { libc::dlsym(lib, name.as_ptr().cast()) };
                        if symbol.is_null() {
                            return Err(dl_error());
                        }
                        // SAFETY: the symbol is the libasound function of this name, whose C
                        // signature in alsa/pcm.h is the one this field declares.
                        unsafe {
                            std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($ty),*) $(-> $ret)?>(symbol)
                        }
                    },)*
                })
            }
        }
    };
}

asound! {
    fn snd_strerror(c_int) -> *const c_char;
    fn snd_pcm_open(*mut *mut RawPcm, *const c_char, c_int, c_int) -> c_int;
    fn snd_pcm_close(*mut RawPcm) -> c_int;
    fn snd_pcm_hw_params_malloc(*mut *mut HwParams) -> c_int;
    fn snd_pcm_hw_params_free(*mut HwParams);
    fn snd_pcm_hw_params_any(*mut RawPcm, *mut HwParams) -> c_int;
    fn snd_pcm_hw_params_set_access(*mut RawPcm, *mut HwParams, c_int) -> c_int;
    fn snd_pcm_hw_params_set_rate_resample(*mut RawPcm, *mut HwParams, c_uint) -> c_int;
    fn snd_pcm_hw_params_test_format(*mut RawPcm, *mut HwParams, c_int) -> c_int;
    fn snd_pcm_hw_params_set_format(*mut RawPcm, *mut HwParams, c_int) -> c_int;
    fn snd_pcm_hw_params_test_channels(*mut RawPcm, *mut HwParams, c_uint) -> c_int;
    fn snd_pcm_hw_params_get_channels_min(*const HwParams, *mut c_uint) -> c_int;
    fn snd_pcm_hw_params_set_channels(*mut RawPcm, *mut HwParams, c_uint) -> c_int;
    fn snd_pcm_hw_params_set_rate(*mut RawPcm, *mut HwParams, c_uint, c_int) -> c_int;
    fn snd_pcm_hw_params_set_period_size_near(*mut RawPcm, *mut HwParams, *mut c_ulong, *mut c_int) -> c_int;
    fn snd_pcm_hw_params_set_periods_near(*mut RawPcm, *mut HwParams, *mut c_uint, *mut c_int) -> c_int;
    fn snd_pcm_hw_params(*mut RawPcm, *mut HwParams) -> c_int;
    fn snd_pcm_hw_params_get_period_size(*const HwParams, *mut c_ulong, *mut c_int) -> c_int;
    fn snd_pcm_hw_params_get_buffer_size(*const HwParams, *mut c_ulong) -> c_int;
    fn snd_pcm_sw_params_malloc(*mut *mut SwParams) -> c_int;
    fn snd_pcm_sw_params_free(*mut SwParams);
    fn snd_pcm_sw_params_current(*mut RawPcm, *mut SwParams) -> c_int;
    fn snd_pcm_sw_params_set_start_threshold(*mut RawPcm, *mut SwParams, c_ulong) -> c_int;
    fn snd_pcm_sw_params_set_avail_min(*mut RawPcm, *mut SwParams, c_ulong) -> c_int;
    fn snd_pcm_sw_params(*mut RawPcm, *mut SwParams) -> c_int;
    fn snd_pcm_wait(*mut RawPcm, c_int) -> c_int;
    fn snd_pcm_writei(*mut RawPcm, *const c_void, c_ulong) -> c_long;
    fn snd_pcm_readi(*mut RawPcm, *mut c_void, c_ulong) -> c_long;
    fn snd_pcm_start(*mut RawPcm) -> c_int;
    fn snd_pcm_recover(*mut RawPcm, c_int, c_int) -> c_int;
    fn snd_device_name_hint(c_int, *const c_char, *mut *mut *mut c_void) -> c_int;
    fn snd_device_name_get_hint(*const c_void, *const c_char) -> *mut c_char;
    fn snd_device_name_free_hint(*mut *mut c_void) -> c_int;
}

impl Asound {
    /// `code` if ALSA calls it success, else what failed and ALSA's words for why.
    pub fn check(&self, code: c_int, doing: &str) -> Result<c_int, DriverError> {
        if code >= 0 {
            return Ok(code);
        }
        // SAFETY: snd_strerror returns a static string for any code.
        let reason = unsafe { CStr::from_ptr((self.snd_strerror)(code)) }.to_string_lossy();
        Err(DriverError::Backend(format!("ALSA could not {doing}: {reason}")))
    }
}

/// libasound, loaded on first use.
pub fn asound() -> Result<&'static Asound, DriverError> {
    static ASOUND: OnceLock<Result<Asound, String>> = OnceLock::new();
    ASOUND
        .get_or_init(Asound::load)
        .as_ref()
        .map_err(|reason| DriverError::Backend(format!("could not load libasound: {reason}")))
}

fn dl_error() -> String {
    // SAFETY: dlerror returns null or a nul-terminated string valid until the next dl call.
    let reason = unsafe { libc::dlerror() };
    if reason.is_null() {
        return "unknown dlopen error".into();
    }
    unsafe { CStr::from_ptr(reason) }.to_string_lossy().into_owned()
}
