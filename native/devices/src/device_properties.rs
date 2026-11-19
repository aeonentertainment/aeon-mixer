use engine::driver::DriverError;

/// The two properties of a device the engine sets and puts back, which the OS keeps for every app:
/// on CoreAudio, `kAudioDevicePropertyNominalSampleRate` and `kAudioDevicePropertyBufferFrameSize`.
/// A test passes a fake that records each set.
pub trait DeviceProperties {
    fn rate(&self, device: u32) -> Result<f64, DriverError>;
    /// Asks for the rate and returns at once; the HAL applies it in the order it was asked.
    fn put_rate(&self, device: u32, rate: f64) -> Result<(), DriverError>;
    /// Returns once the device reports `rate`, or fails if it does not within the second a
    /// change is given.
    fn wait_for(&self, device: u32, rate: f64) -> Result<(), DriverError>;
    /// The IO buffer size in frames.
    fn frames(&self, device: u32) -> Result<u32, DriverError>;
    /// Sets the IO buffer as near `frames` as the device allows, before it returns.
    fn put_frames(&self, device: u32, frames: u32) -> Result<(), DriverError>;
}
