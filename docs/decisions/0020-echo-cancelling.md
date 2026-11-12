# 0020: The echo canceller runs in the slot's block, and phones get a howl guard

**Status:** accepted, 2026-11-11, on the offline driver and on Aeon Karaoke's own synthetic bleed.
The canceller and the guard ship in separate PRs.
No real room, speaker or phone has run it; the runs a person owes are at the end.

## Decision

Two slot stages, each off until a host turns it on, both before the corrector:

- **`echo-cancel`** takes the room's echo of the master out of the slot's input. It is SpeexDSP's
  multidelay block frequency-domain canceller, ported to Rust and run in the engine's own
  128-frame block, with a delay finder in front of it after AEC3's. It adds no delay.
- **`howl-guard`** ducks the slot by 12 dB when its loop through the speakers runs away. It looks
  at the slot's voice on the side and moves the gain within the block it decides in, so it adds no
  delay either.

Off, either one hands the slot over bit for bit and does no work. `stats()` reports what the
canceller found and how often the guard ducked. A house mic in a room with speakers wants both. A
phone wants the guard; whether it wants the canceller too is the bleed run's to say.

## Why no delay

A canceller subtracts an estimate of the echo, and the estimate is the reference through a filter.
It holds audio back only when it must wait for reference it has not seen yet. Here the reference is
the master, which the engine rendered before the echo of it could reach any mic: an echo is at least
the output's latency late, and the output latency is at least a block. So every reference sample a
block's estimate needs is already in hand, and the canceller can work in whatever block the slot
does. In the slot's 128 frames, it holds nothing back.

AEC3 cannot do this here. Its API takes 10 ms frames, and an engine of 128-frame blocks feeding it
480-frame frames needs a FIFO of at least 448 frames, 9.33 ms, on the mic path. Aeon Karaoke counts
10 ms for it in `PipelineLatency.ts`. Against a 20 to 30 ms budget (0008), that is a third of it.

## What others do, and what each costs

| Approach                                        | Added delay on the mic path                                            | Echo removed                                                                          | Double talk                                                       | CPU                                                    | Licence      |
| ----------------------------------------------- | ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------- | ----------------------------------------------------------------- | ------------------------------------------------------ | ------------ |
| WebRTC AEC3, as GStreamer's `webrtcdsp` runs it | 9.33 ms at least, from 10 ms frames; Aeon Karaoke counts 10            | 19.2 to 35.2 dB on `aec:measure`; 25.2 to 41.2 with its noise suppressor              | the singer 0.5 to 3.0 dB down; 1.1 to 4.3 with it                 | not measured here                                      | BSD-3-Clause |
| SpeexDSP MDF, ported here, in the slot's block  | none                                                                   | 30.4 to 39.4 dB on `aec:measure`'s own files                                          | the singer 0.0 to 0.1 dB from untouched                           | 1.5% of an M2 Pro core per slot, 40 us a block         | BSD-3-Clause |
| DTLN-aec, a neural canceller                    | 24 ms, a 32 ms window on an 8 ms hop, by the AEC Challenge's own count | not compared                                                                          | not compared                                                      | 0.97 to 3.06 ms per 8 ms hop on desktop i5s, its paper | not weighed  |
| ICASSP AEC Challenge entries                    | capped at 20 ms in 2023 and 40 ms in 2022, so up to the whole budget   | not compared                                                                          | not compared                                                      | under real time on a quad-core i5                      | not weighed  |
| Frequency shifting, 5 Hz                        | its Hilbert filter's, unmeasured                                       | none; gains 5 to 8 dB before feedback, about 6 dB acceptable                          | shifts the singer's pitch, which the corrector and the score read | small                                                  |              |
| Notch-based suppression                         | none broadband: a biquad per howl                                      | none; up to about 10 dB more gain before feedback                                     | notches whatever note howled                                      | an FFT a hop                                           |              |
| Gain reduction, Patronis's AGC                  | none                                                                   | none; a loop turned down far enough is stable, so suppressors keep it as their rescue | the whole slot ducks                                              | an FFT a hop                                           |              |

