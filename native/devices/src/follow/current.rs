use std::sync::{Arc, Mutex, PoisonError};

use crate::follow::playing::Playing;

/// The follower's `Playing`, shared with whoever reports it: the binding's getter and its events
/// thread, which turns a new `moves` into an event. Read and written off the audio thread only.
#[derive(Clone, Debug)]
pub struct Current(Arc<Mutex<Playing>>);

impl Current {
    pub(super) fn new(playing: Playing) -> Self {
        Self(Arc::new(Mutex::new(playing)))
    }

    pub fn get(&self) -> Playing {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub(super) fn set(&self, playing: Playing) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = playing;
    }
}
