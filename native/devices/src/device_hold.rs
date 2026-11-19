use crate::device_holds::DeviceHolds;
use crate::device_properties::DeviceProperties;

/// One stream's use of a device at 48 kHz and its period. Dropping it, after the stream has
/// stopped, lets go, and the last to let go puts back the buffer size, then the rate, the first
/// found.
pub struct DeviceHold<'a, P: DeviceProperties> {
    pub(crate) holds: &'a DeviceHolds<P>,
    pub(crate) device: u32,
    /// The IO buffer the device took, which may not be what was asked.
    pub frames: u32,
}

impl<P: DeviceProperties> Drop for DeviceHold<'_, P> {
    fn drop(&mut self) {
        self.holds.release(self.device);
    }
}
