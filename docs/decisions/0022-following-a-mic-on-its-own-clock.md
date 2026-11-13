# 0022: The echo canceller follows a mic on its own clock

**Status:** accepted, 2026-11-13, on the offline driver and the fake inputs. No real USB mic has
run it; the run a person owes is at the end.

## Decision

The canceller learns the drift itself. A delay-locked loop reads how far its filter's estimate
trails the echo and moves the reference by that and by the rate it has learned, reading the master
between frames with a 16-tap windowed sinc. The capture slot keeps its splices and says how many it
has made, and a phone's jitter buffer says the same of its skew corrections: a source hands the
canceller a `SourceDelay`, its latency, its splices so far and the streams it has started. A splice of a sample slides every
window the filter holds by one, in the block it happens; a longer one reads them all again. Nothing changes
on the mic path.

## The problem

A USB mic beside the speakers runs on its own crystal. At 100 ppm its echo of the room slides 4.8
samples a second against the master. The capture slot holds its ring's trough, and the trough only
moves when the two clocks' phases cross: every 27 s for a 128-frame input at 100 ppm, every 6.7 s
for a 480-frame one. Between those steps the echo walks, and the canceller could not follow a path
that never holds still. In the offline case of decision 0018, a 128-frame mic on the output's clock
had 23.5 to 28.8 dB taken out from the fifth second to the ninth; 100 ppm slow, 6.6 to 11.1 dB.

## What was weighed

Echo taken out is from the fifth second to the ninth in `capture_edges.rs`'s living room, against
23.5 to 28.8 dB on the output's clock.

| Approach                                                          | Echo out at 100 ppm                              | Added delay                                                                                  | CPU per slot                                      | Why not, or why                                                                                                                                              |
| ----------------------------------------------------------------- | ------------------------------------------------ | -------------------------------------------------------------------------------------------- | ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Nothing, as before                                                | 6.6 to 11.1 dB                                   | none                                                                                         | 39.3 us a block                                   | the path never holds still                                                                                                                                   |
| The slot spreads its corrections at the rate it learns            | not by the fifth second                          | none                                                                                         | none                                              | the fill shows the rate only at a step: a 128-frame mic under 128-frame callbacks learns it at its second step, 27 to 54 s in at 100 ppm                     |
| The slot resamples at the rate it learns                          | not by the fifth second                          | a sinc's half length, 8 to 16 samples, 0.17 to 0.33 ms; zita-ajbridge recommends 32, 0.67 ms | a 16- to 32-tap filter a sample                   | the same wait for the rate, and a cost on the mic path                                                                                                       |
| The capture thread's timestamps date each delivery                | not built                                        | none                                                                                         | a loop on the capture thread                      | the audio thread may not read a clock, and wakes jitter by a millisecond or so; by estimate, not measured, a fit over 5 s pins the rate only to about 16 ppm |
| The canceller follows the drift, and the slot reports its splices | 22.6 to 28.6 dB, within 0.9 dB of the same clock | none                                                                                         | 39.8 us a block; 52.0 us in a block with a splice | chosen                                                                                                                                                       |

The loop needs no help from the slot to find the rate: on a pure drift, the kernel test learns 4.84
frames a second at 100 ppm and 9.61 at 200, within 1%, two seconds after the filter adapts. What
it cannot follow is a splice. The slot moves a sample in one block, and without being told, a
128-frame correction left the canceller adding 2 dB to the slot for seconds after it.

## Following a splice

Told, the canceller moves its reference a sample. The windows its filter already holds were read
where the path was, and a filter partition j windows back would then sit j samples off. Reading
and transforming all 65 again is exact and took 59% of a core per slot while it lasted, which a
128-frame correction does for a third of a second: two slots correcting together would overrun the
block. A window one sample older has the spectrum of this one turned by `e^(-2 pi i k / 256)`, with
the sample before it added and its newest taken out, so the canceller slides each window instead:
12 us a block, within 0.3 dB of reading them all again.

