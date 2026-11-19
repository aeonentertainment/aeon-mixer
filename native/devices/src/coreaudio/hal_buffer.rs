use engine::driver::DriverError;
use objc2_core_audio::{kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal};

use super::property::{get, set};
use crate::buffer_holds::BufferHolds;
use crate::io_buffer::IoBuffer;

/// Every device whose IO buffer this process sets, its output streams and its inputs alike.
pub static BUFFERS: BufferHolds<HalBuffer> = BufferHolds::new(HalBuffer);

/// The HAL's IO buffer size of a device, by AudioObjectID.
pub struct HalBuffer;

impl IoBuffer for HalBuffer {
    fn frames(&self, device: u32) -> Result<u32, DriverError> {
        // SAFETY: the buffer size is a UInt32.
        unsafe { get(device, kAudioDevicePropertyBufferFrameSize, kAudioObjectPropertyScopeGlobal) }
    }

    fn put_frames(&self, device: u32, frames: u32) -> Result<(), DriverError> {
        // SAFETY: the buffer size is a UInt32.
        unsafe {
            set(
                device,
                kAudioDevicePropertyBufferFrameSize,
                kAudioObjectPropertyScopeGlobal,
                &frames,
            )
        }
    }
}
