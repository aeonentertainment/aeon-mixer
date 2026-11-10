# 0011: A timer clock for a box with no sound card

**Status:** accepted, 2026-11-09

## Decision

`openEngine({ output: { timer: true } })` runs the engine on a thread of its own that sleeps to
each block's deadline on the OS's monotonic clock and renders that block. It opens no device,
lists no outputs and reports a latency of zero, as the offline clock does; the mix leaves only
on a mixer's `rtpReturn`. A host with nothing to play locally, such as a development container
or a CI runner, opens this instead of pumping the offline engine from a JavaScript timer, so a
stalled event loop no longer stalls the mix.

- **Absolute deadlines.** Block `n` is due `n` periods after the clock started, worked out from
  the start every time, so a period of 2 666 666.67 ns never rounds into drift. Linux waits
  with `clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME)`, macOS with `mach_wait_until`, Windows
  and the rest with std's sleep to the deadline, which on Windows is a high-resolution waitable
  timer since Rust 1.75.
- **A late wake catches up.** Each wake renders every block that has fallen due, so the
  engine's output clock stays on wall time. A stall longer than a second renders only the
  newest block and counts one underrun.
- **Realtime where the host grants it.** Linux asks for SCHED_FIFO at 70, the ALSA backend's
  priority, and sets the thread's timer slack to 1 ns either way. macOS asks for the
  time-constraint policy CoreAudio's IO threads run under. Windows stays ordinary: a Windows
  room has a sound card.
- **It lives in `devices`, outside the `hardware` feature.** It needs OS calls and their
  `unsafe`, which belong in `devices`, but no audio stack, so it builds and runs where none is
  installed.

## How it squares with the audio thread's rules

Section 2's rule, nothing in the OS, is about render. The timer's wait is the clock itself and
sits between renders, where a device backend's thread waits in `poll` or on the HAL's semaphore
before its callback. From one wait to the next the thread reads the clock in userspace (the
vDSO, the commpage, QueryPerformanceCounter), renders through the same `Feed` every backend
uses, and allocates nothing: the tests run the whole loop under the allocation guard on a
synthetic clock.

## Measured

`measure-clock`, which prints the timer's wakes in `measure-sink`'s form plus the blocks it
rendered against the blocks wall time reached. 60 s per run, with 300 µs of busy work a block
standing in for a mix.

| Machine                        | Load                          | Realtime | p99.9, max interval | Late over 1.5 periods | Blocks rendered, expected | Underruns |
| ------------------------------ | ----------------------------- | -------- | ------------------- | --------------------- | ------------------------- | --------- |
| MacBook Pro M2 Pro, macOS 26.6 | idle                          | yes      | 2.75, 2.85 ms       | 0                     | 22 502, 22 501            | 0         |
|                                | 20 busy processes on 10 cores | yes      | 2.70, 2.72 ms       | 0                     | 22 503, 22 503            | 0         |

With no work a block the clock cost 0.07 s of CPU in 30 s, 0.2 percent of one core.

## Why catch up, not drop

Nothing plays the timer's output, so a block rendered late is late nowhere but on the return
stream. What matters is what each policy does to the mic path.

- **Catch up.** The jitter buffer reads arrivals at the top of each block against the output
  clock, not the wall. Packets that came during a stall are read when the catch-up renders
  their blocks, on time, and none is counted late while the stall fits the packet ring: 64
  packets, 160 ms of 2.5 ms frames or 1.28 s of 20 ms frames. The cost is a burst on the
  return stream: its ring holds 50 ms, so a stall past that drops return blocks the network
  thread has not sent, and the listener's own jitter buffer conceals them.
- **Drop and count.** The output clock falls behind the wall by every stall, so every slot's
  packets arrive that much early and the voice plays that much later. Skew correction pulls it
  back one codec frame a second: a 20 ms stall costs 20 ms on every mic for a second with 20 ms
  frames and for eight seconds with 2.5 ms frames. That is the whole 20 to 30 ms budget.
- **Past a second.** A stall that long is a suspended process or a paused VM, and the packet
  ring has overflowed for every phone. Replaying it would spin a core for its length to fill
  rings that already lost the audio.

## Drift against the senders

The schedule is exact, and a test holds 100 000 jittery wakes to wall time to the block. What
remains is the OS clock's rate against each sender's. Linux's `CLOCK_MONOTONIC` is slewed by
NTP, so on a synced box it runs at true time within a few parts per million, though it may slew
up to 500 ppm while converging after boot. macOS's `mach_absolute_time` is the raw crystal. A
phone stamps RTP from its own audio crystal, tens of ppm off, and a sound card would be as far
off the other way, so the timer is no worse than a device. At 50 ppm a sender drifts 2.4 frames
a second; skew correction moves one codec frame once a second when the last second's packets
all agree, so it acts once every 50 s with 2.5 ms frames and every 400 s with 20 ms frames, and
its reach, one codec frame a second, is 2 500 ppm at the shortest frame.

Wake jitter does not reach the jitter buffer at all below the catch-up bound, since it reads
arrivals on the output clock. It reaches only the return stream, where a wake L late sends L
late.

## Rejected

- **The JavaScript pump.** The status quo: the offline engine rendered from a 5 ms `setInterval`
  on the event loop. Garbage collection and any synchronous work on the loop stall the mix, and
  the timer's resolution is the loop's.
- **Spinning, or sleeping then spinning.** A core at 100 percent for sub-millisecond precision
  that catch-up does not need.
- **ALSA's `snd-dummy` or `snd-aloop`.** Kernel modules the host must load; a container cannot,
  and neither Docker Desktop's VM nor a CI runner ships them. They put a two-period ALSA ring,
  5.3 ms, behind the same kernel hrtimer for no gain.
- **JACK's dummy backend or PipeWire's Dummy-Driver.** Both are this loop, PipeWire's a timerfd
  on the monotonic clock, inside a sound server the box would then have to run.
- **A CoreAudio null device.** It needs an AudioServerPlugIn installed system-wide, and a Mac
  that hosts the engine has an output anyway.
- **timerfd and epoll.** The same kernel timer as `clock_nanosleep` behind a file descriptor;
  the thread has nothing else to wait on.

## Not measured yet

- Linux, on a box and in a container. A container drops CAP_SYS_NICE, so SCHED_FIFO fails and
  the thread runs ordinary with 1 ns of slack; Docker grants it with `--cap-add SYS_NICE
  --ulimit rtprio=95` where the kernel has no realtime group scheduling, and that is unverified
  here. Run `cargo run --release -p devices --bin measure-clock -- --work-us 300` there.
- Windows, where std's sleep resolves to about half a millisecond.

## Sources

- [PipeWire driver scheduling](https://docs.pipewire.org/page_driver.html): a driver on the
  monotonic clock wakes through timerfd, and the Dummy-Driver clocks every node with no device.
- [cyclictest's nanosleep option](https://wiki.linuxfoundation.org/realtime/documentation/howto/tools/cyclictest/options/nanosleep):
  `clock_nanosleep` wakes from hard interrupt context, faster than signal-based POSIX timers.
- [CreateWaitableTimerExW](https://learn.microsoft.com/windows/win32/api/synchapi/nf-synchapi-createwaitabletimerexw)
  and its high-resolution flag, which std's sleep uses on Windows 10 1803 and later.
