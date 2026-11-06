use engine::block::BLOCK;

/// What the engine asks of an OS input: which device, the hardware period, and the one channel to
/// take. A backend gets as close to the period as the device allows and reports it in
/// `CaptureInfo`.
#[derive(Clone, Debug)]
pub struct CaptureRequest {
    /// An id from `inputs()`, or the default input.
    pub device: Option<String>,
    pub period_frames: u32,
    /// The device's channel, from 0.
    pub channel: u16,
}

impl Default for CaptureRequest {
    fn default() -> Self {
        Self { device: None, period_frames: BLOCK as u32, channel: 0 }
    }
}
