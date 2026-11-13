# 0010: How the corrector reads pitch

## The problem

The corrector read one 2048-sample YIN window every 10 ms. A reading describes the window's centre,
so on a glide the correction trailed the voice by 21 ms plus up to 10 ms of waiting for the next
reading. #45 asked to halve the window. Halving helps, but it is the smaller part of what was there.

## What it does now

Three things, all on the side chain, so the voice gets no extra delay.

**The window follows the voice.** Every 5 ms `PitchReader` tries 512 samples, then 1024. It takes the
first window that is sure, holds at least 2.2 periods of what it found, and does not leap more than 9
semitones from the last note. Every 10 ms the 2048 window has the last word, and that reading is the
one a host publishes. So 207 Hz and up reads in about 5 ms, 104 Hz up in about 11 ms, and a bass in
21 ms, as before.

**The corrector steers from where the voice is now.** A sure reading updates a slope, averaged over 20
ms, and the shift is taken from the reading projected across the gap: half the window, half a hop, less
what the shifter itself delays the output. The note it pulls toward still comes from the reading, so a
glide does not reach a new note before the voice does. Published readings are the longest window's,
raw, every 10 ms at its centre, so the display and the score get a measurement at a known position
that never moves back when the window steering the voice changes. Its error is against that same reading: the
class nearest the published pitch, held as the steering class is, less that pitch, in the singer's
own octave, and 0 when the reading is under the bar. The shifter still steers by its own newer
reading; taking the event's class from that one put a clean C4 to D4 step 1.9 semitones out for a
reading, where this keeps every sure reading within 0.7. On today's corrector a reading under the bar within 30 ms of a sure one
kept the sure one's error; a host now sees 0 there, as it sees for any reading it would call
unvoiced. The cadence, the position and every other field are as before.

**The period comes from the paired samples.** YIN's dip leans sharp, because the unpaired tail and the
running mean both pull it toward shorter periods. On a steady tone that was 1.6 cents at 82 Hz in the
2048 window and 4.2 cents at 104 Hz in the 1024 window, and a corrector hands that lean to the voice.
The period is now refined on the difference per paired sample: under 0.2 cents at every window from 82
to 880 Hz. Choosing the dip and the confidence are as #87 measured them.

## What it moved

`measure-corrector`, direct, each row with its span over start offsets. `0 ms` is a hard retune, the
corrector with nothing but the reading in its way.

| Clip, setting           | Before                | After                 |
| ----------------------- | --------------------- | --------------------- |
| 6, strength 0.6, 40 ms  | 21.3 (20.0 to 21.7) c | 14.8 (13.6 to 15.6) c |
| 6, strength 1, 40 ms    | 19.3 (18.2 to 19.6) c | 11.9 (11.8 to 13.1) c |
| 6, strength 1, 0 ms     | 7.3 (7.3 to 8.4) c    | 1.0 (1.0 to 1.4) c    |
| 19, strength 0.6, 40 ms | 19.1 (16.8 to 19.7) c | 15.8 (15.8 to 18.2) c |
| 19, strength 1, 40 ms   | 16.4 (16.0 to 18.0) c | 12.0 (11.3 to 15.0) c |
| 19, strength 1, 0 ms    | 9.0 (7.8 to 9.0) c    | 1.1 (1.1 to 1.4) c    |

Clip 19 at strength 0.6 is the thin one. Its row clears the old span's floor, and its span still
overlaps. Clip 6 at strength 0.6 improves against annotator 1 and worsens against annotator 2, 14.5 to
17.2 cents. Every other row improves against both.

The ruler moved too, since `contour` reads the output with the same detector. `off` is the control: the
voice did not change and its row moved by up to 2.3 cents on clip 19. With only the period fix applied,
today's corrector reads 19.6 and 19.0 on clip 6 and 18.6 and 16.9 on clip 19, so those are the fairer
before.

Published readings are the 2048 window's, so the detector rows read as they did: within a semitone
95.4, 97.4, 97.7 and 98.4 percent on clips 2, 6, 19 and 10, against 95.4, 97.7, 97.6 and 97.8.
What the corrector acts on is its own reader's, which the `flapping` rows see. Inside sung notes past
their first 30 ms, at strength 1, correction held 98.8, 100.0, 99.6 and 98.5 percent of readings,
against 97.1, 99.0, 96.2 and 95.3. It let go of a note 0.21, 0, 0 and 0.17 times a second, against
0.26, 0.67, 0.65 and 0.68, and every drop on today's corrector came at a reading under the bar. The
golden vocal's notes are 70 ms at the median, which leaves its rows 2.4 s of note to count over: it
lets go 2.1 times a second either way, and 39 percent of its readings inside notes are under the bar. While the reader's own readings were published, frames the reference calls unvoiced that
it acted on rose from 0.5, 1.7, 5.6 and 16.6 percent to 2.1, 4.3, 9.5 and 24.6. The published rows no
longer see that.

## The reading is no longer most of the trouble

A throwaway zero-lag oracle delayed the shifter's input so the 2048 window centred on the sample being
shifted. At strength 1 and 40 ms it reads 11.3 and 8.6 cents on clips 6 and 19, and we read 11.9 and
12.0. At a hard retune it reads 1.0 and 1.1, and we read 1.0 and 1.1. So the reading has at most 3.4
cents left to give at 40 ms, and going from 40 ms to a hard retune gives 11. That part is the one-pole
retune, which is #46's to change. Strength sets its own floor: at 0.6 the oracle reads 15.2 and 15.0.

