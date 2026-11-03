//! What drives the audio thread: an OS output through the devices crate, or the offline driver
//! pumped from JavaScript.

use std::sync::Arc;

use devices::{Request, Stats};
use engine::driver::{OfflineDriver, Render, Running, StreamInfo};
use napi::bindgen_prelude::*;
use napi_derive::napi;

pub enum Clock {
    Offline(Box<OfflineDriver>),
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

    pub fn info(&self) -> StreamInfo {
        match self {
            Self::Offline(driver) => driver.info(),
            Self::Device { running, .. } => running.info,
        }
    }

    pub fn stats(&self) -> Option<Arc<Stats>> {
        match self {
            Self::Offline(_) => None,
            Self::Device { stats, .. } => Some(Arc::clone(stats)),
        }
    }

    /// The outputs this OS offers. The offline clock never asks the OS and lists none.
    pub fn outputs(&self) -> Result<Vec<OutputEntry>> {
        match self {
            Self::Offline(_) => Ok(Vec::new()),
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
