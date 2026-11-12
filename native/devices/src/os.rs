use std::sync::Arc;

use engine::driver::{Driver, DriverError};

use crate::follow::System;
use crate::output::Output;
use crate::request::Request;
use crate::stats::Stats;
use crate::wake::Wake;

/// This OS's `System`: its outputs, its backend, and its listeners.
pub struct Os;

impl System for Os {
    fn outputs(&self) -> Result<Vec<Output>, DriverError> {
        crate::outputs()
    }

    fn driver(&self, request: Request, stats: Arc<Stats>) -> Box<dyn Driver + Send> {
        crate::driver(request, stats)
    }

    fn watch(&self, wake: Wake) -> Result<Box<dyn Send>, DriverError> {
        #[cfg(target_os = "macos")]
        return Ok(Box::new(crate::coreaudio::Watch::start(wake)?));
        #[cfg(windows)]
        return Ok(Box::new(crate::wasapi::Watch::start(wake)?));
        // ALSA has no default above the hardware device; a lost stream wakes the follower itself
        #[cfg(target_os = "linux")]
        return {
            drop(wake);
            Ok(Box::new(()))
        };
    }
}