What is left is the sweep. The slot spreads each splice across its block by linear interpolation, so
the delay moves from one sample to the next within the block, and the filter sees a block at a time.
In the kernel test, 32 splices cost the second they fall in 1.7 dB and 128 cost it 4.0, and the
second after is within 0.3 dB of a mic that never spliced. The same 32 splices as whole-sample
jumps cost 0.4 dB, so the sweep is most of it. A better interpolator in the slot changed nothing,
and correcting the canceller's output for the sweep gained 0.3 dB.

The delay finder follows a moved echo too, and slower. While a capture slot corrected 128 frames it
named lags up to 259 frames from where the splices put the path, more than the 128 the canceller
takes for the same path, and the filter started over: up to 26.5 dB under the same mic on the
output's clock, and more than 3 dB under for eight seconds. The finder stayed that far off for up
to two seconds, and 1.6 s of waiting was not enough. So the canceller judges a lag the finder names
only once the source's recent splices, halving every three seconds, come to less than 64 frames,
and a lag named meanwhile is judged then, not dropped. A 128-frame correction holds the finder about
three seconds. A 480-frame mic's 32-frame corrections never hold it, and neither do the few frames
a second of a finer step, such as a 100-frame mic's under 128-frame callbacks. Held for three
seconds after every splice instead, a source that spliced 4 frames every 0.8 s never had a new path
found: for the eight seconds after its path moved, its echo came out between 1.9 dB louder and
1.3 dB down. Now it comes out 17.6 dB down by the ninth second.

## A new stream

A phone that reconnects, or pauses for more than a second, starts a stream its jitter buffer
anchors at the opening delay, however far the last stream had drifted. So a source counts the
streams it has started, and the canceller starts its drift over on each, keeping the rate, since
the clocks are the same.

The echo is only near where the latency says, though. The buffer anchors a stream a playout delay
after the block its first packet arrives before, so the echo moves by where that packet fell in
its block, up to a block, and by however much longer or shorter it took to arrive than the last
stream's first packet. The room has not moved, so the filter still holds it; only where it reads
the master is wrong, and a filter that reads it even a few frames off takes out little. Left to
the filter and the finder, a phone whose new stream landed a quarter, a half or three quarters of
a packet off took out between 0.9 dB louder and 7 dB down in the second ending two seconds after
it came back, against 16 to 18 dB before it stopped: a move of 30 to 90 frames falls inside what
the finder takes for the same path, so nothing moves the filter, and a longer one waits 1.4 to
1.7 s for the finder to name the new lag and then learns the room afresh.

So on a new stream the canceller looks for where the echo landed. It runs the filter without
learning, reading the master 21 ms early, and cross-correlates the filter's estimate of the echo
of everything but the slot's own voice with what the mic hears. The slot's own voice is the mic
itself played back and correlates with it at every lag. The peak, once it leads every lag more
than 16 frames from it twofold, is the move to a fraction of a frame; the filter then reads the
master there and cancels at once. A voice correlates with itself a few pitch periods away nearly
as well as at its own lag over one syllable, and without the second test an echo 30 frames early
was taken for one 711 early. While it looks, under half a second, the slot passes untouched,
and `stats()` keeps the path it had. A frame counts only when the estimate holds at least a tenth of the mic's
energy: a perfect estimate 10 dB under the mic correlates at 0.32, under the 0.5 the peak needs.
An estimate under -70 dBFS is silence: a silent intro or break, or a master holding only the
slot's own voice. The search waits silence out with the slot passing untouched, however long it
lasts; after 10 s of it, what was summed is forgotten and the next frame that counts starts
afresh, since the drift's rate, learned to 1%, moves the reference half a frame in that time at
100 ppm. Spending its budget on silence, a phone back a quarter packet off during a silent intro
took out 10.6 dB in the second ending 2 s after the backing returned, against 16.7 before it
stopped; waiting, 14.4 to 14.7.

