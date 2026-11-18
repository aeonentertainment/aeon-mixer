use engine::driver::DriverError;

/// A device's IO buffer size in frames: on CoreAudio, `kAudioDevicePropertyBufferFrameSize`. The
/// HAL applies a set before it returns. A test passes a fake that records each set.
pub trait IoBuffer {
    fn frames(&self, device: u32) -> Result<u32, DriverError>;
    fn put_frames(&self, device: u32, frames: u32) -> Result<(), DriverError>;
}
