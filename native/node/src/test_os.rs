//! A stand-in for the OS that the TypeScript tests reach through `openEngine` and the device
//! lists, so they can show what a device open and a listing do without touching a device.
//! `AEON_MIXER_TEST_OS=hangs` selects it.

use std::sync::Arc;

use devices::{Output, Request, Stats, System, Transport, Wake};
use engine::driver::{Driver, DriverError};

use crate::hanging_start::HangingStart;

/// One device of each transport, the built-in one the default, listed as outputs and as inputs.
/// Every output's start never returns: the default is the wedged output of decision 0016.
pub struct TestOs;

impl TestOs {
    /// The test OS when the environment asks for it. Read on the control thread, where Node
    /// writes the environment.
    pub fn from_env() -> Option<Self> {
        (std::env::var_os("AEON_MIXER_TEST_OS").is_some_and(|value| value == "hangs"))
            .then_some(Self)
    }

    /// What it lists as its outputs and its inputs alike.
    pub fn devices(&self) -> Vec<Output> {
        Transport::ALL
            .into_iter()
            .map(|transport| Output {
                id: format!("test-{transport}"),
                name: format!("Test {transport}"),
                is_default: transport == Transport::BuiltIn,
                transport,
            })
            .collect()
    }
}

impl System for TestOs {
    fn outputs(&self) -> Result<Vec<Output>, DriverError> {
        Ok(self.devices())
    }

    fn driver(&self, _request: Request, _stats: Arc<Stats>) -> Box<dyn Driver + Send> {
        Box::new(HangingStart)
    }

    fn watch(&self, wake: Wake) -> Result<Box<dyn Send>, DriverError> {
        Ok(Box::new(wake))
    }
}
