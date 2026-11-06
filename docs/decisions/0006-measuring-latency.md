# 0006: How we measure latency

**Status:** accepted, 2026-11-05. This covers the engine's share, which is measured. The output's
share on real hardware is #40's next step and comes with its own numbers.

## Decision

`measure-latency` (`native/tools/measure-latency`) sends a known signal through the real mic path on
the offline driver and times where it comes out.

- **The signal is a Hann-tapered linear chirp,** 0.5 to 7.5 kHz over 200 ms. The detector is a
  matched filter's envelope. It takes the earliest peak within 20 dB of the strongest and at most
  30 ms ahead of it, placed between frames by a parabola through the log.
- **It runs the real path.** The chirp goes through a sender, the real jitter buffer, decoder, slot
  and master, for every Opus frame size, l16 and push. It opens nothing, and a test holds its
  numbers: adding 64 frames to the jitter buffer's anchor fails that test.

Offline alone needs no taper or onset rule, since it has no room in it. They are here because #40's
hardware figure has to use the same signal and detector to be added to this one, and a real room is
what they were chosen for.

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

The first is decisive, since this mode has to read through the codec and one signal for every mode
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

## Not measured yet

- Anything after the master. These figures stop at the engine's output, with no device, network or
  room in them.
- A phone's capture and the Wi-Fi, which are outside what this tool can reach.
