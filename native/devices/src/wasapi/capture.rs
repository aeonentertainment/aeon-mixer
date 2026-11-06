use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use engine::driver::DriverError;
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::core::w;

use super::capture_stream::CaptureStream;
use super::com::Com;
use crate::capture::{Capture, CaptureInfo, Capturing, Sink};
use crate::capture_feed::CaptureFeed;
use crate::capture_request::CaptureRequest;
use crate::stats::Stats;

/// WASAPI capture, shared and event-driven. The capture thread is ours, registered with MMCSS as
/// Pro Audio; it opens the device too, so COM lives on one thread from open to close.
pub struct WasapiCapture {
    request: CaptureRequest,
    stats: Arc<Stats>,
}

impl WasapiCapture {
    pub fn new(request: CaptureRequest, stats: Arc<Stats>) -> Self {
        Self { request, stats }
    }
}

impl Capture for WasapiCapture {
    /// Starts the capture thread and blocks until it has opened the device or failed to.
    fn start(self: Box<Self>, sink: Sink) -> Result<Capturing, DriverError> {
        let Self { request, stats } = *self;
        let (opened, open) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let feed = CaptureFeed::new(sink, Arc::clone(&stats));
        let audio = thread::Builder::new()
            .name("aeon-capture".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || run(&request, feed, &stats, &stop, &opened)
            })
            .map_err(|error| DriverError::Backend(format!("capture thread: {error}")))?;

        match open.recv() {
            Ok(Ok(info)) => Ok(Capturing::new(info, move || {
                stop.store(true, Ordering::Release);
                // The feed comes back so the sink drops here and not on the capture thread
                drop(audio.join());
            })),
            Ok(Err(error)) => {
                drop(audio.join());
                Err(error)
            }
            Err(_) => Err(DriverError::Backend(
                "the capture thread ended before the device opened".into(),
            )),
        }
    }
}

fn run(
    request: &CaptureRequest,
    mut feed: CaptureFeed,
    stats: &Stats,
    stop: &AtomicBool,
    opened: &mpsc::SyncSender<Result<CaptureInfo, DriverError>>,
) -> CaptureFeed {
    let channel = usize::from(request.channel);
    let stream = Com::init().and_then(|com| {
        let device = super::input_device(request.device.as_deref())?;
        let stream = CaptureStream::open(&device, request.period_frames)?;
        if channel >= stream.channels() {
            return Err(DriverError::Backend(format!(
                "the input has {} channels, so no channel {channel}",
                stream.channels()
            )));
        }
        stats.set_opened(stream.describe(&super::name(&device).unwrap_or_default(), channel));
        Ok((com, stream))
    });
    let (_com, stream) = match stream {
        Ok(opened_stream) => opened_stream,
        Err(error) => {
            let _ = opened.send(Err(error));
            return feed;
        }
    };
    let _ = opened.send(Ok(stream.info()));

    let mut task = 0;
    // SAFETY: registers this thread with MMCSS; reverted below on the same thread.
    let mmcss = unsafe { AvSetMmThreadCharacteristicsW(w!("Pro Audio"), &mut task) };
    stats.set_realtime(mmcss.is_ok());
    if stream.record(&mut feed, channel, stats, stop).is_err() {
        stats.fail();
    }
    if let Ok(handle) = mmcss {
        // SAFETY: the handle came from AvSetMmThreadCharacteristicsW on this thread.
        let _ = unsafe { AvRevertMmThreadCharacteristics(handle) };
    }
    // The stream's COM objects go before COM does
    drop(stream);
    feed
}
