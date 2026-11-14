use engine::driver::DriverError;

/// A device's nominal sample rate, which the OS keeps for every app and past the process: on
/// CoreAudio, `kAudioDevicePropertyNominalSampleRate`. A test passes a fake that records each set.
pub trait NominalRate {
    fn rate(&self, device: u32) -> Result<f64, DriverError>;
    /// Asks for the rate and returns at once; the HAL applies it in the order it was asked.
    fn put_rate(&self, device: u32, rate: f64) -> Result<(), DriverError>;
    /// Returns once the device reports `rate`, or fails if it does not within the second a
    /// change is given.
    fn wait_for(&self, device: u32, rate: f64) -> Result<(), DriverError>;
}
