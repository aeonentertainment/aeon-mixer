# 0003: One device backend per OS, chosen by measurement

**Status:** accepted, 2026-11-01

## Decision

`native/devices` holds one `Driver` per OS, behind the `hardware` feature. Each reports the
period and latency it got in `StreamInfo`.

- **macOS: the CoreAudio HAL through `objc2-core-audio`.** An IOProc on the device, with
  `kAudioDevicePropertyBufferFrameSize` set to the period. No AudioUnit sits between.
- **Windows: WASAPI through Microsoft's `windows` crate, event-driven, shared by default.**
  IAudioClient3's shared stream at the nearest period the shared engine allows. Exclusive opens
  when asked for, or when the shared stream cannot open at all: a mix format other than f32 at
  48 kHz, no IAudioClient3, or an engine that refuses the stream.
- **Linux: ALSA on the `hw:` device through our own bindings to libasound, loaded with
  `dlopen`.** An explicit period in a ring of two, written by an audio thread of ours that asks
  for SCHED_FIFO.

The hardware period is 128 frames, one block, unless the host asks for 64.

## Measured

measure-sink, one minute of silence per run at 48 kHz, on commit 9dd70a6. Latency is what the
OS reports. Underruns are what each backend can see: ALSA xruns, CoreAudio sample-time gaps,
WASAPI an empty shared buffer or, in exclusive mode, a wake more than 1.5 periods late.

| Machine                          | Output              | Mode      | Period asked, got | Latency | p99.9, max interval | Underruns | Realtime |
| -------------------------------- | ------------------- | --------- | ----------------- | ------- | ------------------- | --------- | -------- |
| MacBook Pro, arm64, macOS 26.6   | DisplayPort monitor | HAL       | 128, 128          | 11.2 ms | 2.75, 2.95 ms       | 0         | yes      |
|                                  |                     |           | 64, 64            | 9.8 ms  | 1.40, 1.45 ms       | 0         | yes      |
|                                  | built-in speakers   |           | 128, 128          | 20.0 ms | 2.80, 36.08 ms      | 2         | yes      |
|                                  |                     |           | 128, 128 (repeat) | 20.0 ms | 2.75, 2.90 ms       | 0         | yes      |
|                                  |                     |           | 64, 64            | 18.7 ms | 1.45, 1.60 ms       | 0         | yes      |
| Dell laptop, Windows build 26200 | Realtek speakers    | shared    | 128, 480          | 20.0 ms | 10.90, 10.99 ms     | 0         | MMCSS    |
| (Realtek behind Intel SST)       |                     | default   | 128, 480          | 20.0 ms | 10.85, 11.13 ms     | 0         | MMCSS    |
|                                  |                     | exclusive | 128, 144          | 9.0 ms  | 77.00, 77.00 ms     | 2216      | MMCSS    |
|                                  |                     | exclusive | 128, 144 (repeat) | 9.0 ms  | 76.19, 76.19 ms     | 2183      | MMCSS    |
|                                  |                     | exclusive | 64, 144           | 9.0 ms  | 77.01, 77.01 ms     | 1972      | MMCSS    |
| Dell Inspiron 7391, Ubuntu 26.04 | sof-hda-dsp analog  | hw direct | 128, 128          | 5.3 ms  | 3.85, 15.76 ms      | 13        | no       |
|                                  |                     |           | 64, 64            | 2.7 ms  | 2.60, 8.40 ms       | 36        | no       |

The Windows "default" row ran on 06962e7, after the default became shared, and shows what the
engine now opens there. The laptop has no MSVC tools, so its measure-sink was built with the
`x86_64-pc-windows-gnullvm` toolchain and llvm-mingw. CI builds the same code with MSVC.

## What the numbers say

- CoreAudio and ALSA gave the asked period every time. WASAPI gave what the driver allows: this
  laptop's shared engine runs only at 480 frames, and its exclusive minimum is 144.
- macOS is clean at both periods. The engine controls the small part of its latency: the
  DisplayPort monitor adds 8.5 ms of its own and the MacBook Pro speakers 17.4 ms. A room on a
  Mac plays through an external output.
- Windows exclusive delivered its events in bursts, about 0.25 ms apart with 77 ms gaps, at the
  right average rate. Either it glitches or the driver hides about 77 ms of buffering behind the
  9 ms it reports; the late-wake count cannot tell which. Shared at 10 ms ran clean and leaves
  other apps playing. So shared is the default, and exclusive no longer takes over when shared
  cannot reach the asked period: on the one Windows machine measured, exclusive missed the
  budget too. A Windows room that needs a shorter period needs a driver that offers small
  IAudioClient3 periods, or a device measured clean with `measure-sink --exclusive`.
- Linux glitches when the audio thread runs at normal priority: 13 underruns a minute at 128
  and 36 at 64 in a two-period ring. The device service on a Linux box needs realtime
  scheduling, from `LimitRTPRIO=` in its systemd unit or an rtprio limit for its user. This SOF
  card moves audio on a 1 ms tick, so 64-frame wakes alternate between 1 and 2 ms, and 64
  frames buys nothing on it.
- 64 frames ran clean only on macOS. 128 stays the default; 64 is for a device measured clean
  at it.

## Why these crates

- `objc2-core-audio` is generated from Apple's SDK headers, so selectors and struct layouts are
  Apple's own and nothing runs bindgen at build time. `coreaudio-rs` is built around AudioUnits,
  and an IOProc on the HAL needs none.
- `windows` is Microsoft's own binding, with IAudioClient3 and exclusive event mode at their
  real signatures. cpal has neither, as 0002 says.
- libasound by `dlopen`, because the `alsa` crate links it. A linked libasound stops the addon
  loading on a Linux with no ALSA, where an offline engine still has to run, and the build then
  needs ALSA's headers and pkg-config. `devices/src/alsa/asound.rs` binds the 31 functions the
  backend calls.

## Not measured yet

- Linux with realtime scheduling. On the LAN box, run
  `cargo run --release -p devices --features hardware --bin measure-sink -- --period 128` under
  a unit or limit that grants rtprio.
- Latency at the speaker. Every figure above is what the OS reports: ALSA reports nothing past
  its ring, and the Windows exclusive figure is suspect. `measure-latency` measures it with a mic
  at the speaker or a cable, as far as software can: decision 0006.
- A Windows driver that offers small shared periods, and a USB audio interface on any OS.