## What it asks of the howl guard

The corrector reads a howl as readily as a voice, and at 5 ms it retunes one a semitone at a time.
A loop through a correcting slot then moves and loses its shape for part of each trip. On the guard
before #137 that hid more howls than today's corrector does: 20 of 480 seeded rooms never caught
against 8. #137's guard follows a tone the corrector moves, and with it this corrector leaves none of
480 uncaught and 74 louder than -20 dBFS, against 213. Decision 0020 has the numbers.

## What it costs

|                        | Before | After          |
| ---------------------- | ------ | -------------- |
| CPU, the core          | 0.28 % | 0.68 to 0.71 % |
| CPU, a slot correcting | 0.36 % | 0.75 to 0.93 % |
| CPU, four slots        | 1.37 % | 2.94 to 3.60 % |

The ranges are four runs on one machine. Publishing the 2048 window's reading costs one more 2048 read
every 10 ms when a short window vouched, 24.4 microseconds, at most 0.24 % of a core.

A reading costs 7.4, 14.9 and 24.4 microseconds at 512, 1024 and 2048 samples on an M2 Pro. An unsure
frame runs all three, 47 microseconds in one block, and today every slot reads in the same block. That
is 0.75 ms of a 2.67 ms block at 16 slots on this machine, against 0.39 ms before. If a slower device
misses blocks, staggering the slots is the first thing to try.

The bound on the voice's delay does not change, 1.2 ms plus 1.125 periods. On the golden vocal the mean
stays at about 5 ms and the largest delay seen rose from 12.0 to 13.9 ms in the song's key, since the
corrector engages more often.

The cost in the voice is vibrato. A 50 cent vibrato at 5.5 Hz came out at 1.21 of its depth at full
strength and 1.10 at 0.6, because late correction added to it. It now comes out at 0.82 and 0.85, and
0.70 and 0.76 at 4 Hz. The correction is on time, so it takes part of the vibrato out. On real singing, direct,
`motion_kept` went from 1.07 to 1.24 down to 0.82 to 0.88, and `motion_added_p95` from up to 19.4 cents
to 0. A voice sitting on the midpoint of two notes with vibrato crossing it moves more: 4.6 to 16.2
cents out for 19.7 in at full strength, 9.0 to 15.5 at 0.6. #46's slow centre stops correction chasing
vibrato, which should settle both. An agent cannot hear, so whether any of this is better is for the
blind listen.

## What was rejected, and why

| Option                                 | Number                                                                            | Verdict                                         |
| -------------------------------------- | --------------------------------------------------------------------------------- | ----------------------------------------------- |
| 1024 alone at 10 ms, #45's proposal    | clip 6 15.4 and clip 19 18.1 at full, clip 10 accuracy 95.8 %, engaged 89 to 95 % | half the gain, 1.75 periods at 82 Hz            |
| 512 alone                              | clip 10 accuracy 63.7 %, clip 19 88.1 %                                           | loses low voices                                |
| Ladder with no leap guard              | clip 2 accuracy 94.1 %, octave errors 5.4 %                                       | fails the bar                                   |
| 2048 at 5 ms                           | clip 6 17.0 at full, engaged unchanged                                            | cadence alone moves little                      |
| Ladder every 2.5 ms                    | hard retune 0.1 c better on clip 6, twice the readings                            | not worth it                                    |
| Five windows, 512 to 2048              | no row moves past its span                                                        | more worst-case cost for nothing                |
| 2.0 or 2.5 periods                     | 2.0 costs clip 10 a point of accuracy; 2.5 puts A3 in the longer window           | 2.2                                             |
| A stricter bar for short windows, 0.93 | clip 10 engaged 89 %, false engagement 18.8 %                                     | little for the recall it gives up               |
| Projecting further than the lag        | hard retune worse: clip 6 6.7 c at 16 ms against 2.0 c, vibrato 0.69              | compensating the retune is #46's choice to make |
| Picking the note from the projection   | fence row 19.2 c out against 14.1 c                                               | the reading picks the note                      |

Other trackers, from their papers and sources:

- **zita-at1** reads a 2048-sample Hann window every 512 samples at 48 kHz (`retuner.cc`), the regime we
  started from.
- **MPM** scored 93.7 % on clip 2 at 2048 in Aeon Karaoke's table, and the same window means the same
  causal delay.
- **pYIN** smooths with a Viterbi pass over a batch, which a live path cannot wait for.
- **PESTO** answers a step in 90 ms at 25 % of a core in Aeon Karaoke's table. **CREPE tiny** scored
  86.5 % on clip 2.
- **Bitstream autocorrelation** (Cycfi Q) claims two cycles on guitar. It works on zero crossings, and we
  did not test it on a voice whose formants cross them.
- **Antares** publishes latency claims but nothing we could find on tracking time.

YIN itself sets the floor. It gives latency as Tmax + T, the rule of thumb as twice the longest period,
and warns that integrating less than Tmax risks locking to the fine structure of a long period.

## Sources

- De Cheveigne and Kawahara, YIN, JASA 2002.
- McLeod and Wyvill, A Smarter Way to Find Pitch, ICMC 2005.
- Mauch and Dixon, pYIN, ICASSP 2014.
- Riou et al., PESTO, ISMIR 2023.
- De Guzman, Fast and Efficient Pitch Detection: Bitstream Autocorrelation, cycfi.com, 2018.
- Adriaensen's retuner, as carried in x42's fat1.lv2 `src/retuner.cc`.
