//! Typed reads and writes of CoreAudio object properties, element main.

use std::mem::{MaybeUninit, size_of};
use std::ptr::{NonNull, null};

use engine::driver::DriverError;
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID,
    AudioObjectPropertyAddress, AudioObjectPropertyScope, AudioObjectPropertySelector,
    AudioObjectSetPropertyData, kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
};
use objc2_core_foundation::{CFRetained, CFString};

/// Reads a fixed-size property.
///
/// # Safety
/// `T` must be the property's C type.
pub unsafe fn get<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<T, DriverError> {
    let address = address(selector, scope);
    let mut value = MaybeUninit::<T>::uninit();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `value` has room for `size` bytes, and the caller vouches that `T` is the type.
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::from(&mut value).cast(),
        )
    };
    check(status, selector)?;
    // SAFETY: the HAL wrote the whole value.
    Ok(unsafe { value.assume_init() })
}

/// Reads a property that is an array of `T`.
///
/// # Safety
/// `T` must be the C type of the array's elements.
pub unsafe fn get_list<T: Copy>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> Result<Vec<T>, DriverError> {
    let address = address(selector, scope);
    let mut size = 0u32;
    // SAFETY: `size` receives the byte length.
    let status = unsafe {
        AudioObjectGetPropertyDataSize(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
        )
    };
    check(status, selector)?;
    let mut list = Vec::<T>::with_capacity(size as usize / size_of::<T>());
    if list.capacity() == 0 {
        return Ok(list);
    }
    // SAFETY: `list` has room for `size` bytes of `T`, and the HAL reports how many it wrote.
    unsafe {
        let status = AudioObjectGetPropertyData(
            object,
            NonNull::from(&address),
            0,
            null(),
            NonNull::from(&mut size),
            NonNull::new_unchecked(list.as_mut_ptr()).cast(),
        );
        check(status, selector)?;
        list.set_len(size as usize / size_of::<T>());
    }
    Ok(list)
}

/// Writes a fixed-size property.
///
/// # Safety
/// `T` must be the property's C type.
pub unsafe fn set<T>(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
    value: &T,
) -> Result<(), DriverError> {
    let address = address(selector, scope);
    // SAFETY: `value` is `size_of::<T>()` readable bytes of the property's type.
    let status = unsafe {
        AudioObjectSetPropertyData(
            object,
            NonNull::from(&address),
            0,
            null(),
            size_of::<T>() as u32,
            NonNull::from(value).cast(),
        )
    };
    check(status, selector)
}

/// Reads a CFString property, such as a name or a UID, which the caller owns.
pub fn string(
    object: AudioObjectID,
    selector: AudioObjectPropertySelector,
) -> Result<String, DriverError> {
    // SAFETY: names and UIDs are CFStringRefs, retained for the caller.
    let value: *const CFString = unsafe { get(object, selector, kAudioObjectPropertyScopeGlobal)? };
    let value = NonNull::new(value.cast_mut()).ok_or_else(|| {
        DriverError::Backend(format!("CoreAudio '{}' is empty", fourcc(selector)))
    })?;
    // SAFETY: the reference is ours to release, and CFRetained releases it.
    Ok(unsafe { CFRetained::from_raw(value) }.to_string())
}

pub fn check(status: i32, selector: AudioObjectPropertySelector) -> Result<(), DriverError> {
    match status {
        0 => Ok(()),
        _ => Err(DriverError::Backend(format!(
            "CoreAudio '{}' failed with OSStatus {status}",
            fourcc(selector)
        ))),
    }
}

pub fn address(
    selector: AudioObjectPropertySelector,
    scope: AudioObjectPropertyScope,
) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

// Selectors are four-character codes, which is how Apple's documentation names them.
fn fourcc(selector: AudioObjectPropertySelector) -> String {
    selector.to_be_bytes().iter().map(|byte| char::from(*byte)).collect()
}
