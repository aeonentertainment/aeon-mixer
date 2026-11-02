use std::ptr::null_mut;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use engine::block::RATE;
use engine::driver::{DriverError, StreamInfo};
use windows::Win32::Foundation::{S_OK, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::{
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_SHAREMODE_EXCLUSIVE,
    AUDCLNT_STREAMFLAGS_EVENTCALLBACK, IAudioClient, IAudioClient3, IAudioRenderClient, IMMDevice,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoTaskMemFree};
use windows::Win32::System::Threading::WaitForSingleObject;

use super::event::Event;
use super::failed;
use super::format::{Format, is_float_48k};
use super::mode::WasapiMode;
use crate::feed::Feed;
use crate::sample::Sample;
use crate::stats::Stats;

/// A device that has not signalled in this long has stopped: unplugged or wedged.
const STALL_MS: u32 = 1000;

/// An initialised event-driven render stream, shared or exclusive. Built and played on the
/// audio thread, which owns COM for it.
pub struct Stream {
    client: IAudioClient,
    render: IAudioRenderClient,
    event: Event,
    shared: bool,
    format: Format,
    channels: usize,
    period: u32,
    buffer: u32,
    stream_latency: u32,
}

impl Stream {
    pub fn open(device: &IMMDevice, asked: u32, mode: WasapiMode) -> Result<Self, DriverError> {
        match mode {
            WasapiMode::Exclusive => Self::exclusive(device, asked),
            WasapiMode::Shared => Self::shared(device, asked, true),
            // A room with a slow shared engine and exclusive mode turned off still gets sound
            WasapiMode::Auto => Self::shared(device, asked, false)
                .or_else(|_| Self::exclusive(device, asked))
                .or_else(|exclusive| Self::shared(device, asked, true).map_err(|_| exclusive)),
        }
    }

    pub fn info(&self) -> StreamInfo {
        StreamInfo {
            sample_rate: RATE,
            period_frames: self.period,
            output_latency_frames: self.most_queued() + self.stream_latency,
        }
    }

    pub fn describe(&self, device: &str) -> String {
        let path = if self.shared { "shared (IAudioClient3)" } else { "exclusive" };
        format!(
            "WASAPI {path}, {device}, {} x{}, period {}, buffer {}",
            self.format.name(),
            self.channels,
            self.period,
            self.buffer,
        )
    }

    /// Plays until `stop` is set or the device fails.
    pub fn play(
        &self,
        feed: &mut Feed,
        stats: &Stats,
        stop: &AtomicBool,
    ) -> Result<(), DriverError> {
        match self.format {
            Format::F32 => self.play_as::<f32>(feed, stats, stop),
            Format::I32 | Format::I24In32 => self.play_as::<i32>(feed, stats, stop),
            Format::I16 => self.play_as::<i16>(feed, stats, stop),
        }
        .map_err(failed("keep playing"))
    }

    // IAudioClient3 at the smallest period the shared engine allows at or above the asked one.
    // Unless `forced`, a shared engine whose smallest period is longer than the asked one cannot
    // reach the budget, and that is an error so the caller falls back to exclusive.
    fn shared(device: &IMMDevice, asked: u32, forced: bool) -> Result<Self, DriverError> {
        // SAFETY: COM is initialised on this thread; every out-pointer below is a local.
        unsafe {
            let client: IAudioClient3 =
                device.Activate(CLSCTX_ALL, None).map_err(failed("activate IAudioClient3"))?;
            let mix = client.GetMixFormat().map_err(failed("read the mix format"))?;
            let started = (|| {
                if !is_float_48k(mix) {
                    return Err(DriverError::Backend("the shared mix is not f32 at 48 kHz".into()));
                }
                let (mut default, mut fundamental, mut min, mut max) = (0, 0, 0, 0);
                client
                    .GetSharedModeEnginePeriod(
                        mix,
                        &mut default,
                        &mut fundamental,
                        &mut min,
                        &mut max,
                    )
                    .map_err(failed("read the shared engine's periods"))?;
                if min > asked && !forced {
                    return Err(DriverError::Backend(format!(
                        "the shared engine's shortest period is {min} frames"
                    )));
                }
                let step = fundamental.max(1);
                let period = (asked.div_ceil(step) * step).clamp(min, max);
                client
                    .InitializeSharedAudioStream(
                        AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                        period,
                        mix,
                        None,
                    )
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
            Self::finish(client.into(), true, Format::F32, channels, period)
        }
    }

    // Exclusive and event-driven: the device double-buffers one period each side of the flip.
    fn exclusive(device: &IMMDevice, asked: u32) -> Result<Self, DriverError> {
        // SAFETY: COM is initialised on this thread; every pointer below is to a local.
        unsafe {
            let activate = || -> Result<IAudioClient, DriverError> {
                device.Activate(CLSCTX_ALL, None).map_err(failed("activate IAudioClient"))
            };
            let mut client = activate()?;
            let format = Format::PREFERRED
                .into_iter()
                .find(|format| {
                    let wave = format.wave();
                    client.IsFormatSupported(
                        AUDCLNT_SHAREMODE_EXCLUSIVE,
                        (&raw const wave).cast(),
                        None,
                    ) == S_OK
                })
                .ok_or_else(|| {
                    DriverError::Backend(
                        "the device takes no stereo 48 kHz format exclusively".into(),
                    )
                })?;
            let wave = format.wave();
            let initialize = |client: &IAudioClient, duration: i64| {
                client.Initialize(
                    AUDCLNT_SHAREMODE_EXCLUSIVE,
                    AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                    duration,
                    duration,
                    (&raw const wave).cast(),
                    None,
                )
            };

            let mut minimum = 0;
            client
                .GetDevicePeriod(None, Some(&mut minimum))
                .map_err(failed("read the device period"))?;
            if let Err(error) = initialize(&client, frames_to_hns(asked).max(minimum)) {
                if error.code() != AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED {
                    return Err(failed("open the device exclusively")(error));
                }
                // The device offers the nearest buffer it can align; a fresh client asks for that
                let frames = client.GetBufferSize().map_err(failed("read the aligned buffer"))?;
                client = activate()?;
                initialize(&client, frames_to_hns(frames))
                    .map_err(failed("open the device exclusively"))?;
            }
            let buffer = client.GetBufferSize().map_err(failed("read the buffer size"))?;
            Self::finish(client, false, format, 2, buffer)
        }
    }

    fn finish(
        client: IAudioClient,
        shared: bool,
        format: Format,
        channels: usize,
        period: u32,
    ) -> Result<Self, DriverError> {
        let event = Event::new()?;
        // SAFETY: the client is initialised for event callbacks and the event outlives it here.
        unsafe {
            client.SetEventHandle(event.0).map_err(failed("set the event"))?;
            let render = client.GetService().map_err(failed("get the render client"))?;
            let buffer = client.GetBufferSize().map_err(failed("read the buffer size"))?;
            let latency = client.GetStreamLatency().map_err(failed("read the stream latency"))?;
            Ok(Self {
                client,
                render,
                event,
                shared,
                format,
                channels,
                period,
                buffer,
                stream_latency: hns_to_frames(latency),
            })
        }
    }

    // Shared mode keeps two periods queued, one being mixed and one ahead, however large the
    // buffer is; exclusive mode has one period each side of the flip.
    fn most_queued(&self) -> u32 {
        if self.shared { self.buffer.min(2 * self.period) } else { 2 * self.buffer }
    }

    fn play_as<S: Sample>(
        &self,
        feed: &mut Feed,
        stats: &Stats,
        stop: &AtomicBool,
    ) -> windows::core::Result<()> {
        let target = if self.shared { self.most_queued() } else { self.buffer };
        // A wake this late in exclusive mode missed the flip: the device replayed or went silent.
        let late = Duration::from_secs_f64(1.5 * f64::from(self.period) / f64::from(RATE));
        self.write::<S>(feed, target)?;
        // SAFETY: the client is initialised and its first buffer written.
        unsafe { self.client.Start()? };
        while !stop.load(Ordering::Acquire) {
            // SAFETY: the event is open for the life of `self`.
            if unsafe { WaitForSingleObject(self.event.0, STALL_MS) } != WAIT_OBJECT_0 {
                stats.fail();
                break;
            }
            let interval = stats.callback();
            let frames = if self.shared {
                // SAFETY: the client is started.
                let queued = unsafe { self.client.GetCurrentPadding()? };
                if queued == 0 {
                    stats.underrun();
                }
                target.saturating_sub(queued)
            } else {
                if interval.is_some_and(|interval| interval > late) {
                    stats.underrun();
                }
                self.buffer
            };
            self.write::<S>(feed, frames)?;
        }
        // SAFETY: the client is started.
        unsafe { self.client.Stop() }
    }

    fn write<S: Sample>(&self, feed: &mut Feed, frames: u32) -> windows::core::Result<()> {
        if frames == 0 {
            return Ok(());
        }
        // SAFETY: WASAPI lends `frames` interleaved frames of this stream's format until
        // ReleaseBuffer, aligned for its sample type.
        unsafe {
            let data = self.render.GetBuffer(frames)?;
            let samples =
                slice::from_raw_parts_mut(data.cast::<S>(), frames as usize * self.channels);
            feed.fill(samples, self.channels);
            self.render.ReleaseBuffer(frames, 0)
        }
    }
}

// WASAPI counts time in 100 ns units.
fn frames_to_hns(frames: u32) -> i64 {
    (10_000_000.0 * f64::from(frames) / f64::from(RATE)).round() as i64
}

fn hns_to_frames(hns: i64) -> u32 {
    (hns as f64 * f64::from(RATE) / 10_000_000.0).round() as u32
}
