# 0023: A device's rate is put back as the engine found it

**Status:** accepted, 2026-11-13. Hygiene found while chasing aeon-karaoke#510; not its fix.

## The problem

The engine never resamples, so on CoreAudio it sets a device's
`kAudioDevicePropertyNominalSampleRate` to 48 kHz when the device is elsewhere. That rate is the
device's, for every app, and it outlives the process: Audio MIDI Setup shows it afterwards. The
engine never put it back, so a device found at 44.1 kHz stayed at 48 kHz after one song.

This is not the cause of the wedged DisplayPort monitor in aeon-karaoke#510. That monitor reads
48000 Hz, so the engine never changed its rate. Nothing found so far ties that wedge to the
engine.

## Decision

The last of this process's streams to let go of a device puts back the rate the first one found.

- **When.** A stream lets go when it stops, for any reason: `close`, a move to another output, a
  reopen after a loss or a stall, and a start that fails after the rate moved. A rate that never
  lands within the second it is given fails the start, and if it lands late it is put back too.
- **Shared devices.** An output and a house mic can be one USB interface, one device. The process
  counts its streams on each device, so the input keeps 48 kHz while the output moves away, and
  the last one out restores it. The count lives in `devices/src/rate_holds.rs`, behind a
  `NominalRate` trait, and its tests run on a fake HAL that records every set.
- **What it puts back.** The rate found, whenever the device reads otherwise, so a late landing is
  undone. If a person changed the rate mid-song, the song was already playing at the wrong speed.
- **What it cannot.** A process that is killed or crashes leaves 48 kHz, as before.

## The costs

A device already at 48 kHz, which is most Mac outputs and the monitor in #510, is never set and
costs nothing. A device elsewhere costs one more rate change each time a stream on it stops: a
reopen of the same device after a loss or a stall puts the rate back and moves it again. The
HAL's change lands asynchronously, in a few ms by cpal's account and within the 1 s the engine
allows, and a move is already bounded by the same second (decision 0016).

## What others do

- **cpal** sets the nominal rate when a stream asks for another, after checking the device's
  `kAudioDevicePropertyAvailableNominalSampleRates`, and waits on a listener for it to land. It
  never puts it back (`src/host/coreaudio/macos/format.rs`, `set_sample_rate`, at 13c485fc86).
- **PortAudio** changes the rate only when the app passes `paMacCoreChangeDeviceParameters`, and
  otherwise lets the AUHAL unit convert. `pa_mac_core.c` has no restore either.
- **Apple's AUHAL unit and AudioQueue**, which `afplay` uses, convert between the app's format and
  the device's, so apps on them never touch the rate.

## Rejected

- **Change the rate only when no other process runs the device**
  (`kAudioDevicePropertyDeviceIsRunningSomewhere`). A browser tab holding a 44.1 kHz output would
  leave the engine with nothing to play on. The HAL converts for the other apps when the rate
  moves.
- **Never change the rate, and resample the output.** That breaks the rule that the engine never
  resamples, and needs its own decision. The filter alone costs its half-length against the
  latency budget, 16 frames or 0.33 ms for a 32-tap polyphase filter at 48 kHz. It runs on every
  block of the mix, and on every capture slot whose input sits at another rate. Nothing measured
  shows the rate change hurting a device, and putting the rate back removes its lasting effect.
  The lead in #510 that argued for resampling went away when the monitor read 48 kHz.

## The other properties the CoreAudio backend sets

| Property                                            | Where                    | Restored | Why                                                                                                                                                                                                                                                                 |
| --------------------------------------------------- | ------------------------ | -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `kAudioDevicePropertyNominalSampleRate`             | output and input devices | yes      | the device's, for every app, past the process                                                                                                                                                                                                                       |
| `kAudioDevicePropertyBufferFrameSize`               | output and input devices | no       | the asked period, clamped to `kAudioDevicePropertyBufferFrameSizeRange`, so it is never out of the device's range. AUHAL apps get 512 frames by default (Apple's TN2321). `AudioHardware.h` does not say whether it outlives the process; #510's repro separates it |
| `kAudioHardwarePropertyRunLoop`                     | the system object        | no       | this process's notification thread, gone with it                                                                                                                                                                                                                    |
| listeners on the default output and the device list | the system object        | yes      | `Watch` removes both when the follower drops, a failed open included                                                                                                                                                                                                |
| the IOProc                                          | output and input devices | yes      | `Io` and `CaptureIo` stop and destroy it on drop, which a failed `AudioDeviceStart` reaches too                                                                                                                                                                     |

Sources: [Apple's TN2321](https://developer.apple.com/library/archive/technotes/tn2321/_index.html)
on the IO buffer, `AudioHardware.h` and `AudioHardwareBase.h` in the macOS SDK, cpal's
[`format.rs`](https://github.com/RustAudio/cpal/blob/13c485fc8622e6f90eb2d0848df3062b3e27706d/src/host/coreaudio/macos/format.rs)
and PortAudio's
[`pa_mac_core.c`](https://github.com/PortAudio/portaudio/blob/master/src/hostapi/coreaudio/pa_mac_core.c).
