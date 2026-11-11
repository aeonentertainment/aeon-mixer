//! What drives the audio thread: an OS output through the devices crate, the timer clock on a box
//! with no sound card, or the offline driver pumped from JavaScript.

use std::sync::Arc;

use devices::{Request, Stats, TimerDriver};
use engine::driver::Driver;
use engine::driver::{OfflineDriver, Render, Running, StreamInfo};
use napi::bindgen_prelude::*;
use napi_derive::napi;

pub enum Clock {
    Offline(Box<OfflineDriver>),
    Timer { running: Running, stats: Arc<Stats> },
    Device { running: Running, stats: Arc<Stats> },
}

impl Clock {
    /// Starts this OS's backend on `device`, or the default output, at `period_frames`.
    pub fn open(device: Option<String>, period_frames: u32, render: Render) -> Result<Self> {
        let stats = Arc::new(Stats::new());
        let running = devices::driver(Request { device, period_frames }, Arc::clone(&stats))
            .start(render)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        Ok(Self::Device { running, stats })
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
            Self::Timer { running, .. } | Self::Device { running, .. } => running.info,
        }
    }

    pub fn stats(&self) -> Option<Arc<Stats>> {
        match self {
            Self::Offline(_) => None,
            Self::Timer { stats, .. } | Self::Device { stats, .. } => Some(Arc::clone(stats)),
        }
    }

    /// The outputs this OS offers. The offline and timer clocks never ask the OS and list none.
    pub fn outputs(&self) -> Result<Vec<OutputEntry>> {
        match self {
            Self::Offline(_) | Self::Timer { .. } => Ok(Vec::new()),
            Self::Device { .. } => devices::outputs()
                .map(|outputs| outputs.into_iter().map(OutputEntry::from).collect())
                .map_err(|error| Error::from_reason(error.to_string())),
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
