# 0021: The corrector's shifter pays one period of the voice, and a rest moves it down

**Status:** proposed, 2026-11-11, for #112. It rewrites decision 0019's "until when".

## The problem

Decision 0019 ships pitch correction at GStreamer parity: 1.2 ms at rest and up to 15 ms for a voice
at 82 Hz, over the 20 to 30 ms budget of decision 0008. Cloud agreed to that and asked for an
innovative way to cut the corrector's own delay below parity.

## What the delay is made of

The shifter reads the voice from a delay line at the transposition's rate. Shifted up, the read
point gains on the input until it repeats a period; shifted down, it falls behind until it skips
one. Either way the delay rides a sawtooth one period of the voice tall, on a floor. Before #112:

- The floor was 59 samples whatever the shift, room for the old read point to keep gaining on the
  input through a 128-sample crossfade at six semitones up. A correction is a fraction of that.
- A jump ahead fired at the floor plus 1.125 periods, so the search could reach an eighth of a
  period either side of the detector's reading. A voice shifted down rode from the floor plus an
  eighth to the floor plus 1.125 periods, an eighth higher than it needed to all the time.
- A rest froze the delay wherever the last note left it, so the next line started there, at a
  median of 4.7 ms on the golden vocal.

## What others do about it

- **Every time-domain corrector pays a period.** A splice that is not a whole period away breaks
  the waveform, so a causal shifter that keeps a voice's harmonics in place waits up to one period.
  TD-PSOLA waits a whole period past each pitch mark for its grain (Moulines and Charpentier 1990),
  worse than a delay line's half a period on average; decision 0004 measured both. Hildebrand's
  patent behind Auto-Tune (US 5,973,252) corrects by resampling under a pitch detector, the same
  family as this shifter.
- **Vendors quote the floor.** Antares gives 2.5 ms for Auto-Tune's low latency mode. Reviews of
  Waves Tune Real-Time give 0 to 4 ms "depending on the note", and it reports zero to the host.
  TC-Helicon publishes no figure; Antares' own comparison puts the VoiceLive 3 near 2 ms. A delay
  of up to 4 ms that moves with the note is one period of a voice from 250 Hz up. None says what a
  bass pays.
- **Short-frame resynthesis under-shifts.** A spectral shifter's delay is its frame, and a frame
  short enough for the budget cannot resolve a low voice's harmonics. Signalsmith Stretch at 5.3 ms
  gave +37 of 50 cents at 110 Hz (decision 0004), and at 10.7 ms +8.6 (Aeon Karaoke's
  `auto-tune.md`). Juillerat, Schubiger-Banz and Müller Arisona (ICALIP 2008) shift each of 200 to
  1500 bands at its own period, under 10 ms, and the bands lose the phase relation a voice's
  harmonics share.
- **Predicting the next period does not move the read point.** A jump's correlation reads only
  behind the read point, at samples already written, so looking back costs nothing. What costs is
  the drift until the next jump.

So the period is the price of a splice that keeps the voice whole, and what can be cut is
everything around it.

## Decision

Four changes to `PitchShifter`, and one line in `PitchCorrector` that tells it when the detector
hears a voice. The detector and the control path are untouched.

1. **The floor follows the ratio.** It is 5 samples, plus what the old read point gains through a
   crossfade when shifting up. 9 at a quarter tone up, 5 shifting down, 59 at six semitones up.
   Decision 0004 found this on the voice shifter in #47.
2. **A jump ahead waits for one period of drift and no longer**, unless the waveform still matches
   better further on. Then it waits two samples more and looks again, up to an eighth of a period
   past the reading. A jump back searches as before. Both stop at 82 Hz's period plus a sample.
3. **Jumps are finer than a sample**, from #47. Whole-sample jumps bias a high voice by up to
   1.8 cents.
4. **A rest moves the read point down.** Once the detector has missed the voice for four readings
   in a row, the read point jumps, under the usual crossfade, to a quarter period above the floor,
   once per rest. A rest is the one place a splice has no waveform to match, so the delay comes down
   for free, and the next line starts low with room for a voice shifted up to run before it repeats
   a period.

