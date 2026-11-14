use std::fmt::Write;
use std::time::{Duration, Instant};

use engine::driver::DriverError;

use crate::summary::ms;

/// When each step of a start finished, so a start that fails says where its time went: in the
/// rate change, in setting the IO buffer, or in the HAL's own start.
pub struct Steps {
    began: Instant,
    done: Vec<(&'static str, Duration)>,
}

impl Steps {
    pub fn new() -> Self {
        Self { began: Instant::now(), done: Vec::new() }
    }

    pub fn done(&mut self, step: &'static str) {
        self.done.push((step, self.began.elapsed()));
    }

    /// `error`, with how long into the start it came and what had finished by then.
    pub fn failed(&self, error: DriverError) -> DriverError {
        let DriverError::Backend(reason) = error else { return error };
        let mut reason = format!("{reason}, {} ms into the start", ms(self.began.elapsed()));
        for (at, (step, done)) in self.done.iter().enumerate() {
            let joint = if at == 0 { ", after" } else { "," };
            let _ = write!(reason, "{joint} {step} at {} ms", ms(*done));
        }
        DriverError::Backend(reason)
    }
}
