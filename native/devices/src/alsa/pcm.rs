use std::ffi::{CString, c_int, c_long, c_uint, c_ulong};
use std::ptr::null_mut;

use engine::block::RATE;
use engine::driver::DriverError;

use super::asound::{
    ACCESS_RW_INTERLEAVED, Asound, HwParams, RawPcm, STREAM_CAPTURE, STREAM_PLAYBACK, SwParams,
};
use super::direction::Direction;
use super::format::Format;
use crate::channel_count::channel_count;
use crate::sample::Sample;

/// The periods in a playback ring: one playing while the next is written.
const PERIODS: c_uint = 2;

/// The periods in a capture ring. Filled frames are read as they land, so a longer ring adds no
/// delay and rides out a late thread.
const CAPTURE_PERIODS: c_uint = 4;

/// An ALSA handle on a hardware device, at 48 kHz with an explicit period. It is opened and
/// configured on the control thread and then used by the audio thread alone. Closed on drop.
pub struct Pcm {
    lib: &'static Asound,
    raw: *mut RawPcm,
    direction: Direction,
    pub format: Format,
    pub channels: u32,
    pub period: u32,
    pub buffer: u32,
}

// SAFETY: an ALSA handle is not tied to the thread that opened it; one thread at a time uses it.
unsafe impl Send for Pcm {}

impl Pcm {
    /// Opens `device` with at least `least_channels`, so that capture can reach the channel it
    /// reads.
    pub fn open(
        lib: &'static Asound,
        device: &str,
        period: u32,
        least_channels: u32,
        direction: Direction,
    ) -> Result<Self, DriverError> {
        let name = CString::new(device).map_err(|_| DriverError::NoDevice)?;
        let mut raw = null_mut();
        let stream = match direction {
            Direction::Playback => STREAM_PLAYBACK,
            Direction::Capture => STREAM_CAPTURE,
        };
        // SAFETY: `raw` receives the handle and `name` is nul-terminated.
        let opened = unsafe { (lib.snd_pcm_open)(&mut raw, name.as_ptr(), stream, 0) };
        if opened == -libc::EBUSY {
            return Err(DriverError::Backend(format!(
                "{device} is busy: another program, or PipeWire, has it open"
            )));
        }
        lib.check(opened, &format!("open {device}"))?;
        let mut pcm =
            Self { lib, raw, direction, format: Format::F32, channels: 2, period, buffer: 0 };
        pcm.configure(device, least_channels)?;
        Ok(pcm)
    }

    /// Blocks until a period of the ring is free, the stream fails, or `timeout_ms` passes.
    /// Returns 1, a negative error code, or 0.
    pub fn wait(&self, timeout_ms: c_int) -> c_int {
        // SAFETY: `raw` is open for the life of `self`.
        unsafe { (self.lib.snd_pcm_wait)(self.raw, timeout_ms) }
    }

    /// Writes whole interleaved frames and returns how many went in, or a negative error code.
    pub fn write<S: Sample>(&self, samples: &[S]) -> c_long {
        let frames = (samples.len() / self.channels as usize) as c_ulong;
        // SAFETY: `samples` holds `frames` frames in the format and channel count configured.
        unsafe { (self.lib.snd_pcm_writei)(self.raw, samples.as_ptr().cast(), frames) }
    }

    /// Reads whole interleaved frames and returns how many came, or a negative error code.
    pub fn read<S: Sample>(&self, samples: &mut [S]) -> c_long {
        let frames = (samples.len() / self.channels as usize) as c_ulong;
        // SAFETY: `samples` has room for `frames` frames in the format and channel count configured.
        unsafe { (self.lib.snd_pcm_readi)(self.raw, samples.as_mut_ptr().cast(), frames) }
    }

    /// Starts a prepared capture stream, which unlike playback has nothing to start it. Negative on
    /// failure, and -EBADFD for a stream that is not prepared because it is already running.
    pub fn start(&self) -> c_int {
        // SAFETY: `raw` is open for the life of `self`.
        unsafe { (self.lib.snd_pcm_start)(self.raw) }
    }

    /// Brings the stream back after an underrun or a suspend. Negative if it cannot.
    pub fn recover(&self, error: c_int) -> c_int {
        // SAFETY: `raw` is open; silent so ALSA prints nothing.
        unsafe { (self.lib.snd_pcm_recover)(self.raw, error, 1) }
    }

    fn configure(&mut self, device: &str, least_channels: u32) -> Result<(), DriverError> {
        let lib = self.lib;
        let mut hw = null_mut();
        // SAFETY: `hw` receives an allocation freed below.
        lib.check(unsafe { (lib.snd_pcm_hw_params_malloc)(&mut hw) }, "allocate hw params")?;
        let applied = self.apply_hw(hw, device, least_channels);
        // SAFETY: allocated above and no longer used.
        unsafe { (lib.snd_pcm_hw_params_free)(hw) };
        applied?;

        let mut sw = null_mut();
        // SAFETY: `sw` receives an allocation freed below.
        lib.check(unsafe { (lib.snd_pcm_sw_params_malloc)(&mut sw) }, "allocate sw params")?;
        let applied = self.apply_sw(sw);
        // SAFETY: allocated above and no longer used.
        unsafe { (lib.snd_pcm_sw_params_free)(sw) };
        applied
    }