The fourth is the new idea. Decision 0009 tried draining to the floor itself and saw the sung p95
rise, because a voice shifted up from the floor repeats a period at once. A quarter period of room
is what fixes that, and the measurements below chose it.

## The numbers

Bound for a voice at each pitch, corrected by a quarter tone or less, in ms. The bound is the floor
and one period with two samples to spare. "Held tone" is `measure-corrector`'s row, the larger delay of a
voice held 30 cents sharp and one held 30 cents flat. "At rest" is a click with no voice.

|                  | 82 Hz | 110 Hz | 220 Hz | 880 Hz | At rest |
| ---------------- | ----- | ------ | ------ | ------ | ------- |
| Bound before     | 14.97 | 11.48  | 6.36   | 2.53   | 1.23    |
| Bound now        | 12.43 | 9.32   | 4.78   | 1.37   | 0.10    |
| Held tone before | 14.60 | 11.23  | 6.25   | 2.48   | 1.23    |
| Held tone now    | 12.02 | 9.40   | 4.77   | 1.29   | 0.10    |

The 110 Hz held tone reads over its pitch's bound because its flat voice sits at 108 Hz, whose
period is longer.

The ceiling a host budgets with, `correctorMaxDelayMs`, is six semitones up on an 82 Hz voice: 59
samples of floor, 585.4 of period and 2 to spare, rounded up to 648 frames, **13.5 ms**, from 720
and 15 ms. A voice at 82.6 Hz pulled six semitones each way reaches 637 frames; before, 717 of 720.

What a singer pays while singing, through the real slot chain. Sung is the delay every 50 ms where
the voice sounds; onset is the median at the first voiced reading after 100 ms or more of rest. The
golden vocal is in the key of C at full strength, over four start offsets under 40 ms; vocadito is
melody mode at full strength.

| Take, ms            | Sung mean, before and now | Sung p95          | Max                 | Onset median     |
| ------------------- | ------------------------- | ----------------- | ------------------- | ---------------- |
| Golden vocal, first | 5.4, 2.5                  | 8.6, 5.6          | 13.0, 6.4           | 4.7, 1.7         |
| Golden, all offsets | 4.5-6.0, 2.5-3.0          | 8.6-10.9, 5.6-7.2 | 11.7-13.0, 6.4-10.5 | 4.3-8.6, 1.7-2.6 |
| vocadito 2          | 2.8, 1.5                  | 4.3, 2.9          | 5.3, 3.9            | 2.2, 1.1         |
| vocadito 6          | 3.2, 1.9                  | 5.0, 3.7          | 6.0, 4.5            | 3.6, 1.2         |
| vocadito 10, low    | 5.0, 3.3                  | 9.2, 7.6          | 10.4, 8.6           | 6.5, 1.3         |
| vocadito 19         | 4.2, 2.4                  | 6.8, 5.0          | 9.6, 8.3            | 4.6, 1.7         |

Intonation on #43's rows lands inside each row's start-offset range before #112, or under it. The
answer key's restored medians move by a cent or less. Restored, its mel cepstral distortion, which
reads damage more than pitch, rises by 0.0 to 0.1 dB on every clip: the one row that moved the same
way everywhere, and one for the listen. Left alone it moves both ways. A slot with correction off
is the input bit for bit, as before. CPU per correcting slot is 0.36 % of a core, as before.

## What each candidate costs

Delays are the bound at 82, 110, 220 and 880 Hz, or the held-tone row where one was run. Splice
correlation is the mean correlation of every jump the run made, across all of `measure-corrector`'s
rows: 0.963 before. It is the only artefact measure here that reads the splices themselves.

