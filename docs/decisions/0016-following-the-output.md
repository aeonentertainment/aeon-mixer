# 0016: The engine follows the room's output

**Status:** accepted, 2026-11-10, ahead of the code for #53. An output that will not start
leaves the song where it was, 2026-11-13, for aeon-karaoke#510. A start that hangs is refused
after 3 s, 2026-11-13, for #161. The open starts its output off the event loop, 2026-11-13, for
#164.

## The problem

The engine opened an output once, at `openEngine`, and kept it until `close`. On a Mac in a real
session the default output was the built-in speakers, muted, at the moment the engine opened. The
default then went back to the monitor the room listens through, and the mix stayed on the muted
speakers for the whole night. Nothing in the log said where the sound was going, and the
latency the engine reported, 20.0 ms, was the speakers' figure, against 11.2 ms on the monitor.

A lost output was no better. Section 5 said a device loss faults the engine and the host closes
and reopens it. Closing destroys the live mixer, so an unplugged cable ended the song, and on
CoreAudio nothing noticed the loss at all: the IOProc simply stopped being called.

## Decision

The engine follows the output, inside one `openEngine`, with the mixer and the song alive.

- **Which output.** An engine opened with no `output.device` plays on the OS default and moves
  whenever the default changes. One opened with a `device` is pinned: it plays there while that
  output exists, moves to the default when it goes, and moves back when it returns. Both are one
  rule: the engine plays on the pinned output if the OS lists it, and otherwise on the default.
- **How the host learns.** `engine.output` names the output playing now, its `id` and `name`
  beside its latency, at open and after every move. An `outputChanged` event fires after each
  move with the new `output` and the gap the move left, and with a gap of 0 when a refusal
  clears with no move. The host logs both.
- **Muted is not the engine's call.** A muted default is still where the OS sends every other
  app's sound, so the person in the room fixes it where they fix everything else. In the session
  above the mute was not the fault: the default had moved and the engine had not. Following the
  default puts the mix where the room is listening.
- **What a move costs.** The old stream stops, then the new one opens, so the room hears a gap.
  The audio thread does not run during it: the backing playhead holds and resumes where it was,
  every slot starts its stream again on the new clock, and the first 5 ms on the new output fade
  in. The new output's latency is in the event.
- **When the new output will not start.** The engine starts the output it left again, if the OS
  still lists it, and failing that the default, which only a pinned engine has not tried yet.
  With no pin the default is the output that refused, so the engine faults once the one it left
  fails too. `engine.output.refused` names the one that would not start, with what the OS said.
  The fallback counts as a move, with the gap the room heard, so the song carries on after one
  gap and `outputChanged` says so. The engine does not try the refused output again while the
  rule still picks it. Once the rule picks another output, as when the default changes or the
  refused output leaves the list, the refusal clears, and the next time the rule picks it, it is
  tried afresh.
- **When it gives up.** If none of the output it should play on, the one it left and the default
  will start within a second of the wake, the engine faults as before, and the host closes and
  reopens it. The second counts from the wake, and a start is never cut short for it: a start
  that hangs ends its try when the 3 s bound refuses it, so with starts that hang the fault comes
  once every try under way has been refused, not at the second.
