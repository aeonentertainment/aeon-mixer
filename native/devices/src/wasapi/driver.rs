use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW,
};
use windows::core::w;

use super::com::Com;
use super::mode::WasapiMode;
use super::stream::Stream;
use crate::feed::Feed;
use crate::request::Request;
use crate::stats::Stats;

/// WASAPI, event-driven. `Auto` opens IAudioClient3's shared stream at the nearest period the
/// shared engine allows, and opens exclusive only when shared cannot open at all. Exclusive
/// silences every other app on the device, and on the laptop decision 0003 measured it ran in
/// bursts; it stays available on request. The audio thread is ours, registered with MMCSS as
/// Pro Audio; it opens the device too, so COM lives on one thread from open to close.
pub struct WasapiDriver {
    request: Request,
    mode: WasapiMode,
    stats: Arc<Stats>,
}

impl WasapiDriver {
    pub fn new(request: Request, mode: WasapiMode, stats: Arc<Stats>) -> Self {
        Self { request, mode, stats }
    }
}

impl Driver for WasapiDriver {
    /// Starts the audio thread and blocks until it has opened the device or failed to.
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let Self { request, mode, stats } = *self;
        let (opened, open) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let feed = Feed::new(render, Arc::clone(&stats));
        let audio = thread::Builder::new()
            .name("aeon-audio".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || run(&request, mode, feed, &stats, &stop, &opened)
            })
            .map_err(|error| DriverError::Backend(format!("audio thread: {error}")))?;

        match open.recv() {
            Ok(Ok(info)) => Ok(Running::new(info, move || {
                stop.store(true, Ordering::Release);
                // The feed comes back so render drops here and not on the audio thread
                drop(audio.join());
            })),
            Ok(Err(error)) => {
                drop(audio.join());
                Err(error)
            }
            Err(_) => {
                Err(DriverError::Backend("the audio thread ended before the device opened".into()))
            }
        }
    }
}

fn run(
    request: &Request,
    mode: WasapiMode,
    mut feed: Feed,
    stats: &Stats,
    stop: &AtomicBool,
    opened: &mpsc::SyncSender<Result<StreamInfo, DriverError>>,
) -> Feed {
    let com = Com::init();
    let stream = com.and_then(|com| {
        let device = super::device(request.device.as_deref())?;
        let stream = Stream::open(&device, request.period_frames, mode)?;
        stats.set_opened(stream.describe(&super::name(&device).unwrap_or_default()));
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
    if stream.play(&mut feed, stats, stop).is_err() {
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
