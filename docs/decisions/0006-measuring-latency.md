# 0006: How we measure latency, and what a figure can say

**Status:** accepted, 2026-11-05. The engine's share is measured. The hardware numbers are owed by a
person on each OS.

## Decision

`measure-latency` (`native/tools/measure-latency`) has two modes built on one signal and one
detector.

- **The signal is a Hann-tapered linear chirp,** 0.5 to 7.5 kHz over 200 ms. The detector is a
  matched filter's envelope. It takes the earliest peak within 20 dB of the strongest and at most
  30 ms ahead of it, placed between frames by a parabola through the log.
- **Offline mode measures the engine's share.** The chirp goes through a sender, the real jitter
  buffer, decoder, slot and master on the offline driver, for every Opus frame size, l16 and push. It
  opens nothing, and a test holds its numbers: adding 64 frames to the jitter buffer's anchor fails
  that test.
- **Device mode measures the output's share.** The chirp goes through a real slot and a real output,
  and a real input records it. The two streams are compared on the host clock, each through a line
  fitted to its own wakes. It refuses to open anything without `device --open-device`.

The mic path's figure is the offline total plus the device figure.

## A loopback cannot be split, and the tool does not pretend to

A loopback measures output latency plus input latency. No software measurement can separate them,
and this is arithmetic. With outputs O1, O2 and inputs I1, I2, every run gives one sum. All four
sums are unchanged if you add a constant to both outputs and subtract it from both inputs, so more
devices and more runs never pin down a single O. Only something outside the sound card can, such as
the Teensy that [WALT](https://github.com/google/walt/blob/master/docs/AudioLatency.md) uses, and
that synchronises to about 1 ms. Android's own documentation
[measures round trip instead](https://source.android.com/docs/core/audio/latency/measure). Paul Davis
[says the same of Ardour](https://discourse.ardour.org/t/understanding-latency-figures/109819):
"normally we can only measure the sum and divide by two on the assumption that they are equal". That
assumption fails on the MacBook speakers, which decision 0003 puts at 17.4 ms of their own.

So the tool does three things.

1. **It takes the input's buffering out by construction.** A capture thread wakes after its frames
   came off the hardware and never before. A line under all of an input's wakes dates each frame to
   the earliest moment the OS could have delivered it, so the input's period drops out of the
   figure. Its slope is the edge of the wakes' lower convex hull at their mean, the estimate Moon,
   Skelly and Towsley give for a clock's skew from one-way delays, so wakes that grow later over a
   run cannot tilt it and date the early frames before they arrived. A test moves the input from 64 to 960 frames and the figure holds to 100 µs.
2. **It prints what is left as a bound.** The ADC and the driver hold a frame before any wake can
   happen. That share stays in, so the figure is `output at most`. Next to it sits the same figure
   less what the OS reports for the input, labelled as trusting the OS.
3. **It says differences are exact.** Two runs on one input cancel that input's share. 64 frames
   against 128, a change on the mic path, or one output against another are free of the capture
   side, and those differences are what most later issues need.

## What makes it refuse

A reading it cannot stand behind prints why instead of a number.

- The input is silent, or the first sound stands under 30 dB over the noise.
- The input lost frames, since its count of frames no longer says the time.
- The figure moves more than 0.75 ms across one opening's chirps. An output that drops audio and
  carries on keeps counting frames as if nothing happened, so its chirps either side of the dropout
  disagree by the time lost. One 64-frame period is 1.33 ms. This reads the sound rather than the
  backend's underrun count, because the count misses what the OS does not report, and in WASAPI
  exclusive it counts every late wake of a driver that signals in bursts. Decision 0003 needs that
  mode measured, and a refusal on the count would have ruled it out. A dropout after the last chirp
  touches nothing the figure rests on and is not refused.

## What the signal choice rests on

The tools in use, read at their source where the source is public:

| Tool                                                                                                                                                    | Signal and detector                             | What it costs here                                                                                                                                                                                                                                   |
| ------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `jack_iodelay`, `alsa_delay`, Ardour ([`mtdm.cc`](https://github.com/Ardour/ardour/blob/master/libs/ardour/mtdm.cc))                                    | 13 tones, delay resolved from their phases      | Its [manual](https://manpages.ubuntu.com/manpages/jammy/man1/alsa_delay.1.html) claims 1/1000 of a sample and assumes one frequency-flat path. A room is not one. I did not test this; the reasoning is that a reflection rotates every tone's phase |
| Superpowered ([`latencyMeasurer.cpp`](https://github.com/superpoweredSDK/SuperpoweredLatency/blob/master/Android/app/src/main/jni/latencyMeasurer.cpp)) | 1 kHz tone, input 24 dB over ambient            | See the table below                                                                                                                                                                                                                                  |
| OboeTester ([`LatencyAnalyzer.h`](https://github.com/google/oboe/blob/main/apps/OboeTester/app/src/main/cpp/analyzer/LatencyAnalyzer.h))                | 500 ms of random bits, normalised correlation   | The closest relative. It takes the strongest peak, so a reflection louder than the direct sound wins, and it reports whole frames                                                                                                                    |
| Oblique RTL Utility                                                                                                                                     | A cable and a pulse; its detector is not public | I found the setup described and nothing on how it detects                                                                                                                                                                                            |

I found no Apple-published method. The nearest is a
[January 2020 coreaudio-api thread](https://lists.apple.com/archives/coreaudio-api/2020/Jan/msg00006.html),
which I could read only through a search summary: a developer found the session's reported latencies
to be estimates, about 8 ms short against a loopback. Networked-music papers define mouth-to-ear
delay ([one](https://arxiv.org/pdf/1808.09405) splits it into sound card, processing and network) but
I found none that describes its measurement in enough detail to use.

I ran the candidates on a synthetic room rather than quote folklore: a direct sound 10.5 dB under a
reflection 4 ms behind it, a second reflection at 9 ms, every signal at a peak of -12 dBFS, white
noise at the level given, 60 trials. Error is against the direct sound, in frames of 20.8 µs. These
are simulations, and a real room should be expected to be worse.

| Noise, dBFS | Click, first over -20 dB | 1 kHz tone, +24 dB rule | Chirp 50 ms | Chirp 200 ms |
| ----------- | ------------------------ | ----------------------- | ----------- | ------------ |
| -70         | 0                        | 1                       | 0           | 0            |
| -50         | 0                        | 5                       | 0.01        | 0.00         |
| -40         | 0                        | 196, took the echo      | 0.04        | 0.01         |
| -30         | 3000, took the noise     | 442                     | 0.13        | 0.04         |
| -20         | 3000, took the noise     | never found             | 0.40        | 0.13         |

A click holds until the noise reaches it and then fails all at once. The chirp has 32.6 dB more
energy at the same peak, and it degrades by fractions of a frame.

MLS was the real rival, and it beat me on the first comparison. At equal peak it has 42.1 dB over a
click against the chirp's 32.6, and its errors were as small on one clock, even with a bending
speaker. Three things decided against it:

| Test                                       | Chirp 200 ms            | MLS 341 ms                   |
| ------------------------------------------ | ----------------------- | ---------------------------- |
| Through Opus at 5 ms and 64 kb/s           | 0.02 frames off, 0.0 dB | 4.4 frames off, 20.4 dB down |
| Peak over floor, one clock, -40 dBFS noise | 47 dB                   | 37 dB, its own self-noise    |
| Same, input 300 ppm from the output        | 46 dB                   | 12 dB                        |

The first is decisive, since offline mode has to read through the codec and one signal for both modes
keeps one detector. The second is MLS outside the periodic use it is designed for. The third matters
on any rig with a USB mic.

The chirp's cost is that clock drift moves its peak, by delay-Doppler coupling: 1.3 frames at
100 ppm and 4 frames at 300 ppm, so 83 µs at worst. That is small against figures quoted to 0.1 ms,
and it is left uncorrected.

The rest of the signal follows from the product. 0.5 to 7.5 kHz is the voice band, so the delay
measured is the voice's delay, not a tweeter's. The taper puts the largest sidelobe at -46.8 dB, so
the 20 dB onset rule never takes one for an arrival. The main lobe is 0.29 ms wide at -3 dB, so two
paths closer than about 10 cm of air merge into one.

## What it says

Offline, 15 ms playout, in ms:

| Path                 | Sender gathers | Sender encoder | Engine holds   | Total          |
| -------------------- | -------------- | -------------- | -------------- | -------------- |
| rtp 20 ms            | 20.00          | 6.50           | 15.01 to 17.66 | 41.51 to 44.16 |
| rtp 10 ms            | 10.00          | 6.50           | 15.01 to 17.66 | 31.51 to 34.16 |
| rtp 5 ms             | 5.00           | 6.50           | 15.02 to 17.67 | 26.52 to 29.17 |
| rtp 2.5 ms           | 2.50           | 6.50           | 15.02 to 17.67 | 24.02 to 26.67 |
| rtp 5 ms low-delay   | 5.00           | 2.50           | 15.02 to 17.67 | 22.52 to 25.17 |
| rtp 2.5 ms low-delay | 2.50           | 2.50           | 15.02 to 17.67 | 20.02 to 22.67 |
| l16 7.3 ms           | 7.29           | 0.00           | 15.02 to 17.67 | 22.31 to 24.96 |
| push 10 ms           | 10.00          | 0.00           | 2.02 to 2.67   | 12.02 to 12.67 |

The engine adds nothing on the mic path beyond the playout delay and the wait for the next block, to
within 0.02 ms. The range is that wait: the first packet anchors the stream, so where it lands in a
block sets the delay for the whole stream. Most of what the path spends is the sender's. Going from
20 ms voice frames to 2.5 ms low-delay frames saves 21.5 ms, far more than anything left in the
engine.

Hardware numbers go here when they exist. The rig is a mic at the speaker where possible, since a
powered speaker's own processing is delay the room hears and a cable never sees.

## Not measured yet

- Every hardware figure: nobody has run device mode. Its analysis is tested only on a simulated
  device, with a known answer, on the offline driver.
- How far the input's residual reaches on real hardware. Running one output with two different
  inputs gives the difference between their residuals, the closest software can get.
- The whole path from a singer's mouth. A phone's capture and the Wi-Fi are outside what this tool
  can reach.
