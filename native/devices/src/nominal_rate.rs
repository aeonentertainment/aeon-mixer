use engine::driver::DriverError;

/// A device's nominal sample rate, which the OS keeps for every app and past the process: on
/// CoreAudio, `kAudioDevicePropertyNominalSampleRate`. A test passes a fake that records each set.
pub trait NominalRate {
    fn rate(&self, device: u32) -> Result<f64, DriverError>;
    /// Sets the rate and returns once the device reports it, or fails if it never does.
    fn set_rate(&self, device: u32, rate: f64) -> Result<(), DriverError>;
}
