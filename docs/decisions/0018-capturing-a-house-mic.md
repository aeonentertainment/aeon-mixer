# 0018: A house mic is a capture slot with a ring and no resampler

**Status:** accepted, 2026-11-10, on simulation and the offline driver. No real input has run it;
the run a person owes is at the end.

## Decision

A mic plugged into the machine that hosts the engine is a `capture` slot:
`{ kind: "capture", device?, channel? }` in `MixerConfig.slots`, with ids from
`engine.inputDevices()`. Its input's capture thread pushes into an SPSC ring and the slot pulls a
block from it. There is no socket, no jitter buffer and no resampler. When the input runs on
another clock than the output, the slot moves its delay a sample at a time against a target fill,
each move spliced across a block by linear interpolation.

## Why a slot of its own

Today a house mic reaches the engine through a GStreamer sender over loopback as `l16`. Offline,
that path costs 22.31 to 24.96 ms before the output, 15 ms of it the playout delay meant for
Wi-Fi, before counting the sender's own 10 ms source buffer. A wire has no jitter to absorb.

The API is one slot kind, because a house mic belongs to a performance the way a phone does and
the adapter already binds one slot per house mic. `device` is optional, as `output.device` is,
so a box with one mic needs no configuration. `channel` is there because a mic on an
interface's second input is silent on channel 0; the sender today folds stereo to mono, which
halves a mono mic and adds the other input's noise. The input opens in `createMixer`, so a wrong
id rejects there as a taken port would, and stops at `destroy`.

## Two clocks

An input on its own crystal runs fast or slow against the output. Ordinary audio crystals hold
20 to 100 ppm; at 100 ppm the ring gains or loses 4.8 frames a second, 360 ms an hour.

What others do, and what each adds to the delay:

| Approach                                                   | Added delay                                                                             | Cost                                                                                                         |
| ---------------------------------------------------------- | --------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| Adaptive resampling, zita-ajbridge                         | the filter's half length, 16 to 96 samples; its author recommends 32 at 48 kHz, 0.67 ms | a polyphase filter per sample; a loop that settles over seconds to minutes                                   |
| Adaptive resampling, JACK's alsa_in and alsa_out           | libsamplerate's filter, plus a target delay set from the period and period count        | its man page says to give it some minutes to settle                                                          |
| Adaptive resampling, PipeWire's followers                  | a windowed-sinc filter's half length, the same order                                    | the same kind of buffer-level rate loop                                                                      |
| Drop or repeat a sample against a target fill              | none                                                                                    | a step per move, up to twice the signal's change between samples: a broadband click, 4.8 a second at 100 ppm |
| The same clock, an interface that is both input and output | none                                                                                    | none: the fill never drifts, so nothing is corrected                                                         |

Whatever corrects the rate, the ring has to hold its fill above a block at the moment before a
pull, and deliveries come a period at a time. Two clocks slide past each other, and the fill
before a pull steps by the greatest common divisor of the two periods each time their phases
cross: 128 frames, 2.67 ms, for two 128-frame streams; 32 for a 480-frame WASAPI input. A slower
input needs that much over the block at the trough, and timestamps would not remove it, only
spread it out.

So the slot takes the drop-and-repeat path, which adds no delay, and removes its click: a move
reads 129 or 127 frames into 128 by linear interpolation, a fractional delay sweeping across the
block. Nothing steps, and the cost is that block's high end, at worst -2 dB at 10 kHz where the
sweep passes half a sample. At 100 ppm, 4.8 blocks in 375 a second carry a move.

## The target

The slot holds its trough, the lowest fill before a pull over 128 blocks, at a target. It opens at
a block and 16 frames, 3 ms, the least that works on the output's clock and on most faster
inputs. A pull the ring cannot fill plays what it has and silence after, and raises the target by
what was missing and 16 frames, at most one input period over where it opened. A slower input
meets this once, the first time its period slips past the output's, tens of seconds in; after
that it never is short. Opening at the higher target instead would avoid that one gap and cost
every same-clock mic 2.67 ms for good. We chose the lower delay.

An empty ring is an input that stopped and moves no target. A fill more than a period and a
block over the target is a stall's backlog, dropped at once to a period over the target, which
leaves the next trough on or above it whatever the phase.

## Numbers

From the tests in `engine/src/ingest/capture.rs`, an hour at 200 ppm either way with wakes up to
8 frames early or late, the ring's share of the delay:

| Input period | Input clock        | Target     | Ring delay low, mean, high | Gaps         |
| ------------ | ------------------ | ---------- | -------------------------- | ------------ |
| 128          | the output's       | 144 frames | 3.00 ms throughout         | 0            |
| 128          | 200 ppm fast       | 144 frames | 3.00, 3.29, 5.67 ms        | 0            |
| 128          | 200 ppm slow       | 272 frames | 3.00, 5.75, 8.33 ms        | 1, then none |
| 480          | 200 ppm either way | 176 frames | 3.00, about 8.5, 13.67 ms  | 1, then none |

Before the ring sit the input's period, 2.67 ms at 128 frames, and what the OS reports for its
hardware. So a 128-frame mic on the output's clock costs the engine 5.67 ms before the output, and
a slower one 8.4 ms on average, against 22.31 to 24.96 ms for the `l16` path. A 480-frame input
pays for its period twice, in the wait and in the swing of the fill, so a Windows house mic wants
a driver with short shared periods as much as its output does (0003).

`measure-latency` (#61) gives the phone path's engine share offline. With a capture variant in its
rig it would give this one: a block of pull and the ring's 3 ms on one clock. Its device mode
(#70) gives the sum through a real mic and speaker, the number this table cannot.

## What this does not do

- No resampling. A device that cannot give 48 kHz fails to open, as 8.3 has it.
- No timestamps. The capture thread's `at` could date the fill below a period and spread a slower
  input's correction ahead of its slip, taking about half a period off its mean. That wants a clock
  on the audio thread or a loop on the capture thread, and waits for a measurement that asks for it.
- No sharing of one input between slots or between an armed mixer and a live one. On CoreAudio and
  WASAPI two slots may open one device; ALSA's `hw:` opens once.

## The run a person owes

Before the code merges, on each of a Mac, a Windows laptop and a Linux box, a person runs
`measure-capture` for an hour with a USB mic, whose clock is its own, and the room's output; and
`measure-latency device` (#70) with the house mic against a phone. The commands are in the code
PR. This record gains the table of what they print.

## Sources

- [zita-ajbridge](https://kokkinizita.linuxaudio.org/linuxaudio/zita-ajbridge-doc/quickguide.html)
  and its [man page](https://manpages.ubuntu.com/manpages/jammy/man1/zita-j2a.1.html): `-Q` is the
  filter's half length, its delay.
- [zita-resampler](https://kokkinizita.linuxaudio.org/linuxaudio/zita-resampler/resampler.html):
  `hlen` from 16 to 96, 32 for very high quality at 48 kHz.
- [alsa_in and alsa_out](https://man.archlinux.org/man/alsa_in.1.en): quality 0 to 4, a target
  delay from the period and period count, minutes to settle.
- [PipeWire's resampler options](https://dyn.manpages.debian.org/testing/pipewire-bin/pipewire-client.conf.5.en.html).
