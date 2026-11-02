use engine::block::BLOCK;

/// What the engine asks of the OS output: which device, and the hardware period. A backend gets
/// as close as the device allows and reports what it got in `StreamInfo`.
#[derive(Clone, Debug)]
pub struct Request {
    /// An id from `outputs()`, or the default output.
    pub device: Option<String>,
    pub period_frames: u32,
}

impl Default for Request {
    fn default() -> Self {
        Self { device: None, period_frames: BLOCK as u32 }
    }
}
