use std::ffi::c_int;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use engine::block::RATE;
use engine::driver::{Driver, DriverError, Render, Running, StreamInfo};

use super::asound::asound;
use super::direction::Direction;
use super::format::Format;
use super::pcm::Pcm;
use crate::feed::Feed;
use crate::request::Request;
use crate::sample::Sample;
use crate::stats::Stats;

/// A device that has not woken the thread in this long has stopped: unplugged or wedged.
const STALL_MS: c_int = 1000;

/// SCHED_FIFO priority for the audio thread: above every desktop thread, below the kernel's own.
const PRIORITY: c_int = 70;

/// ALSA straight onto the hardware device, with no dmix, plug or sound server between, so the
/// period asked for is the period the DMA runs at. The engine holds the device alone while it
/// runs. ALSA has no callback: the audio thread is ours, and it waits for a free period, renders
/// into it and writes it.
pub struct AlsaDriver {
    request: Request,
    stats: Arc<Stats>,
}

impl AlsaDriver {
    pub fn new(request: Request, stats: Arc<Stats>) -> Self {
        Self { request, stats }
    }
}

impl Driver for AlsaDriver {
    /// Opens and configures the device on the calling thread, which blocks while ALSA does, then
    /// starts the audio thread.
    fn start(self: Box<Self>, render: Render) -> Result<Running, DriverError> {
        let Self { request, stats } = *self;
        let lib = asound()?;
        let device = match request.device {
            Some(device) => device,
            None => super::default_output()?,
        };
        let pcm = Pcm::open(lib, &device, request.period_frames, 1, Direction::Playback)?;
        let info = StreamInfo {
            sample_rate: RATE,
            period_frames: pcm.period,
            output_latency_frames: pcm.buffer,
        };
        stats.set_opened(format!(
            "ALSA {device}, {} x{}, {} frames in periods of {}",
            pcm.format.name(),
            pcm.channels,
            pcm.buffer,
            pcm.period,
        ));

        let stop = Arc::new(AtomicBool::new(false));
        let feed = Feed::new(render, Arc::clone(&stats));
        let audio = thread::Builder::new()
            .name("aeon-audio".into())
            .spawn({
                let stop = Arc::clone(&stop);
                move || run(pcm, feed, &stats, &stop)
            })
            .map_err(|error| DriverError::Backend(format!("audio thread: {error}")))?;

        Ok(Running::new(info, move || {
            stop.store(true, Ordering::Release);
            // The feed comes back so render, and whatever it holds, drops here and not on the
            // audio thread.
            drop(audio.join());
        }))
    }
}

/// Asks for realtime scheduling for this thread, and notes in `stats` whether it was granted.
pub(super) fn ask_for_realtime(stats: &Stats) {
    let param = libc::sched_param { sched_priority: PRIORITY };
    // SAFETY: sets this thread's own policy; fails without effect when the user may not.
    let realtime =
        unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) };
    stats.set_realtime(realtime == 0);
}

fn run(pcm: Pcm, mut feed: Feed, stats: &Stats, stop: &AtomicBool) -> Feed {
    ask_for_realtime(stats);
    match pcm.format {
        Format::F32 => play::<f32>(&pcm, &mut feed, stats, stop),
        Format::I32 => play::<i32>(&pcm, &mut feed, stats, stop),
        Format::I16 => play::<i16>(&pcm, &mut feed, stats, stop),
    }
    feed
}

fn play<S: Sample>(pcm: &Pcm, feed: &mut Feed, stats: &Stats, stop: &AtomicBool) {
    let channels = pcm.channels as usize;
    let mut period = vec![S::default(); pcm.period as usize * channels];
    // After a start or a recovery the first wakes fill the empty ring rather than follow the
    // device clock, so they are left out of the callback timing.
    let periods = (pcm.buffer / pcm.period.max(1)) as usize;
    let mut priming = periods;

    while !stop.load(Ordering::Acquire) {
        let ready = pcm.wait(STALL_MS);
        if ready == 0 {
            stats.lose();
            return;
        }
        if ready < 0 {
            if !recover(pcm, ready, stats) {
                return;
            }
            priming = periods;
            continue;
        }
        if priming > 0 {
            priming -= 1;
        } else {
            stats.callback();
        }

        feed.fill(&mut period, channels);
        let mut written = 0;
        while written < period.len() {
            let frames = pcm.write(&period[written..]);
            if frames < 0 {
                if !recover(pcm, frames as c_int, stats) {
                    return;
                }
                priming = periods;
                break;
            }
            written += frames as usize * channels;
        }
    }
}

fn recover(pcm: &Pcm, error: c_int, stats: &Stats) -> bool {
    if error == -libc::EPIPE {
        stats.underrun();
    }
    let recovered = pcm.recover(error) >= 0;
    if !recovered {
        stats.lose();
    }
    recovered
}
