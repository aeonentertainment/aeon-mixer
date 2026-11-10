# 0009: What a host is told about the corrector's delay

**Status:** accepted, 2026-11-06, ahead of the code in #44.

## The problem

The corrector's shifter reads the voice back from a delay line. The delay climbs or falls with the
shift, jumps by a pitch period at the end of its range, and stays where it is when the voice stops.
On the golden vocal, one read per second gave 10.0, 1.2, 7.2, 7.6, 7.9, 1.3, 4.4 ms. A host needs
something to do with that, and #44 first proposed adding the current figure to its configured
latency.

## Decision

`stats()` gives each slot two numbers. Both read 0 with correction off.

- **`correctorDelayMs`** is the delay as it stands, for a status panel or a log.
- **`correctorMaxDelayMs`** is 15 ms: the shifter's floor and 1.125 periods of the lowest voice it
  corrects, 82 Hz. It holds still, so a latency budget adds this one.

Neither enters a clock. Nothing can play a live voice early, so there is no compensation for a
moving figure to drive. The guide clock and a pitch reading's position both name the corrector's
input, which is ahead of its delay. Subtracting a sawtooth would also run that clock backward at
every jump, and note edges and readings need it to run forward.

Aeon Karaoke's `configuredLatencyMs` is documented as the sum of the fixed buffers, and its HUD and
latency e2e compare it with a budget. Adding `correctorDelayMs` would make that comparison depend on
when it was read. Adding `correctorMaxDelayMs` keeps it steady and overstates a high voice, which
stays on the safe side.

`pitch-correction` used to change nothing, and now corrects. The package is at 0.0.0 and this is
what the API always promised, so it takes no major bump.

## How others report a delay