The neural cancellers fail on delay before anything else: either one alone spends the budget. AEC3
is the strongest system on the list. It pairs a linear filter with a residual echo suppressor that
works on its frames, and its "aec only" figures include both. On Aeon Karaoke's synthetic bleed,
which is linear and has no room, the MDF's linear filter alone removes more and leaves the singer
alone.

## What the exact reference buys

GStreamer's `webrtcechoprobe` timestamps the playback and AEC3 estimates how far the capture lags
it. Here:

- **The reference is what the device got**, after the clipper, the reactions in it (section 11),
  sample for sample. Nothing estimates the render side.
- **The canceller follows its source's own moves.** A jitter buffer that steps its delay reports
  it, and the canceller moves its reference by the same frames two blocks later instead of finding
  the echo again. Measured: 22.2 dB removed in the second before a 5 ms step, 24.8 dB from 20 ms
  after it.
- **It adapts without the singer's own voice.** A singer is in the master, so in the reference, and
  a sung vowel is easy to predict from its own past: a filter adapted on the whole master learns to
  take the singer out with the echo. Feedback cancellers in hearing aids know this as the
  closed-loop bias. The engine knows each slot's share of the master exactly, so the echo estimate
  convolves the whole master, own voice included, and the adaptation sees the master less that
  slot's share. Measured on a house mic with the singer in the loop: adapting on the whole master
  left the singer 0.87 dB down and the filter at 3 to 13 dB for the four seconds after they
  stopped; leaving their voice out, with the double-talk hold below, leaves them 0.02 dB from
  untouched and the filter at 23 dB the second after.

A delay finder still has to find the rest of the path: the output's latency, the air, and for a
phone its capture and the network. It is AEC3's method: a matched filter on low-passed copies of
mic and master decimated to 3 kHz, adapted by normalised LMS, its largest tap the lag, searched to
515 ms; AEC3's own matched filters reach about 600 ms. A lag counts only once the filter predicts
30% of the mic and its tap leads every other tap more than 8 away by 3 dB, for 160 ms. The second
test is for karaoke: a singer sings the backing's own notes, and two tones that share a pitch
predict each other at every period of it, which shows in the taps as a comb of near-equal peaks.
A singer following the melody 10 cents sharp and 40 ms late is not taken for an echo.

## Where the port departs from mdf.c

Three changes, each measured in a test that fails without it:

- **Where adaptation starts.** mdf.c spreads its first steps across the whole tail, falling ten to
  one, for an echo that could be anywhere in it. The finder has put this one at the front, so the
  start falls ten to one across the first eight partitions. On `aec:measure`'s files, 15 dB is
  reached a second sooner; a reverberant room settles where it did, 26.8 dB against 26.9.
- **The double-talk hold.** Once adapted, the filter stops learning while the mic carries twice the
  energy of the echo it estimates, a Geigel detector on energies. 3 dB is where it sits: 1.5 dB
  slowed the first convergence and cost the reverberant room 9 dB, and 6 dB took 5 dB less echo
  out while a singer sang over it, 11.1 dB against 16.1.
- **The sanity reset.** mdf.c resets a filter whose error stays over the mic's energy for 50
  frames. Its fixed-point build asks for 6 dB over, and its float build, where the shift is a
  no-op, for any excess, which a singer over the echo trips. This uses the fixed-point threshold.

## The howl guard

A phone at the mouth is the one input the canceller may not be needed for, and the one most likely
to howl when someone walks it to a speaker. The guard is the rescue every feedback suppressor in
van Waterschoot and Moonen's survey keeps: when a howl is detected, turn the gain down, which
stabilises any loop. It detects the howl the way notch suppressors do. Every 10.7 ms, a 1024-point
FFT of the slot's voice gives the strongest peak that is narrow (10 dB over the bins three and four
away), over the band's mean (10 dB), and has no harmonic at double its frequency (12 dB over it),
which every sung partial has and a howl does not. A peak followed across analyses, allowed two
misses, that grows 6 dB after its first 107 ms is a loop running away. The first 107 ms are a sung
note's attack.

