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

At the opening the slot stays silent one window longer than it needs to fill, reads the trough
and drops the excess at once. Played instead, that excess would go a sample a block, sliding the
delay by up to a period in the first second of a song, under an echo canceller that has just
found the path: on a 480-frame input it settled 96 frames off and took out 1 dB instead of 26.

## The wait

The fill before a pull is not how long a sample waited. Deliveries come a period at a time and
pulls a block at a time, so a pull just after a delivery finds more in the ring than one just
before it, and its samples waited as much less. The wait itself holds still: the trough, an input
period, less a part of the step the two periods make, the greatest common divisor of the period
and the block. The slot reports the middle of that step. The tests check it against the real wait
on 64-, 128- and 480-frame inputs on either clock: within a step, and still for a minute at a time.

An output callback longer than a block pulls its blocks back to back: a 480-frame callback, WASAPI
shared's on the laptop in 0003, takes three or four at a wake, and the input delivers nothing
between them. So the fill before the first of them holds up to three blocks more than a
block-a-wake output would leave, all of it healthy. The slot counts those blocks in its headroom,
for priming, for what it calls a backlog, and for the wait. Counting the input period alone, it
dropped that healthy audio as backlog and then ran short: 282 gaps in 6 s under a 128-frame input.

## Numbers

From the tests in `engine/src/ingest/capture.rs`, an hour at 200 ppm either way with wakes up to
8 frames early or late. The wait runs from a sample reaching the input to its pull, the input's
period included; what the OS reports for the hardware comes on top.

| Input period | Input clock        | Target     | Wait            | Reported | Gaps         |
| ------------ | ------------------ | ---------- | --------------- | -------- | ------------ |
| 64           | the output's       | 144 frames | 3.00 to 4.33 ms | 3.67 ms  | 0            |
| 128          | the output's       | 144 frames | 3.00 to 5.67 ms | 4.33 ms  | 0            |
| 128          | 200 ppm fast       | 144 frames | 3.00 to 5.67 ms | 4.33 ms  | 0            |
| 128          | 200 ppm slow       | 272 frames | 5.67 to 8.33 ms | 7.00 ms  | 1, then none |
| 480          | the output's       | 144 frames | 12.33 to 13 ms  | 12.67 ms | 0            |
| 480          | 200 ppm either way | 176 frames | 13 to 13.67 ms  | 13.33 ms | 1, then none |

Where in its range a wait falls depends on the phase of the two clocks, which nothing here can
see. So a 128-frame mic costs the engine 3 to 5.67 ms before the output on one clock, and 5.67 to
8.33 ms on a slower one, against 22.31 to 24.96 ms for the `l16` path. A 480-frame input pays for
its period in full, so a Windows house mic wants a driver with short shared periods as much as its
output does (0003).

`measure-latency` (#61) gives the phone path's engine share offline. With a capture variant in its
rig it would give this one. Its device mode (#70) gives the sum through a real mic and speaker,
the number this table cannot.

## Echo cancelling

The canceller follows the latency its slot reports, which for a capture slot is the wait, still
until the target moves. From the closed-loop tests in `engine/src/ingest/capture_edges.rs`, a
living room's echo of the far end in a house mic:

| Input                      | Taken out, second 3, 5 to 9 | Path found                              |
| -------------------------- | --------------------------- | --------------------------------------- |
| 128 frames, output's clock | 15.3, 23.5 to 28.8 dB       | 624 frames, the room's 480 and the wait |
| 480 frames, output's clock | 16.0, 23.4 to 29.6 dB       | 1072 frames, the same                   |
| 128 frames, 100 ppm slow   | 10.9, 6.6 to 11.1 dB        | 752 frames, still moving                |

On its own clock the mic's echo of the room moves 4.8 samples a second until the slot's next move,
27 s on, and the canceller cannot follow a path that never holds still. It never makes the slot
louder. A house mic beside the speakers on its own clock, the commonest kind, gets partial
cancelling until the slot spreads its moves at the rate it has learned, one sample at a time
between slips, which would hold the echo's delay to a sample.

## What this does not do

- No resampling. A device that cannot give 48 kHz fails to open, as 8.3 has it.
- No timestamps. The capture thread's `at` could date the fill below a period and spread a slower
  input's correction ahead of its slip, taking about half a period off its mean. That wants a clock
  on the audio thread or a loop on the capture thread, and waits for a measurement that asks for it.
- No input period or output callback over 1024 frames, 21 ms. The ring is a fixed 4096 frames, so
  the audio thread never sizes it, and at 1024 of each with the target at its ceiling the healthy
  fill still fits. A device that grants more is refused when the input opens. Every backend
  measured in 0003 grants 480 or less.
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