Every search ends once the master has reached the mic for 2 s, counted or not, and silence does
not run that clock down, so no stretch of silence keeps a search open past it. The slowest search
in these tests named its lag half a second after the master reached the mic. A mic far louder
than the filter's estimate never counts: a phone picked up off the table, a new input gain, a room
singing along. With no end, it kept the slot untouched for good. A search that ends without a lag
keeps the filter, unless fewer frames were heard than a peak needs to be judged, 85 ms of them:
then the mic outweighed the estimate throughout, as a room far louder than the filter learned
does, and the filter starts over. A room 15 dB louder left 3 of 750 frames heard; a note held
17 dB over the echo for the whole search, 72, and one 9 dB over, 382. An
adapted filter under a mic 15 dB over its estimate holds in double talk for good, and took out
1.6 dB from the search's end on. Learning afresh, a phone back 15 dB louder took out 14.6 dB in
the second ending 4 s after its return, against 12.1 for the same room learned from nothing in
its second second. A singer over the return is told apart by the frames it leaves heard: a held
note 9 or 17 dB over the echo for the whole search kept the room, and took out 17.7 dB from a
quarter second after the note, against 16.8 before the stop. Starting over on any search without
a lag threw that room away: 10.1 dB, with no findings for 0.69 s. A search that ran out starts
again only with a new stream. The search reaches 21 ms either way: a block's rounding, and
three quarters of a 20 ms packet.

On the offline driver, through Opus and the jitter buffer, a phone whose new stream lands a
quarter, a half or three quarters of a packet later or earlier, on 2.5 ms and 20 ms packets, takes
out 17.1 to 17.9 dB in the second ending two seconds after it came back, against 16.3 and 17.8
before it stopped; each move is found within 0.4 of a frame. In the kernel test, a phone 100 ppm
fast that reconnects after 15 s comes back within 0.1 dB of one that never did, against 2.6 on
the drift restart alone. The second it reconnects in costs 3.3 dB more, 9.5 dB out against 12.8,
for the time it passes untouched. Without the count, it took out between 1.1 dB
louder and 2.8 dB down for the seven seconds after.

## The loop

A filter estimate that is `lag` samples early leaves an error of `-lag` times its slope, so `lag` is
the error against the estimate's slope, over the slope's energy, each block. A proportional and an
integral gain of 0.03 and 4e-4 per block give a natural frequency of 1.2 Hz, damped at 0.75. Gains
of 0.005 and 2e-5 took four seconds to learn the rate instead of two, and 7 to 18 dB less out at the
fifth second.

The loop reads only once the filter has adapted and not while the double-talk hold is on; then it
moves on at the rate it has learned. A tighter gate, reading only while the filter explains 90% of
the mic, kept it from learning: at 200 ppm, 16 dB out at the fifth second instead of 27.

## Numbers

From `capture_edges.rs` on the offline driver and the fake input, under the allocation guard, the
echo taken out from the fifth second to the ninth:

| Input period | Clock        | Taken out                       | Against the same clock            |
| ------------ | ------------ | ------------------------------- | --------------------------------- |
| 128          | the output's | 26.2, 23.5, 24.4, 28.8, 28.1 dB |                                   |
| 128          | 100 ppm fast | 26.3, 22.6, 24.3, 28.8, 28.3 dB | 0.9 dB under at most              |
| 128          | 100 ppm slow | 25.9, 22.6, 23.9, 28.6, 27.7 dB | 0.9                               |
| 128          | 200 ppm fast | 26.0, 22.7, 24.0, 28.4, 27.3 dB | 0.8                               |
| 128          | 200 ppm slow | 25.6, 23.1, 24.5, 28.6, 27.6 dB | 0.6                               |
| 480          | the output's | 26.8, 23.5, 25.1, 29.7, 28.9 dB |                                   |
| 480          | 100 ppm fast | 27.3, 23.9, 25.5, 27.3, 28.8 dB | 2.4, the second with a correction |
| 480          | 100 ppm slow | 27.0, 24.0, 24.5, 26.4, 28.5 dB | 3.3, the same                     |
| 480          | 200 ppm fast | 25.9, 23.4, 24.7, 26.5, 27.9 dB | 3.2, the same                     |
| 480          | 200 ppm slow | 25.5, 23.2, 23.5, 28.1, 27.4 dB | 1.6                               |

