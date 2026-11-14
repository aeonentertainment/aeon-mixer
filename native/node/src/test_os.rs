//! A stand-in for the OS that the TypeScript tests reach through `openEngine`, so they can show
//! what a device open does without opening a device. `AEON_MIXER_TEST_OS=hangs` selects it.

use std::sync::Arc;

use devices::{Output, Request, Stats, System, Wake};
use engine::driver::{Driver, DriverError};

use crate::hanging_start::HangingStart;

/// One output, the default, whose start never returns: the wedged output of decision 0016.
pub struct TestOs;

impl TestOs {
    /// The test OS when the environment asks for it. Read on the control thread, where Node
    /// writes the environment.
    pub fn from_env() -> Option<Self> {
        (std::env::var_os("AEON_MIXER_TEST_OS").is_some_and(|value| value == "hangs"))
            .then_some(Self)
    }
}

impl System for TestOs {
    fn outputs(&self) -> Result<Vec<Output>, DriverError> {
        Ok(vec![Output {
            id: "test-hangs".into(),
            name: "Hanging output".into(),
            is_default: true,
        }])
    }

    fn driver(&self, _request: Request, _stats: Arc<Stats>) -> Box<dyn Driver + Send> {
        Box::new(HangingStart)
    }

    fn watch(&self, wake: Wake) -> Result<Box<dyn Send>, DriverError> {
        Ok(Box::new(wake))
    }
}
