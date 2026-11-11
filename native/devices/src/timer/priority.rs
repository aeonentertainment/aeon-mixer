//! Asks the OS to run the timer's thread ahead of ordinary ones. Each returns whether it did,
//! for `Stats::set_realtime`.

/// SCHED_FIFO priority: the ALSA backend's, above every desktop thread, below the kernel's own.
#[cfg(target_os = "linux")]
const PRIORITY: libc::c_int = 70;

/// A container grants SCHED_FIFO only with CAP_SYS_NICE and an rtprio limit. Without it the
/// thread stays ordinary, so its timer slack goes to 1 ns: by default Linux lets an ordinary
/// thread's wake run 50 µs late to batch timers, and never a realtime one's. 0 would restore
/// the default.
#[cfg(target_os = "linux")]
pub fn promote() -> bool {
    let slack: libc::c_ulong = 1;
    // SAFETY: both act on the calling thread only and fail without effect.
    unsafe {
        libc::prctl(libc::PR_SET_TIMERSLACK, slack);
        let param = libc::sched_param { sched_priority: PRIORITY };
        libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_FIFO, &param) == 0
    }
}

/// The time-constraint policy CoreAudio's own IO threads run under: a period of one block, half
/// of it for computation. It also exempts the thread's wakes from timer coalescing.
#[cfg(target_os = "macos")]
pub fn promote() -> bool {
    use std::time::Duration;

    use engine::block::{BLOCK, RATE};

    use super::mach::Timebase;

    let block = Duration::from_nanos(1_000_000_000 * BLOCK as u64 / u64::from(RATE));
    let period = Timebase::get().ticks(block) as u32;
    let mut policy = libc::thread_time_constraint_policy {
        period,
        computation: period / 2,
        constraint: period,
        preemptible: 1,
    };
    // SAFETY: the policy is the flavour's own struct, and its count is that struct's size.
    unsafe {
        libc::thread_policy_set(
            libc::pthread_mach_thread_np(libc::pthread_self()),
            libc::THREAD_TIME_CONSTRAINT_POLICY as libc::thread_policy_flavor_t,
            (&raw mut policy).cast(),
            libc::THREAD_TIME_CONSTRAINT_POLICY_COUNT,
        ) == libc::KERN_SUCCESS
    }
}

/// A Windows box has a sound card, so the timer there is for development and stays ordinary.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn promote() -> bool {
    false
}
