use std::ffi::c_void;
use std::ptr::NonNull;
use std::slice;

use engine::driver::DriverError;
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioObjectID,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};

use super::capture_callback::CaptureCallback;

/// An input IOProc running on a device, and the callback it hands audio to. Dropping it stops the
/// proc, unregisters it, and only then frees the callback, so the HAL never sees a dangling
/// pointer.
pub struct CaptureIo {
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    callback: NonNull<CaptureCallback>,
}

// SAFETY: while the proc is registered only the HAL's IO thread touches the callback; after
// `drop` unregisters it, only the dropping thread does. The ids are plain values.
unsafe impl Send for CaptureIo {}

impl CaptureIo {
    pub fn start(device: AudioObjectID, callback: CaptureCallback) -> Result<Self, DriverError> {
        let callback = NonNull::from(Box::leak(Box::new(callback)));
        let mut proc_id: AudioDeviceIOProcID = None;
        // SAFETY: `input_proc` matches the HAL's signature and `callback` lives until `drop`.
        let created = unsafe {
            AudioDeviceCreateIOProcID(
                device,
                Some(input_proc),
                callback.as_ptr().cast(),
                NonNull::from(&mut proc_id),
            )
        };
        if created != 0 {
            // SAFETY: never registered, so this is the only pointer to it.
            drop(unsafe { Box::from_raw(callback.as_ptr()) });
            return Err(failed("register an input IOProc", created));
        }
        let io = Self { device, proc_id, callback };
        // SAFETY: the proc was registered on this device above.
        match unsafe { AudioDeviceStart(device, proc_id) } {
            0 => Ok(io),
            status => Err(failed("start the input", status)),
        }
    }
}

impl Drop for CaptureIo {
    fn drop(&mut self) {
        // SAFETY: the proc is registered on the device. Once it is destroyed the HAL holds no
        // pointer to the callback, which `start` leaked for exactly this.
        unsafe {
            AudioDeviceStop(self.device, self.proc_id);
            AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            drop(Box::from_raw(self.callback.as_ptr()));
        }
    }
}

unsafe extern "C-unwind" fn input_proc(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    callback: *mut c_void,
) -> i32 {
    // SAFETY: `callback` is the one registered with this proc, alive until it is destroyed, and
    // the HAL calls a proc from one thread at a time. The buffer list holds mNumberBuffers
    // buffers, and it and the timestamp are valid for the call.
    unsafe {
        let callback = &mut *callback.cast::<CaptureCallback>();
        let list = input.as_ptr();
        let buffers = slice::from_raw_parts(
            (&raw const (*list).mBuffers).cast::<AudioBuffer>(),
            (*list).mNumberBuffers as usize,
        );
        callback.run(buffers, input_time.as_ref());
    }
    0
}

fn failed(doing: &str, status: i32) -> DriverError {
    DriverError::Backend(format!("CoreAudio could not {doing}: OSStatus {status}"))
}
