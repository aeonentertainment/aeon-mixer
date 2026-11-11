use std::ptr::null_mut;
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use engine::block::RATE;
use engine::driver::DriverError;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Media::Audio::{
    AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT, IAudioCaptureClient,
    IAudioClient, IAudioClient3, IMMDevice,
};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Threading::WaitForSingleObject;

use super::event::Event;
use super::failed;
use super::shared::initialize_shared;
use crate::capture::CaptureInfo;
use crate::capture_feed::CaptureFeed;
use crate::stats::Stats;

/// A device that has not signalled in this long has stopped: unplugged or wedged.
const STALL_MS: u32 = 1000;

/// An initialised event-driven shared capture stream. Built and read on the capture thread, which
/// owns COM for it. Shared because exclusive would take the mic from every other app, and a shared
/// mix of f32 at 48 kHz converts nothing.
pub struct CaptureStream {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Event,
    channels: usize,
    period: u32,
    stream_latency: u32,
}

impl CaptureStream {
    pub fn open(device: &IMMDevice, asked: u32) -> Result<Self, DriverError> {
        // SAFETY: COM is initialised on this thread, and the event outlives the client here.
        unsafe {
            let client: IAudioClient3 =
                device.Activate(CLSCTX_ALL, None).map_err(failed("activate IAudioClient3"))?;
            let (channels, period) = initialize_shared(&client, asked)?;
            let client: IAudioClient = client.into();
            let event = Event::new()?;
            client.SetEventHandle(event.0).map_err(failed("set the event"))?;
            let capture = client.GetService().map_err(failed("get the capture client"))?;
            let latency = client.GetStreamLatency().map_err(failed("read the stream latency"))?;
            Ok(Self {
                client,
                capture,
                event,
                channels,
                period,
                stream_latency: (latency as f64 * f64::from(RATE) / 10_000_000.0).round() as u32,
            })
        }
    }

    pub fn info(&self) -> CaptureInfo {
        CaptureInfo {
            sample_rate: RATE,
            period_frames: self.period,
            channels: self.channels as u16,
            input_latency_frames: Some(self.stream_latency),
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn describe(&self, device: &str, channel: usize) -> String {
        format!(
            "WASAPI shared capture (IAudioClient3), {device}, f32 x{}, channel {channel}, period {}",
            self.channels, self.period,
        )
    }

    pub fn start(&self) -> Result<(), DriverError> {
        // SAFETY: the client is initialised for event callbacks.
        unsafe { self.client.Start() }.map_err(failed("start capture"))
    }

    /// Reads from a started stream until `stop` is set or the device fails, then stops it.
    pub fn record(
        &self,
        feed: &mut CaptureFeed,
        channel: usize,
        stats: &Stats,
        stop: &AtomicBool,
    ) -> windows::core::Result<()> {
        // Nothing has been lost before anything has been read, whatever the first packet's flag says
        let mut first = true;
        while !stop.load(Ordering::Acquire) {
            // SAFETY: the event is open for the life of `self`.
            if unsafe { WaitForSingleObject(self.event.0, STALL_MS) } != WAIT_OBJECT_0 {
                stats.fail();
                break;
            }
            let at = Instant::now();
            stats.callback();
            self.drain(feed, channel, stats, at, &mut first)?;
        }
        // SAFETY: the client is started.
        unsafe { self.client.Stop() }
    }

    // Every packet waiting, which is one unless the thread ran late.
    fn drain(
        &self,
        feed: &mut CaptureFeed,
        channel: usize,
        stats: &Stats,
        at: Instant,
        first: &mut bool,
    ) -> windows::core::Result<()> {
        loop {
            let (mut data, mut frames, mut flags) = (null_mut(), 0u32, 0u32);
            // SAFETY: every out-pointer is a local. An empty stream is a success with no frames.
            unsafe { self.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)? };
            if frames == 0 {
                return Ok(());
            }
            if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 && !*first {
                stats.underrun();
            }
            *first = false;
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                feed.deliver_silence(frames as usize, at);
            } else {
                // SAFETY: WASAPI lends `frames` interleaved f32 frames until ReleaseBuffer, since
                // `initialize_shared` required an f32 mix.
                let samples = unsafe {
                    slice::from_raw_parts(data.cast::<f32>(), frames as usize * self.channels)
                };
                feed.deliver(samples, self.channels, channel, at);
            }
            // SAFETY: returns the buffer GetBuffer lent above, whole.
            unsafe { self.capture.ReleaseBuffer(frames)? };
        }
    }
}