The test holds a 128-frame input within 3 dB and a 480-frame one within 4. A 480-frame input
corrects 32 frames every 6.7 s at 100 ppm and every 3.3 s at 200, so a second in three or two holds
a correction.

The room's echo in these tests is now sampled between the output's frames by a 32-tap windowed sinc,
as a converter on another clock samples it. Linear interpolation, as before, added a lowpass that
swept with the two clocks' phase, which no real mic has. On the output's clock the samples fall on
frames and nothing changed.

`capture.rs` holds the accounting: over an hour at 200 ppm either way with wakes up to 8 frames
early or late, a capture slot's latency and splices stay within 4 frames of the true wait less the
drift, through the gap a slower input meets, every move of the target and every splice. An input
that stops and then hands over all it missed comes back within those 4 frames. One whose frames
are gone comes back within a step of the slot's wait, as its latency does, since the delivery phase
may have moved: the slot counts nothing of a stopped input's recovery, and the latency the target
sets is where the delay ends up.

## What it costs

- **Delay:** none. The mic path is untouched; the sinc reads the reference, which is never heard.
- **CPU:** 39.8 us a block against 39.3 on main, measured back to back on an M2 Pro, 1.49% of a
  core per slot; 52.0 us in a block where the source splices. `measure-echo cpu` reports both.
  Looking for a new stream's echo adds nothing to a block that is not looking: 40.1 us against
  40.1, back to back on the same machine. A block that is looking costs less than one that
  adapts, 38.6 us against 40.9 for a source that starts a stream every 100 blocks, since the
  filter only predicts.
- **A singer over a mic on the master's clock:** with no drift to follow, the loop's offset wanders
  0.24 samples either way, and the echo taken out while the singer sings over it falls from 16.1 to
  15.6 dB. Before and after them nothing moves past 0.1 dB.

## What this does not do

- **Measure a phone.** A jitter buffer that moves its delay reports the new delay in its latency,
  and as a splice the part of that the echo did not move by: the new delay also takes up the slack
  the stream had drifted into, which the loop has followed already. Followed as a jump of the
  latency alone, a fall at 200 ppm counted the drift twice and left the slot up to 2.8 dB louder
  than it went in; reported, the echo comes out no less than a same-clock phone's. Its skew
  corrections report as splices too: each drops or conceals a step, 120 frames on 2.5 ms
  frames, and the playout delay holds. Unreported, the loop would carry its offset across a step
  that sits inside what the finder takes for the same path; in the kernel test that left the slot
  2.3 to 3.3 dB louder than it went in, and reported it costs 0.2 dB. A phone on its own clock
  drifts between steps, and the loop follows that as it does a house mic's, but no test here runs a
  phone's drift through its jitter buffer and the canceller together.
- **A splice in the canceller's own block.** The sweep inside a spliced block stays. A 480-frame
  input's 32-frame correction costs its second up to 3.3 dB, every 6.7 s at 100 ppm. A 128-frame
  input's whole-period correction costs its second 6.2 to 7.1 dB, every 27 s at 100 ppm and 13 s at
  200; `capture_edges.rs` runs it for 40 s. Over two minutes that left 4 to 12 seconds of 115 more
  than 3 dB under the same clock, at 100 ppm and 200 either way. A slower input's first slip also
  plays a gap, once: at 200 ppm slow that second came out 13.7 dB under.

## The run a person owes

`measure-echo` has no capture mode yet. In #116's real-room run, a person runs a slot with a USB mic
beside the speakers, the canceller on, for ten minutes of a song, and reads `echo.removedDb` from
`stats()` each second. This record gains what it shows.

## Sources

- AEC3's delay estimator and SpeexDSP's MDF, as decision 0020 cites them.
- [zita-resampler](https://kokkinizita.linuxaudio.org/linuxaudio/zita-resampler/resampler.html):
  `hlen` from 16 to 96, 32 for very high quality at 48 kHz.
