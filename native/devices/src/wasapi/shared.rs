use std::ptr::null_mut;

use engine::driver::DriverError;
use windows::Win32::Media::Audio::{AUDCLNT_STREAMFLAGS_EVENTCALLBACK, IAudioClient3};
use windows::Win32::System::Com::CoTaskMemFree;

use super::failed;
use super::format::is_float_48k;

/// Initialises a shared, event-driven stream on an endpoint of either direction, at the smallest
/// period the shared engine allows at or above the asked one, and returns its channels and the
/// period the engine runs at. The engine never resamples, so a mix format other than f32 at 48 kHz
/// is an error. COM must be initialised on the calling thread.
pub fn initialize_shared(client: &IAudioClient3, asked: u32) -> Result<(usize, u32), DriverError> {
    // SAFETY: COM is initialised on this thread; every out-pointer below is a local.
    unsafe {
        let mix = client.GetMixFormat().map_err(failed("read the mix format"))?;
        let started = (|| {
            if !is_float_48k(mix) {
                return Err(DriverError::Backend("the shared mix is not f32 at 48 kHz".into()));
            }
            let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
            client
                .GetSharedModeEnginePeriod(mix, &mut default, &mut fundamental, &mut min, &mut max)
                .map_err(failed("read the shared engine's periods"))?;
            let step = fundamental.max(1);
            let period = (asked.div_ceil(step) * step).clamp(min, max);
            client
                .InitializeSharedAudioStream(AUDCLNT_STREAMFLAGS_EVENTCALLBACK, period, mix, None)
                .map_err(failed("start a shared low-latency stream"))?;
            Ok(usize::from(mix.read_unaligned().nChannels))
        })();
        CoTaskMemFree(Some(mix.cast_const().cast()));
        let channels = started?;

        // Another app may already hold the engine at a period of its own
        let (mut format, mut period) = (null_mut(), 0);
        client
            .GetCurrentSharedModeEnginePeriod(&mut format, &mut period)
            .map_err(failed("read the shared engine's period"))?;
        CoTaskMemFree(Some(format.cast_const().cast()));
        Ok((channels, period))
    }
}
