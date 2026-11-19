use crate::buffer_holds::BufferHolds;
use crate::io_buffer::IoBuffer;

/// One stream's IO buffer size on a device. Dropping it, after the stream has stopped, lets go,
/// and the last to let go puts back the size the first found.
pub struct BufferHold<'a, B: IoBuffer> {
    pub(crate) holds: &'a BufferHolds<B>,
    pub(crate) device: u32,
    /// What the device took, which may not be what was asked.
    pub frames: u32,
}

impl<B: IoBuffer> Drop for BufferHold<'_, B> {
    fn drop(&mut self) {
        self.holds.release(self.device);
    }
}