    // Interleaved, no resampling, the best format the hardware takes, the channels `channel_count`
    // picks, 48 kHz, then the period nearest the one asked for.
    fn apply_hw(
        &mut self,
        hw: *mut HwParams,
        device: &str,
        least_channels: u32,
    ) -> Result<(), DriverError> {
        let (lib, raw) = (self.lib, self.raw);
        // SAFETY: `raw` is open and `hw` is allocated for every call below.
        unsafe {
            lib.check((lib.snd_pcm_hw_params_any)(raw, hw), "read the hardware's ranges")?;
            lib.check(
                (lib.snd_pcm_hw_params_set_access)(raw, hw, ACCESS_RW_INTERLEAVED),
                "set interleaved access",
            )?;
            lib.check(
                (lib.snd_pcm_hw_params_set_rate_resample)(raw, hw, 0),
                "turn off resampling",
            )?;

            self.format = Format::PREFERRED
                .into_iter()
                .find(|format| (lib.snd_pcm_hw_params_test_format)(raw, hw, format.code()) == 0)
                .ok_or_else(|| {
                    DriverError::Backend("the device takes no f32, s32 or s16".into())
                })?;
            lib.check(
                (lib.snd_pcm_hw_params_set_format)(raw, hw, self.format.code()),
                "set the format",
            )?;

            let (mut min, mut max): (c_uint, c_uint) = (0, 0);
            lib.check(
                (lib.snd_pcm_hw_params_get_channels_min)(hw, &mut min),
                "read the fewest channels",
            )?;
            lib.check(
                (lib.snd_pcm_hw_params_get_channels_max)(hw, &mut max),
                "read the most channels",
            )?;
            let channels = channel_count(least_channels, min..=max, |count| {
                (lib.snd_pcm_hw_params_test_channels)(raw, hw, count) == 0
            })
            .ok_or_else(|| {
                DriverError::Backend(format!(
                    "{device} opens at {min} to {max} channels, none of them {least_channels} or more"
                ))
            })?;
            lib.check((lib.snd_pcm_hw_params_set_channels)(raw, hw, channels), "set channels")?;
            self.channels = channels;

            lib.check((lib.snd_pcm_hw_params_set_rate)(raw, hw, RATE, 0), "run at 48000 Hz")?;

            let mut period = c_ulong::from(self.period);
            let mut dir = 0;
            lib.check(
                (lib.snd_pcm_hw_params_set_period_size_near)(raw, hw, &mut period, &mut dir),
                "set the period",
            )?;
            let mut periods = match self.direction {
                Direction::Playback => PERIODS,
                Direction::Capture => CAPTURE_PERIODS,
            };
            lib.check(
                (lib.snd_pcm_hw_params_set_periods_near)(raw, hw, &mut periods, &mut dir),
                "set the period count",
            )?;
            lib.check((lib.snd_pcm_hw_params)(raw, hw), "apply hw params")?;

            let mut buffer: c_ulong = 0;
            lib.check(
                (lib.snd_pcm_hw_params_get_period_size)(hw, &mut period, &mut dir),
                "read the period",
            )?;
            lib.check((lib.snd_pcm_hw_params_get_buffer_size)(hw, &mut buffer), "read the buffer")?;
            self.period = period as u32;
            self.buffer = buffer as u32;
        }
        Ok(())
    }

    // Playback starts itself once the ring is full; capture starts as soon as it is read. Either
    // wakes its thread a period at a time.
    fn apply_sw(&self, sw: *mut SwParams) -> Result<(), DriverError> {
        let (lib, raw) = (self.lib, self.raw);
        let start_at = match self.direction {
            Direction::Playback => self.buffer.into(),
            Direction::Capture => 1,
        };
        // SAFETY: `raw` is open and `sw` is allocated for every call below.
        unsafe {
            lib.check((lib.snd_pcm_sw_params_current)(raw, sw), "read sw params")?;
            lib.check(
                (lib.snd_pcm_sw_params_set_start_threshold)(raw, sw, start_at),
                "set the start threshold",
            )?;
            lib.check(
                (lib.snd_pcm_sw_params_set_avail_min)(raw, sw, self.period.into()),
                "set the wake-up size",
            )?;
            lib.check((lib.snd_pcm_sw_params)(raw, sw), "apply sw params")?;
        }
        Ok(())
    }
}

impl Drop for Pcm {
    fn drop(&mut self) {
        // SAFETY: opened in `open` and closed only here. Closing stops the stream.
        unsafe { (self.lib.snd_pcm_close)(self.raw) };
    }
}