A catch ducks 12 dB within the block, holds half a second, and lifts at 3 dB a second to a ceiling
6 dB under the gain that howled, which itself lifts 0.1 dB a second. A ceiling 3 dB under let a loop
3 dB over unity howl again within seconds.

Measured on the offline driver, a phone 80 ms round trip from the speakers in a room that rings at
1 kHz, 3 dB over unity there: unguarded, the howl reaches the clipper. Guarded, it is caught 456 ms
after passing -40 dBFS, at -25.8 dBFS, and never goes louder than -26.1 dBFS in ten seconds, with
one catch. A steady tone and a ten-second sung voice pass bit for bit. With correction and a voice on, a
3 dB loop never runs away at all: the corrector's moving pitch and the voice's formant filter break
its coherence, as frequency shifting would. A whistled note that grows
would be taken for a howl; any detector that looks at the signal alone has that limit.

Frequency shifting was rejected because it moves the singer's pitch, which the corrector and the
score both read. Notches can come later if the guard ducks too often in real rooms. They would
leave the singer's level alone but cut whatever note howled.

## What it costs

- **Delay:** none, for either stage.
- **CPU:** the canceller 1.50% of one M2 Pro core per slot, 40 us of each 2.67 ms block; the guard
  0.03%. `measure-echo cpu` measures it. Sixteen slots with the canceller on would take a quarter
  of a core.
- **Memory:** about half a megabyte per slot, built with the graph whether or not the canceller is
  turned on, and 256 kB per graph for the master's history.
- **Time to work:** each song is a new graph, so each finds its echo again: about 300 ms to find it,
  then in a reverberant room 14 dB out in the second second, 18 in the third and 26 in the fifth. On
  `aec:measure`'s pure delays it is 15 dB from the second second, where AEC3 is from the first.

## Numbers

From the tests on the offline driver, under the allocation guard, and `measure-echo`:

| What                                                                              | Measured                                                    | Test's tolerance               |
| --------------------------------------------------------------------------------- | ----------------------------------------------------------- | ------------------------------ |
| `aec:measure`'s eight scenarios, its own WAVs, scored by its own formulas         | 30.4 to 39.4 dB out; 15 dB from 1 s; singer +0.0 to +0.1 dB | `measure-echo harness`, not CI |
| A house mic in a living room, 300 ms tail, singer in the master, the third second | 17.9 dB out                                                 | 15 dB                          |
| The same before the singer, and after                                             | 23.9 and 28.6 dB                                            | 22 dB                          |
| The same while the singer sings over it, echo against echo                        | 16.1 dB out                                                 | 12 dB                          |
| The singer's level through it                                                     | +0.02 dB                                                    | 0.5 dB                         |
| The path it reports, against the room's 480 frames                                | 480                                                         | 16 frames                      |
| A jitter buffer stepping 5 ms: the second before, and from 20 ms after            | 22.2 and 24.8 dB                                            | within 3 dB                    |
| A singer 10 cents sharp of the backing's melody, 40 ms late, and no echo          | nothing found, nothing touched                              | exact                          |
| A howl 3 dB over unity at 1 kHz, 80 ms round trip: caught after passing -40 dBFS  | 456 ms, at -25.8 dBFS                                       | 500 ms                         |
| The same, loudest in ten seconds                                                  | -26.1 dBFS, one catch                                       | -20 dBFS                       |
| A click with the canceller engaged on a room it has learned                       | leaves on the sample it entered                             | 0 samples                      |
| A click while the guard ducks                                                     | leaves on the sample it entered                             | 0 samples                      |
| A backing seek of 20 s: the second before, and from 100 ms after                  | 22.3 and 27.1 dB                                            | within 3 dB                    |
| A phone joining mid-song, from 3 s after                                          | 23.1 dB, the path at 480 frames                             | 20 dB, 16 frames               |
| A mixer swap mid-song, the new graph's history empty, from 3 s after              | 25.0 dB                                                     | 20 dB                          |
| The canceller turned off after learning                                           | bit for bit from the block it went off                      | exact                          |
| A loop 6 dB over unity with correction and a +2 voice on: caught, and its loudest | before it passed -20 dBFS; -21.2 dBFS                       | -20 dBFS                       |