- **When the first open fails.** `openEngine` rejects with `OutputUnavailableError`, whose `code`
  is `"output-unavailable"` and whose `device` names the output that would not start. The
  module-level `outputDevices()` lists the others with no engine open, so the host can open one
  (aeon-karaoke#511). An engine's own `outputDevices()` lists none on the timer clock by design: it
  plays on no output.
- **How long a start may take.** A start that has not returned after 3 s is refused as one that
  failed is, on every backend: `openEngine` rejects with `OutputUnavailableError`, and a move goes
  back to the output it left. The start carries on alone on a thread of its own until the backend
  returns. A stream that starts late is stopped there without playing a sample of the song, and
  one that fails lets go of what it held, the CoreAudio IOProc and the rate hold of decision 0023
  included. Until then the device is still held: on the wedged monitor, for the 10 s the HAL
  waits. A stream that called back within the 3 s is running, so the follower keeps it and waits
  for its start to return, however long that takes. A healthy output starts in tens of ms and a
  Bluetooth one can take a second or two, so 3 s refuses neither. The wedged monitor below took
  10.1 s. An output slower than 3 s is refused like one that hangs, until the rule picks it again.
- **The open never holds the event loop.** The start runs off Node's event loop, so a host that
  tries one output after another keeps answering through the 3 s each may take.
- **A stall is a loss, not a fault.** A stream that asks for no audio for 200 ms while its
  output stays listed is lost, on every backend, and reopened. A Mac asleep calls no IOProc, and
  by one Mac's logs neither does DarkWake, so a reopen that stays silent waits twice as long as
  the last before the next, up to 6.4 s, and the engine is never faulted for it. When the Mac
  wakes, the stream that is open plays on. The wait is on the monotonic clock, which on macOS
  and Linux stands still while the machine sleeps.

### The case that set the rule

On a Mac on 13 November 2026, the default moved mid-song to a DisplayPort monitor whose audio
had stopped starting for every app: `afplay` failed with `AudioQueueStart` returning `'stop'`,
`kAudioHardwareNotRunningError`, until the monitor was power-cycled (aeon-karaoke#510). The
follower had already stopped the stream it was playing. It retried the monitor every 100 ms, and a
second later the engine faulted and the song ended, with the output it left still listed and
working. With the monitor healthy, moves on the same Mac worked, with gaps of 41 ms to the MacBook
Pro speakers and 194 ms back to the monitor. A move whose output starts is unchanged, so those
gaps stand.

### The case that set the bound

The same monitor, wedged again, took 10.1 s to refuse: `openEngine` rejected 10.1 s after it
began, and Aeon Karaoke's desktop app opened another output 0.17 s later (#161). The time is
CoreAudio's own. In coreaudiod's log each start shows `IOWorkLoopInit … starting`, then 10 s
later `IsTimeRunning_Helper: Device … is not running`, and `AudioDeviceStart` returns `'stop'`.
The monitor read 48 kHz already, so no rate change waited, and setting the IO buffer is a
property write that returns at once. The HAL gives a device's clock 10 s to start running, and
Apple's `AudioServerPlugIn.h` says a driver's `StartIO` "can take as long as necessary". Only the
caller can bound a start.

No property says beforehand that a start will hang. By Apple's `AudioHardware.h`:

| Property                                       | What Apple says it means                                         | Why it cannot catch a wedged output                                     |
| ---------------------------------------------- | ---------------------------------------------------------------- | ----------------------------------------------------------------------- |
| `kAudioDevicePropertyDeviceIsAlive`            | 1 when ready and available, 0 when unusable and about to go away | the monitor stayed listed and every app got as far as starting it       |
| `kAudioDevicePropertyDeviceIsRunningSomewhere` | 1 when any process runs the device                               | 0 on every idle output, healthy or not                                  |
| `kAudioStreamPropertyIsActive`                 | the stream is enabled and doing IO                               | it says the stream is switched on, not that the device's clock will run |
| `kAudioDevicePropertyHogMode`                  | the pid that has the device to itself, or -1                     | `afplay` failed the same way, so no process held it                     |

A start under a bound of our own is the one thing that tells them apart. The repro on
aeon-karaoke#510 records the time to refuse on the real monitor.

## How the OSes say an output changed

| OS                | Default changed                                                             | Open device lost                                                                                                                                                        | Where the signal arrives                                                                                                                                            |
| ----------------- | --------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| macOS             | `kAudioHardwarePropertyDefaultOutputDevice` listener on the system object   | `kAudioHardwarePropertyDevices` changes and the device leaves the list; `kAudioDevicePropertyDeviceIsAlive` goes to 0; the IOProc stops being called and nothing errors | the HAL's notification thread, once `kAudioHardwarePropertyRunLoop` is set to null; left alone, the HAL posts to the main run loop, which a Node process never runs |
| Windows           | `IMMNotificationClient::OnDefaultDeviceChanged`, three times, once per role | `OnDeviceStateChanged` and `OnDeviceRemoved`; the stream's calls return `AUDCLNT_E_DEVICE_INVALIDATED`                                                                  | a system thread; the callback must not register or unregister callbacks, or release the last reference to an MMDevice object                                        |
| Linux, ALSA `hw:` | no signal: there is no default above the hardware device                    | `snd_pcm_writei` and `snd_pcm_wait` return `-ENODEV`, which `snd_pcm_recover` cannot undo; a hung wait is caught by the follower's 200 ms stall                         | the engine's own audio thread                                                                                                                                       |

Sources: Apple's `AudioHardware.h` and the coreaudio-api list on
[the run loop](https://lists.apple.com/archives/coreaudio-api/2010/Mar/msg00121.html) and
[IsAlive](https://lists.apple.com/archives/coreaudio-api/2010/Aug/msg00227.html);
[OnDefaultDeviceChanged](https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-immnotificationclient-ondefaultdevicechanged)
and [IMMNotificationClient](https://msdn.microsoft.com/en-us/library/dd371417);
mpv's [ALSA output](https://git.tjdev.de/mirror/mpv/src/commit/b7325b2f64eb66da255b9cc28b2b70b96578883d/audio/out/ao_alsa.c)
on `ENODEV`, and SDL's [bug 3599](https://bugzilla.libsdl.org/show_bug.cgi?id=3599) on a wait
that hangs after an unplug.

PipeWire and PulseAudio track a default sink and move their streams to it, but decision 0003
opens `hw:` directly, which a running sound server holds busy. On a LAN box with no sound server
the only move is the loss of a USB card, and its default is the first `hw:` device, as today.

## What others do

- **cpal** reports `DeviceNotAvailable` through its error callback and leaves rebuilding the
  stream to the app. In 0.18 its macOS backend sits on the AUHAL default-output unit, which
  reroutes on its own and reports `DeviceChanged`.
- **PortAudio** freezes the device list at `Pa_Initialize`. Seeing a new device means
  `Pa_Terminate` and `Pa_Initialize`, which closes every open stream
  ([its hotplug notes](https://github.com/PortAudio/portaudio/wiki/HotPlug)).
- **RtAudio 6** raises `RTAUDIO_DEVICE_DISCONNECT` and leaves the reopen to the app. Its
  CoreAudio listeners were once never removed and filled the HAL's slots
  ([5045d3b](https://www.github.com/thestk/rtaudio/commit/5045d3b60fd4e431fcbb8246b12e717f41de7382)).
- **JUCE** listens for device-list changes and, since 5.4, falls back to the default when the
  chosen device goes. Its forum has years of reports of it not following a new default, and of
  apps patching the fallback out.
- **cubeb**, Firefox's audio library, reinitialises its stream on a default change on both
  macOS and Windows. Its history is the warning: a reinit racing a destroy deadlocked inside
  CoreAudio ([bug 1347067](https://bugzilla.mozilla.org/show_bug.cgi?id=1347067),
  [bug 1572273](https://bugzilla.mozilla.org/show_bug.cgi?id=1572273)), and a stopped stream was
  restarted by a change it should have ignored
  ([bug 1134078](https://bugzilla.mozilla.org/show_bug.cgi?id=1134078)).

The lessons the design takes: never stop or start a device inside an OS listener; do the move on
a thread that owns the stream; treat the three Windows calls, and the pair of macOS changes an
unplug raises, as one; and reconcile against what the OS lists now rather than act on what a
notification says.

## How it works

A follower thread owns the running stream. The OS listeners do one thing: wake it. A stream that
sees its device go marks the loss in its stats, which wakes it too. On each wake the follower
lists the outputs, works out where the engine should play by the rule above, and moves only if
that differs from where it plays. A stream that is lost plays nowhere, so the follower reopens
even the same output. Three wakes for one change cost two lists and no moves.

Moving stops the old stream, which hands the engine's audio thread back, and starts the new
stream with it. Nothing crosses to the audio thread to do this: the follower holds it only while
no stream runs, so section 2's five crossings are unchanged. A start that fails hands the audio
thread back too, so the follower starts the output it left with it.

## The numbers

These are the parts of a gap, not a measurement; each move reports the gap it left, and #53's
manual run per OS records them.

| Part                                  | Expected                                                                    | Bound                                               |
| ------------------------------------- | --------------------------------------------------------------------------- | --------------------------------------------------- |
| A default change reaches the follower | under 1 ms after the OS posts it                                            |                                                     |
| A loss reaches it                     | macOS and Windows: when the OS posts it; ALSA: the next period, 2.7 ms      | 200 ms without a callback, on every backend         |
| Stop the old stream                   | a few ms                                                                    | a stalled stream's 200 ms, which the stop waits out |
| Open the new stream at 48 kHz         | tens of ms, measured by #53; a Bluetooth output a second or two             | 3 s, then refused; a CoreAudio rate change has 1 s  |
| The new output's own latency          | 11.2 ms on the monitor, 20.0 ms on the MacBook Pro speakers (decision 0003) |                                                     |

The bound that matters is 1 s. A command sent during the gap is acknowledged after it, and the
binding rejects a command unanswered for a second, which ends the song. A move that takes longer
than that ends the song as a fault would have; a move that takes 100 ms does not. That is why the
follower calls a stream lost after 200 ms without a callback, on every backend, where WASAPI and
ALSA used to wait 1 s and CoreAudio never noticed: twenty of WASAPI's 10 ms shared periods, and
the worst of it leaves most of the second for the open. The cases that can still overrun are a
CoreAudio device away from 48 kHz, whose rate change is given a second of its own, and an output
slow to start, such as Bluetooth. Both start in the end. An output that never starts costs 3 s
of silence before the song goes back to the output it left. A command sent in those 3 s still
ends the song a second in, as it did in the 10 s before the bound, so the bound saves the room
the wait but not a song that a command lands in.

## Rejected

- **Ask the HAL whether the device can run, and skip the start.** The table above: no property
  Apple documents tells a wedged output from a healthy idle one.
- **Refuse after 1 s, the second a command waits.** A Bluetooth output that takes a second or two
  to start would be refused every time, and the room could never play through it.
- **Cancel the hung start from another thread.** CoreAudio documents no way to stop an
  `AudioDeviceStart` in progress, and cubeb's deadlocks above came from racing calls into one
  device. The start finishes alone, and whatever it left is dropped there.
- **Wait as long as the backend does.** What 0.7.0 did: 10.1 s on the monitor, at open and on a
  move.
- **Hold silence and retry the refused output, doubling up to 6.4 s, as a stall does.** A command
  waits a second for its ack while no stream runs, so the song ends at the first command anyway,
  and the room hears nothing while a working output sits listed.
- **Fall back to any output that starts.** A Mac lists virtual outputs, such as a call app's or a
  recorder's, that the room cannot hear. The output it left is the one the room was hearing a
  moment ago, and the default is where the rule goes with no pin. With those gone too, the engine
  faults and the host chooses.
- **Start the new output before stopping the old.** One audio thread renders into one stream, and
  two at once means two clocks, as with the crossfade below.
- **Retry a refused output on a timer while playing elsewhere.** Each try costs the room a gap on
  the output that works. The refused output is tried again when the OS says something changed.
- **Fault and let the host reopen.** What the engine did. Reopening destroys the live mixer, so
  every unplug ends the song, and nothing ever follows a new default.
- **Follow a loss, never a default change.** The session above was a default change.
- **Let the OS reroute.** The AUHAL default-output unit on macOS, and WASAPI's automatic stream
  routing through `DEVINTERFACE_AUDIO_RENDER` on Windows, both follow the default themselves.
  Decision 0003 puts an IOProc on the HAL with no AudioUnit, to control the buffer. GStreamer's
  WASAPI plugin found IAudioClient3 unavailable on the virtual default device
  ([its merge request](https://gitlab.freedesktop.org/gstreamer/gst-plugins-bad/-/merge_requests/1332)),
  and the shared low-latency stream is what Windows' numbers rest on. Either way the host is
  never told the new latency.
- **Crossfade from the old output to the new.** Two devices are two clocks. Rendering both at
  once needs a ring and a resampler between them on the mic path, a cost against the latency
  budget for the few seconds a move happens in a night. When the old device is lost there is
  nothing left to fade.
- **Fade out on the old output before stopping it.** The room is listening to the new one, and
  the fade would need a signal into the running audio thread that nothing else needs.
- **A listener on each open device.** `kAudioDevicePropertyDeviceIsAlive` says the same thing as
  the device list, and a listener per device is one more registration per move to remove safely.
  The follower registers its listeners once, for the engine's life.
- **Fault a stream that stays silent for a second,** as a lost output with nothing that will
  start does. A Mac asleep on battery wakes into DarkWake through the night, by its logs with no
  IOProc called, and every song left playing would end there. Reopening ever more slowly costs
  about ten opens a minute while it is awake.
- **Report a muted output.** The OS shows mute already, and a mute flag would need one more
  listener per device. Left for when a host asks.
