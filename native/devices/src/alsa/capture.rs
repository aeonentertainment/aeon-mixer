use std::ffi::c_int;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Instant;

use engine::block::RATE;
use engine::driver::DriverError;

use super::asound::asound;
use super::driver::ask_for_realtime;
use super::format::Format;
use super::pcm::{Direction, Pcm};
use crate::capture::{Capture, CaptureInfo, Capturing, Sink};
use crate::capture_feed::CaptureFeed;
use crate::capture_request::CaptureRequest;
use crate::sample::Sample;
use crate::stats::Stats;

/// A device that has not woken the thread in this long has stopped: unplugged or wedged.
const STALL_MS: c_int = 1000;

/// ALSA straight onto the hardware device, with no dsnoop, plug or sound server between. ALSA has
/// no callback: the capture thread is ours, and it waits for a period, reads it and hands it over.
pub struct AlsaCapture {
    request: CaptureRequest,
    stats: Arc<Stats>,
}

impl AlsaCapture {
    pub fn new(request: CaptureRequest, stats: Arc<Stats>) -> Self {
        Self { request, stats }
    }
}

impl Capture for AlsaCapture {
    /// Opens and configures the device on the calling thread, which blocks while ALSA does, then
    /// starts the capture thread.
    fn start(self: Box<Self>, sink: Sink) -> Result<Capturing, DriverError> {
        let Self { request, stats } = *self;
        let lib = asound()?;
        let device = match request.device {
            Some(device) => device,
            None => super::default_input()?,
        };
        let pcm = Pcm::open(lib, &device, request.period_frames, Direction::Capture)?;
        if u32::from(request.channel) >= pcm.channels {
            return Err(DriverError::Backend(format!(
                "{device} has {} channels, so no channel {}",
                pcm.channels, request.channel
            )));
        }
        let info = CaptureInfo {
            sample_rate: RATE,
            period_frames: pcm.period,
            channels: pcm.channels as u16,
            input_latency_frames: None,
        };
        stats.set_opened(format!(
            "ALSA {device}, {} x{}, channel {}, {} frames in periods of {}",
            pcm.format.name(),
            pcm.channels,
            request.channel,
            pcm.buffer,
            pcm.period,
        ));

        let stop = Arc::new(AtomicBool::new(false));
        let feed = CaptureFeed::new(sink, Arc::clone(&stats));
        let channel = usize::from(request.channel);
        let audio = thread::Builder::new()
            .name("aeon-capture".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || run(pcm, feed, channel, &stats, &stop)
            })
            .map_err(|error| DriverError::Backend(format!("capture thread: {error}")))?;

        Ok(Capturing::new(info, move || {
            stop.store(true, Ordering::Release);
            // The feed comes back so the sink drops here and not on the capture thread
            drop(audio.join());
        }))
    }
}

fn run(
    pcm: Pcm,
    mut feed: CaptureFeed,
    channel: usize,
    stats: &Stats,
    stop: &AtomicBool,
) -> CaptureFeed {
    ask_for_realtime(stats);
    if pcm.start() < 0 {
        stats.fail();
        return feed;
    }
    match pcm.format {
        Format::F32 => record::<f32>(&pcm, &mut feed, channel, stats, stop),
        Format::I32 => record::<i32>(&pcm, &mut feed, channel, stats, stop),
        Format::I16 => record::<i16>(&pcm, &mut feed, channel, stats, stop),
    }
    feed
}

fn record<S: Sample>(
    pcm: &Pcm,
    feed: &mut CaptureFeed,
    channel: usize,
    stats: &Stats,
    stop: &AtomicBool,
) {
    let channels = pcm.channels as usize;
    let mut period = vec![S::default(); pcm.period as usize * channels];
    while !stop.load(Ordering::Acquire) {
        let ready = pcm.wait(STALL_MS);
        let at = Instant::now();
        if ready == 0 {
            stats.fail();
            return;
        }
        if ready < 0 {
            if !recover(pcm, ready, stats) {
                return;
            }
            continue;
        }
        stats.callback();
        let frames = pcm.read(&mut period);
        if frames < 0 {
            if !recover(pcm, frames as c_int, stats) {
                return;
            }
            continue;
        }
        feed.deliver(&period[..frames as usize * channels], channels, channel, at);
    }
}

// An overrun comes back as EPIPE: the device lost frames because nobody took them in time.
// `snd_pcm_recover` leaves that stream prepared, and a prepared capture stream stays put until a read
// or `snd_pcm_start` starts it, so the next wait would only time out. A stream that a resume brought
// back is already running, and `snd_pcm_start` refuses it with EBADFD, which is the good answer.
fn recover(pcm: &Pcm, error: c_int, stats: &Stats) -> bool {
    if error == -libc::EPIPE {
        stats.underrun();
    }
    let recovered = pcm.recover(error) >= 0 && {
        let started = pcm.start();
        started >= 0 || started == -libc::EBADFD
    };
    if !recovered {
        stats.fail();
    }
    recovered
}
