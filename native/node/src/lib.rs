//! The napi binding. Control-thread code only: it builds engines and forwards calls. Audio never
//! comes through here. `src/` in the package root turns this into the public TypeScript API.

use engine::driver::{OfflineDriver, silent_render};
use napi::bindgen_prelude::*;
use napi_derive::napi;

#[napi(object)]
pub struct OutputInfo {
    pub sample_rate: u32,
    pub period_frames: u32,
    pub latency_ms: f64,
}

/// An engine on the offline driver: the test clock, pumped from JavaScript.
#[napi]
pub struct OfflineEngine {
    driver: Option<OfflineDriver>,
}

#[napi]
impl OfflineEngine {
    #[napi(constructor)]
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self { driver: Some(OfflineDriver::new(silent_render())) }
    }

    #[napi(getter)]
    pub fn output(&self) -> Result<OutputInfo> {
        let info = self.driver()?.info();
        Ok(OutputInfo {
            sample_rate: info.sample_rate,
            period_frames: info.period_frames,
            latency_ms: f64::from(info.output_latency_frames) * 1000.0
                / f64::from(info.sample_rate),
        })
    }

    /// Renders `blocks` blocks and returns them as interleaved stereo.
    #[napi]
    pub fn pump(&mut self, blocks: u32) -> Result<Float32Array> {
        let driver = self.driver.as_mut().ok_or_else(closed)?;
        Ok(Float32Array::new(driver.pump(blocks as usize)))
    }

    #[napi]
    pub fn close(&mut self) {
        self.driver = None;
    }

    fn driver(&self) -> Result<&OfflineDriver> {
        self.driver.as_ref().ok_or_else(closed)
    }
}

fn closed() -> Error {
    Error::from_reason("the engine is closed")
}
