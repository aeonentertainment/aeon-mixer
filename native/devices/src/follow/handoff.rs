use std::marker::PhantomData;
use std::ptr::{NonNull, null_mut};
use std::sync::atomic::{AtomicPtr, Ordering};

/// One value that whichever side takes it first keeps: the stream's first render, or the follower
/// giving up on a start that hangs. Taking it allocates and frees nothing, so the audio thread may
/// take it; the box is freed where the value is unboxed, off the audio thread.
pub struct Handoff<T> {
    slot: AtomicPtr<T>,
    /// Owns a `T`, so it is `Send` only when `T` is.
    owns: PhantomData<Box<T>>,
}

impl<T> Handoff<T> {
    pub fn new(value: T) -> Self {
        Self { slot: AtomicPtr::new(Box::into_raw(Box::new(value))), owns: PhantomData }
    }

    /// The value, if nobody has taken it yet.
    pub fn take(&self) -> Option<Box<T>> {
        let taken = NonNull::new(self.slot.swap(null_mut(), Ordering::AcqRel))?;
        // SAFETY: the pointer came from `Box::into_raw` in `new`, and the swap left it to this
        // caller alone.
        Some(unsafe { Box::from_raw(taken.as_ptr()) })
    }
}

impl<T> Drop for Handoff<T> {
    fn drop(&mut self) {
        drop(self.take());
    }
}

// SAFETY: shared, the handoff only ever hands the value out once, by an atomic swap.
unsafe impl<T: Send> Sync for Handoff<T> {}
