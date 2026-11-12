use std::sync::Arc;

use engine::driver::{Driver, DriverError};

use crate::output::Output;
use crate::request::Request;
use crate::stats::Stats;
use crate::wake::Wake;

/// The OS behind the follower: what it lists, how it opens a stream, and how it says something
/// changed. `Os` is this OS's; a test passes fake devices.
pub trait System: Send + 'static {
    fn outputs(&self) -> Result<Vec<Output>, DriverError>;
    fn driver(&self, request: Request, stats: Arc<Stats>) -> Box<dyn Driver + Send>;
    /// Raises `wake` whenever the default output or the device list changes, until dropped.
    fn watch(&self, wake: Wake) -> Result<Box<dyn Send>, DriverError>;
}
