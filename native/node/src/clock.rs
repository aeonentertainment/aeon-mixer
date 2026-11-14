//! What drives the audio thread: an OS output through the devices crate's follower, the timer
//! clock on a box with no sound card, or the offline driver pumped from JavaScript.

use std::sync::Arc;

use devices::{Current, Follower, Os, Playing, Request, StartError, Stats, TimerDriver};
use engine::audio_thread::AudioThread;
use engine::driver::{Driver, OfflineDriver, Render, Running, StreamInfo};
use napi::bindgen_prelude::*;
use napi_derive::napi;

use crate::test_os::TestOs;

pub enum Clock {
    Offline(Box<OfflineDriver>),
    Timer { running: Running, stats: Arc<Stats> },
    Device { follower: Follower, stats: Arc<Stats> },
}

impl Clock {
    /// Starts this OS's backend at `period_frames` on `device`, the pinned output, or on the
    /// default, and follows the room's output from there (docs/design.md section 5). Blocks while
    /// the first stream starts. `test_os` stands in for this OS in the TypeScript tests.
    pub fn open(
        device: Option<String>,
        period_frames: u32,
        audio: AudioThread,
        test_os: Option<TestOs>,
    ) -> std::result::Result<Self, StartError> {
        let stats = Arc::new(Stats::new());
        let request = Request { device, period_frames };
        let follower = match test_os {
            Some(test_os) => Follower::start(request, audio, test_os, Arc::clone(&stats))?,
            None => Follower::start(request, audio, Os, Arc::clone(&stats))?,
        };
        Ok(Self::Device { follower, stats })
    }

    /// Starts the timer clock's thread, which renders a block at each deadline on its own.
    pub fn timer(render: Render) -> Result<Self> {
        let stats = Arc::new(Stats::new());
        let running = Box::new(TimerDriver::new(Arc::clone(&stats)))
            .start(render)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        Ok(Self::Timer { running, stats })
    }

    pub fn info(&self) -> StreamInfo {
        match self {
            Self::Offline(driver) => driver.info(),
            Self::Timer { running, .. } => running.info,
            Self::Device { follower, .. } => follower.current().get().info,
        }
    }

    /// Where a device clock plays now; the offline and timer clocks play nowhere.
    pub fn playing(&self) -> Option<Playing> {
        match self {
            Self::Offline(_) | Self::Timer { .. } => None,
            Self::Device { follower, .. } => Some(follower.current().get()),
        }
    }

    pub fn stats(&self) -> Option<Arc<Stats>> {
        match self {
            Self::Offline(_) => None,
            Self::Timer { stats, .. } | Self::Device { stats, .. } => Some(Arc::clone(stats)),
        }
    }

    /// What the events thread watches for moves.
    pub fn current(&self) -> Option<Current> {
        match self {
            Self::Offline(_) | Self::Timer { .. } => None,
            Self::Device { follower, .. } => Some(follower.current()),
        }
    }

    /// The outputs this OS offers. The offline and timer clocks never ask the OS and list none.
    pub fn outputs(&self) -> Result<Vec<OutputEntry>> {
        match self {
            Self::Offline(_) | Self::Timer { .. } => Ok(Vec::new()),
            Self::Device { .. } => crate::listing::outputs(),
        }
    }
}

#[napi(object)]
pub struct OutputEntry {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

impl From<devices::Output> for OutputEntry {
    fn from(output: devices::Output) -> Self {
        Self { id: output.id, name: output.name, is_default: output.is_default }
    }
}