## What this does not do

- **A stereo reference.** The canceller hears the master folded to mono. Two speakers at different
  distances playing an uncorrelated stereo backing leave a residual nothing here measures.
- **A residual echo suppressor.** Any suppressor working on overlapped frames costs delay, so it
  waits for a real room to show what the linear filter leaves. AEC3's noise suppressor, the one
  extra Aeon Karaoke's measure tries, removes 6 dB more and costs its singer about another 1 dB.
- **A speaker's distortion.** The filter is linear.
- **Decide the phone.** That is the bleed run below.

## The runs a person owes

Both are in #116, each one command:

- **The bleed with a phone at the mouth:** `measure-echo programme` writes 30 s to play at room
  volume while the phone records at the mouth; the person sings over the second half;
  `measure-echo bleed` reports the bleed and the voice at the phone, and what the canceller takes
  out of that recording offline. If the bleed sits within 15 dB of the voice, phones get the
  canceller too, since it costs no delay.
- **A forced feedback loop on real speakers:** `measure-echo howl --open-device` runs the default
  input through a slot with the guard on and raises its gain a decibel a second until the loop
  howls, then reports the gain and the time it was caught at. It mutes the room itself if the guard
  does not hold.

This record gains their numbers.

## Sources

- WebRTC AEC3: [`echo_canceller3_config.h`](https://webrtc.googlesource.com/src/+/refs/heads/main/api/audio/echo_canceller3_config.h)
  (13-block filters, five matched filters at a quarter rate) and
  [`aec3_common.h`](https://webrtc.googlesource.com/src/+/refs/heads/main/modules/audio_processing/aec3/aec3_common.h)
  (64-sample blocks, 250 a second, the matched filters' window and shift).
- SpeexDSP [`mdf.c`](https://github.com/xiph/speexdsp/blob/7a158783df74efe7c2d1c6ee8363c1e695c71226/libspeexdsp/mdf.c),
  and J.-M. Valin, [On Adjusting the Learning Rate in Frequency Domain Echo Cancellation With
  Double-Talk](http://people.xiph.org/~jm/papers/valin_taslp2006.pdf), IEEE TASLP 15(3), 2007.
- N. L. Westhausen and B. T. Meyer, [Acoustic echo cancellation with the dual-signal transformation
  LSTM network](https://arxiv.org/abs/2010.14337), ICASSP 2021.
- The [ICASSP 2023 AEC Challenge](https://arxiv.org/abs/2309.12553) and its
  [2022 rules](https://www.microsoft.com/en-us/research/academic-program/acoustic-echo-cancellation-challenge-icassp-2022/rules/).
- T. van Waterschoot and M. Moonen, [Fifty Years of Acoustic Feedback Control: State of the Art and
  Future Challenges](https://ftp.esat.kuleuven.be/pub/sista/vanwaterschoot/reports/08-13.pdf),
  Proc. IEEE 99(2), 2011: frequency shifting's 5 to 8 dB, notch suppression's features and its
  10 dB bound, and gain reduction as the rescue.
- Aeon Karaoke's `npm run aec:measure` (`app/device/service/scripts/aec-harness.mjs`), run on
  2026-11-11 against webrtc-audio-processing at `d0569cf` and gst-plugins-bad 1.28.3.
