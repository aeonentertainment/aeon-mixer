//! The three Mach clock calls the timer makes on macOS, declared here since the libc crate
//! deprecates its own copies.

use std::ffi::c_int;
use std::time::Duration;

#[repr(C)]
struct RawTimebase {
    numer: u32,
    denom: u32,
}

unsafe extern "C" {
    fn mach_absolute_time() -> u64;
    fn mach_timebase_info(info: *mut RawTimebase) -> c_int;
    fn mach_wait_until(deadline: u64) -> c_int;
}

/// The ratio between mach_absolute_time ticks and nanoseconds: 125/3 on Apple silicon, 1 on
/// Intel.
#[derive(Clone, Copy)]
pub struct Timebase {
    numer: u128,
    denom: u128,
}

impl Timebase {
    pub fn get() -> Self {
        let mut raw = RawTimebase { numer: 1, denom: 1 };
        // SAFETY: writes the struct it is handed; it cannot fail on macOS.
        unsafe { mach_timebase_info(&mut raw) };
        Self { numer: raw.numer.into(), denom: raw.denom.into() }
    }

    pub fn now(self) -> Duration {
        // SAFETY: reads the commpage clock; no arguments.
        let ticks = u128::from(unsafe { mach_absolute_time() });
        Duration::from_nanos((ticks * self.numer / self.denom) as u64)
    }

    /// Ticks for `duration`, rounded up so a wait never ends a tick early.
    pub fn ticks(self, duration: Duration) -> u64 {
        (duration.as_nanos() * self.denom).div_ceil(self.numer) as u64
    }

    pub fn wait_until(self, deadline: Duration) {
        // SAFETY: takes a plain number; an abort returns early, which the pacer handles.
        unsafe { mach_wait_until(self.ticks(deadline)) };
    }
}
