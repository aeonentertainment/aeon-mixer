use crate::nominal_rate::NominalRate;
use crate::rate_holds::RateHolds;

/// One stream's use of a device at 48 kHz. Dropping it, after the stream has stopped, lets go, and
/// the last to let go puts back the rate the first found.
pub struct RateHold<'a, R: NominalRate> {
    pub(crate) holds: &'a RateHolds<R>,
    pub(crate) device: u32,
}

impl<R: NominalRate> Drop for RateHold<'_, R> {
    fn drop(&mut self) {
        self.holds.release(self.device);
    }
}
