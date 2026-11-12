use std::ffi::c_void;
use std::ptr::{NonNull, null};

use engine::driver::DriverError;
use objc2_core_audio::{
    AudioObjectAddPropertyListener, AudioObjectID, AudioObjectPropertyAddress,
    AudioObjectPropertySelector, AudioObjectRemovePropertyListener,
    kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices,
    kAudioObjectPropertyScopeGlobal,
};

use super::SYSTEM;
use super::property::{address, check, set};
use crate::wake::Wake;

/// `kAudioHardwarePropertyRunLoop`, from the deprecated header the crate puts behind a feature.
const RUN_LOOP: AudioObjectPropertySelector = u32::from_be_bytes(*b"rnlp");

/// The default output, and the device list a lost device leaves.
const WATCHED: [AudioObjectPropertySelector; 2] =
    [kAudioHardwarePropertyDefaultOutputDevice, kAudioHardwarePropertyDevices];

/// Listens on the system object for a new default output or a changed device list, and raises
/// the follower's wake from the HAL's notification thread. That is all the listener does: a stop
/// or start inside a HAL listener can deadlock against the HAL's own locks.
pub struct Watch {
    wake: NonNull<Wake>,
}

// SAFETY: the pointer is only handed to the HAL and read by its listener.
unsafe impl Send for Watch {}

impl Watch {
    pub fn start(wake: Wake) -> Result<Self, DriverError> {
        // Left alone, the HAL posts notifications to the main thread's run loop, which a Node
        // process never runs. Null asks it to run them on a thread of its own, process-wide.
        // SAFETY: the property is a CFRunLoopRef, and null is the value the HAL documents.
        unsafe { set(SYSTEM, RUN_LOOP, kAudioObjectPropertyScopeGlobal, &null::<c_void>())? };
        let watch = Self { wake: NonNull::from(Box::leak(Box::new(wake))) };
        for selector in WATCHED {
            let address = address(selector, kAudioObjectPropertyScopeGlobal);
            // SAFETY: `changed` matches the listener signature, and its client data outlives it.
            let status = unsafe {
                AudioObjectAddPropertyListener(
                    SYSTEM,
                    NonNull::from(&address),
                    Some(changed),
                    watch.wake.as_ptr().cast(),
                )
            };
            check(status, selector)?;
        }
        Ok(watch)
    }
}

impl Drop for Watch {
    // The HAL does not say that a listener call in flight has returned when removal does, so the
    // wake stays allocated: a few bytes for each engine the process opens.
    fn drop(&mut self) {
        for selector in WATCHED {
            let address = address(selector, kAudioObjectPropertyScopeGlobal);
            // SAFETY: the same listener and client data that `start` added; removing one that
            // was never added is an error the HAL returns, not undefined behaviour.
            unsafe {
                AudioObjectRemovePropertyListener(
                    SYSTEM,
                    NonNull::from(&address),
                    Some(changed),
                    self.wake.as_ptr().cast(),
                );
            }
        }
    }
}

unsafe extern "C-unwind" fn changed(
    _object: AudioObjectID,
    _count: u32,
    _addresses: NonNull<AudioObjectPropertyAddress>,
    wake: *mut c_void,
) -> i32 {
    // SAFETY: the client data is the wake `start` leaked, which is never freed.
    unsafe { (*wake.cast::<Wake>()).raise() };
    0
}