A plug-in reports one latency, and a change means a restart. VST3's
[`getLatencySamples`](https://dplug.dpldocs.dlang.org/dplug.vst3.ivstaudioprocessor.IAudioProcessor.getLatencySamples.html)
warns that changing it can interrupt audio while the host rebuilds its delay compensation, and
[Ardour](https://git.ardour.org/ardour/ardour/commit/57c660836f3ae2d188b78b4fdde8247e97eb7220)
re-activates the plug-in to read a new value. So nobody reports a sawtooth to a host, and a
[suggestion on a Native Instruments forum](https://community.native-instruments.com/discussion/9823/feature-request-delay-compensation/p2)
is to report the maximum. That one is a user's suggestion and not a standard. I found no published
account of how a live vocal processor reports a delay that moves. Vendors quote one figure:
[Antares claims 2.5 ms](https://www.antarestech.com/blog/real-time-pitch-correction-for-live-performance-with-autotune-2026)
of core latency in low latency mode, unmeasured here.

I found no threshold for a singer's own voice. The nearest is a speech study in which
[a 16 ms delay already affected detection](https://sonicfield.org/library/delay-detectability-and-speech-rate-as-a-function-of-delay-duration-in-auditory-),
so these milliseconds may matter, and only a person can say whether they do.

## The golden vocal's p95 and max

#43 measured p95 10.9 to 12.0 and max 11.7 to 13.0 ms where GStreamer's harness read 8.3 and 10.1,
and could not explain it. I found no cause in this engine. Its four start offsets were too few to
show how far the figure moves. In the key of C, 3 to 13 s, on the harness's 50 ms grid, over 100
start offsets from 0 to 99 ms:

| Samples            | Mean                   | p95                     | Max                      |
| ------------------ | ---------------------- | ----------------------- | ------------------------ |
| All                | 4.0 to 7.1, median 5.5 | 7.7 to 12.1, median 9.9 | 9.8 to 13.1, median 11.1 |
| Voiced only        | 3.9 to 6.0, median 5.2 | 7.1 to 11.0, median 9.2 | 8.0 to 12.5, median 10.5 |
| GStreamer, one run | 4.9                    | 8.3                     | 10.1                     |

GStreamer's figures sit inside the span: 10 of the 100 offsets give a p95 at or under 8.5, and 19 a
max at or under 10.3. Decoding with ffmpeg in place of the engine's decoder reproduces #43's four
offsets to 0.1 ms, so the decoder is not the gap either.

The reason p95 moves so far is that the delay freezes in a rest and most of this clip is rest: 2840
of the 3750 blocks in the window are unvoiced. At offset 0, the top 5 percent is 194 blocks and only
8 are voiced. Nearly all of it is one 0.47 s rest, from 10.25 s, held at 11.9 ms. A statistic over
time mostly says where the sawtooth stopped before the longest rest.

The voice is also really that low. Its guide reaches down to MIDI 40 and a tenth of its voiced
readings sit under 87 Hz, so 13 ms is what such a voice costs and does not point to a detector error.

I did not run the GStreamer harness, since that writes into Aeon Karaoke's tree, so this shows the
gap is within the spread and does not show why that one run landed where it did.

Draining the delay to its floor during a rest looks like a free win and is not. In a scratch build,
the first voiced block after a rest of 100 ms or more went from a median of 5.0 to 1.6 ms, but the
voiced mean only went from 5.2 to 4.6 and the voiced p95 rose from 9.2 to 10.5: a voice shifting up
from the floor jumps a period back at once. It is not worth a change to the shifter on these numbers.

## When correction is worth its cost

What a slot pays depends on the voice. On a held tone the most it reaches is 2.5 ms at 880 Hz, 3.7 at
440, 6.25 at 220, 11.2 at 110 and 14.6 at 82.5, and the mean sits near half that. A singer who has not
asked pays nothing, which a test holds bit for bit. Without a guide the corrector pulls to the nearest
semitone at the same delay, so Aeon Karaoke's rule that a song without a guide is not corrected should
carry over to this engine.

Whether it fits the budget needs a mic-path figure, and the hardware ones do not exist. As arithmetic
across sources, against decision 0008's 20 to 30 ms with correction included, every path is over it
today. A house mic over l16 is about 22 to 25 ms offline plus 9.8 to 20 ms of output on a Mac, 32 to
45 ms, before correction adds anything; the 15 ms ceiling takes it to 47 to 60 ms. A phone is 71 to
114 ms with 20 ms frames and its capture, and correction adds about 5 ms on average to that. So the
corrector's delay is one of several stages that have to shrink: #56's chosen playout delay takes
about 11 ms off the engine's share, and the sender's frame size is worth 21.5 ms by #61's
measurements. None of this changes the `stats()` contract; a budget that counts the corrector is
what `correctorMaxDelayMs` is for.

## Timing the corrector in measure-latency

A sweep will not do as it stands, and the reason is the detector. After a voiced phrase and a 150 ms
rest, #61's 200 ms chirp from 0.5 to 7.5 kHz moved the delay on 37 to 53 of 72 phrase lengths, and its
peak landed up to 362 frames from the delay read just before it. YIN reads the sweep as a pitch near
1 kHz, at a confidence that reached 0.92 and cleared the corrector's 0.9 bar, so the shifter acted on
it. A short signal passes, since the detector's 43 ms window barely holds it. At a retune of 0, across
72 phrase lengths and voices at 116, 233 and 466 Hz, a 50 ms sweep from 2 to 7.5 kHz and a 1 ms click
both arrived exactly at the delay read before them.

So the row plays an off-pitch voiced phrase, then a rest, then the short sweep, through the existing
`MatchedFilter`. The phrase's length sets how far the delay has moved, and the row prints the span for
a low, middle and high voice. It has not been tried through Opus.

## Not measured

- Whether anyone hears the delay move, or how a singer takes 15 ms against 5.
- Any hardware figure, or any phone stream.
- The ceiling is a measured one: 717 frames against 720 at 82 Hz, with targets flipping by up to six
  semitones. Working from the code alone, a shift that reverses during a jump's crossfade could pass
  it by up to 37 frames. That did not occur in these runs.