| Candidate                                                                         | Delay, ms                                                         | Accuracy                  | Splices                               | Verdict                                                                                                                                                                                                                      |
| --------------------------------------------------------------------------------- | ----------------------------------------------------------------- | ------------------------- | ------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Before #112                                                                       | 14.6, 11.2, 6.3, 2.5                                              | rows above                | 0.963                                 |                                                                                                                                                                                                                              |
| Floor that follows the ratio alone                                                | 13.5, 10.1, 5.1, 1.35                                             | within the offset ranges  | 0.963                                 | kept                                                                                                                                                                                                                         |
| Jump capped at one period                                                         | 12.0, 9.4, 4.8, 1.3                                               | within                    | 0.934, 30 % of jumps at the cap       | rejected: the voice's period at the read point is often longer than the reading's                                                                                                                                            |
| Capped at a thirty-second over                                                    | 12.4, 9.4, 4.8, 1.3                                               | within                    | 0.947, 8 % at the cap                 | rejected                                                                                                                                                                                                                     |
| Wait for the period (chosen)                                                      | 12.0, 9.4, 4.8, 1.3                                               | within                    | 0.946 alone, 0.958 with the rest move | kept                                                                                                                                                                                                                         |
| Rest move to the floor                                                            | as above; onset 0.6-1.9; golden sung p95 10.7, against 8.6 before | within                    | 0.892, 24 400 jumps                   | rejected, as in decision 0009: a voice shifted up from the floor repeats a period at once                                                                                                                                    |
| Rest move to an eighth                                                            | as above; onset 0.6-1.6                                           | within                    | 0.944                                 | rejected: more jumps                                                                                                                                                                                                         |
| **Rest move to a quarter (chosen)**                                               | as above; onset 0.8-1.7                                           | within                    | 0.958                                 | kept                                                                                                                                                                                                                         |
| Rest move to a half                                                               | as above; onset 1.4-3.9                                           | within                    | 0.959                                 | rejected: gives back the onset                                                                                                                                                                                               |
| Landing a thirty-second above the floor, as hysteresis                            | +0.4 at 82 Hz                                                     | within                    | 0.960 with the rest move              | rejected: buys 0.002                                                                                                                                                                                                         |
| Correct only as far as the singer is off                                          | unchanged                                                         |                           |                                       | already how strength works. The sawtooth is a period tall whatever the shift; a smaller shift only spaces the jumps, one every period over the shift's ratio less one: 2.1 s at 82 Hz for 10 cents                           |
| Predict the next period from the detector                                         | unchanged                                                         |                           |                                       | the correlation is that prediction, and costs no delay                                                                                                                                                                       |
| Shift on the phone before Opus                                                    | unchanged                                                         |                           |                                       | moves the delay to the phone and removes none: a frame's last sample still waits on the floor and a period. It adds a detector and a guide clock per phone, and aeon-karaoke#335's frame size is the larger cut on that side |
| Short-frame resynthesis                                                           | 5.3 flat                                                          | +37 of 50 cents at 110 Hz |                                       | under-shifts a voice                                                                                                                                                                                                         |
| A filterbank, each band spliced at its own period                                 | under 10, by its paper                                            | not run                   |                                       | 200 to 1500 bands, and the harmonics lose their phase relation                                                                                                                                                               |
| Even and odd harmonics split by a half-period comb, each spliced at half a period | max 0.75 periods                                                  | not run                   |                                       | the mean stays half a period, and the comb leaks when the reading is off                                                                                                                                                     |
| Read ahead into the jitter buffer's slack                                         | up to the playout delay on an `rtp` slot                          | not run                   |                                       | a contract change in section 3 and ingest's workstream; nothing for a `capture` slot. Worth its own issue                                                                                                                    |

## How it fits beside other work

- **#67 and #90** change the reading and the control signal, not the shifter. #67 reads
  `delaySamples()` to project the voice forward, and a lower delay makes that projection shorter.
  Rebased onto either, the one line in `PitchCorrector` becomes
  `shifter.setVoiced(reader.voiced())`.
- **The octave for #48's higher and lower voices.** A floor that follows the ratio lets an octave
  up pay its own floor, 133 samples, only while it shifts an octave, and a voice and its correction
  would ride one sawtooth. Moving the octave in still needs the shift range opened to 18 semitones,
  the crossfade shortened for a high voice shifted far, and the two-period search for a reading an
  octave high, all from decision 0004. None of that is here, so higher and lower still wait.

## Not measured

- How any of it sounds. The blind listen in #116 holds off, the corrector before #112 and this one.
- Mouth to speaker with correction on, which needs `measure-latency` on hardware (#116).
- Whether a rest move is heard. It is a splice inside unvoiced sound, a breath or a consonant's
  tail, where there is no pitch to break.
