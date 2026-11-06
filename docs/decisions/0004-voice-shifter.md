# 0004: The voice shifter is a delay line and a zero-delay formant filter

**Status:** accepted, 2026-11-05

## Decision

`VoiceShifter` moves pitch and formants apart on the mic path with no lookahead. Pitch comes from
a delay line read at the transposition's rate, `PitchShifter`'s method. A resampled voice's
envelope moves with its pitch, so a filter after the line takes the moved envelope off and puts
the asked one on. The filter is a pair of 48-pole lattices, built from the voice's own
autocorrelation read at scaled lags, and it adds no delay. It is pure Rust with no `unsafe`, so
the allocation guard sees all of it but the detector.

The issue proposed pitch-synchronous grains, the classic answer. We built that too and measured
both. Grains lost.

## The bound

In samples at 48 kHz, for a voice of period T and a ratio β: `5 + ⌈max(0, β-1) × min(128, T/|β-1|)⌉ + 1.125 T + 2`.
The second term is what the read point gains during a jump's crossfade when reading faster than
time. T is held to 82 Hz's at the longest, so no input can exceed the 82 Hz column.

| Shift                             | 82 Hz   | 110 Hz  | 220 Hz | 880 Hz |
| --------------------------------- | ------- | ------- | ------ | ------ |
| Formants only, no pitch shift     | 0 ms    | 0 ms    | 0 ms   | 0 ms   |
| +0.5 semitones                    | 14.0 ms | 10.5 ms | 5.4 ms | 1.5 ms |
| +6 semitones                      | 15.0 ms | 11.5 ms | 6.4 ms | 2.6 ms |
| -6 semitones, or any shift down   | 13.9 ms | 10.4 ms | 5.3 ms | 1.4 ms |
| +12 semitones                     | 16.5 ms | 13.0 ms | 7.9 ms | 2.6 ms |
| Today's `PitchShifter`, any shift | 14.9 ms | 11.4 ms | 6.3 ms | 2.5 ms |

A shift of +12 at a high voice can overshoot this by up to half a millisecond. The average is
about half the bound: on vocadito clips 2, 10 and 19, mean 2.3 to 4.9 ms and worst 14.6 ms.

## What each candidate costs

Delays are for a singer's own voice, at 82, 110, 220 and 880 Hz, shifted 6 semitones.

| Candidate                                                                     | Delay                                                                                                           | Accuracy                                                        | CPU per slot, one core                                                     | Rejected because                                           |
| ----------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- | -------------------------------------------------------------------------- | ---------------------------------------------------------- |
| **Delay line and filter (chosen)**                                            | 15.0, 11.5, 6.4, 2.6 ms; formants only 0                                                                        | 0.00 cents on tones                                             | 1.00 %                                                                     |                                                            |
| TD-PSOLA / Lent grains, read lazily (prototype)                               | 12.2, 9.2, 4.6, 1.2 ms; formants x1.2 or x0.8 add 2 to 3 ms at 82 Hz                                            | limited by the detector's reading; see the note below the table | about 0.5 %                                                                | pitch follows the detector                                 |
| TD-PSOLA with whole grains built before playback, as the literature describes | two periods: 24.4, 18.2, 9.1, 2.3 ms                                                                            | as above                                                        |                                                                            | delay                                                      |
| Signalsmith Stretch with formants, at 5.3 ms block                            | 5.3 ms flat                                                                                                     | asked +50 cents: +37 at 110 Hz, +62 at 440 Hz                   | 0.66 %                                                                     | misses the note                                            |
| Signalsmith Stretch, at 21.3 ms block                                         | 21.3 ms flat                                                                                                    | asked +50 cents: +56 at 110 Hz, +66 at 440 Hz                   | 0.66 %                                                                     | misses the note and the budget                             |
| Signalsmith Stretch, default 120 ms block                                     | 120 ms flat                                                                                                     | asked +50 cents: +52 at 110 Hz, +51 at 880 Hz                   | 0.74 %                                                                     | budget                                                     |
| Rubber Band Live (R3)                                                         | documents "50 ms or more"                                                                                       |                                                                 | not run                                                                    | budget, and GPL                                            |
| Eventide VocalShift, hardware                                                 | manual: about 25 ms low, 50 ms high; "high" recommended for extreme formants                                    |                                                                 |                                                                            | budget                                                     |
| Autotalent 0.2 (formant corrector, LPC-style)                                 | reports 2047 samples, 46 ms at 44.1 kHz; its lattice is order 7 and its author calls it "somewhat experimental" |                                                                 |                                                                            | budget, order too low for 48 kHz, and GPL                  |
| Neural voice conversion: LLVC, StreamVC                                       | under 20 ms at 16 kHz, 70.8 ms on a phone                                                                       |                                                                 | LLVC runs 2.8x real time on a consumer CPU, about 36 % if that is one core | one trained target voice, a model per slot, and the budget |

Hardware harmonizers descend from Lent 1989, analysed by Bristow-Johnson 1995. IVL's patents
(US 5,231,671; US 5,567,901, the Lent method plus resampling for gender) are the same idea. No
latency turned up for TC-Helicon or Boss units, so only Eventide is in the table. The Signalsmith
rows re-run what `auto-tune.md` found at a 10 ms block, now with formants on.

On tones, the grains land within 2 cents up to 440 Hz and 9 to 11 cents at 880 Hz. That last figure
was not chased, and a better grain shifter might fix it; it did not decide this.

## Why grains lost

They hold formants in place by construction and their delay is about 1.5 ms lower at 82 Hz on a
shift down. But in a grain shifter the output's pitch is the grains' spacing, which comes from the
detector. YIN's reading is 21 ms late and takes 40 ms to catch a step. A delay line's pitch is the
input's pitch times the ratio, whatever the detector says.

| Measured, shift ±6                                                  | Grains                                    | Delay line and filter  |
| ------------------------------------------------------------------- | ----------------------------------------- | ---------------------- |
| Synthetic vibrato, 5.5 Hz, ±50 cents: output's lag behind the input | 18 to 26 ms                               | 2 to 8 ms              |
| The same: RMS from the input's track                                | 7 to 11 cents                             | 2 to 4 cents           |
| Pitch step: settled within 20 cents                                 | 51 to 61 ms                               | 11 to 21 ms            |
| Detector reads a doubled period for 300 ms                          | **an octave wrong, 91 to 94 % of frames** | within 5 cents, tested |
| Detector reads a halved period for 300 ms                           | **wrong, 83 to 89 % of frames**           | within 5 cents, tested |
| Formants x1.2 or x0.8: delay at 82 Hz                               | 14.2 to 15.2 ms                           | 0 ms                   |
| vocadito 2, 10, 19: frames within 20 cents of input times ratio     | 73 to 94 %                                | 84 to 98 %             |
| vocadito 2, 10, 19: periodicity read back against the input's 0.97  | 0.92 to 0.97                              | 0.92 to 0.97           |

The stressed cases are what a singer does. On a tone both reach their tolerances. Each has a
number only a listen can read: grains cannot blur a consonant through a filter, and the chosen
design can, so the rejected grains are in the listen set too. If the listen disagrees, this file
changes.

## Settings the measurements chose

Each of these was swept on tones with fixed formants and on vocadito. RMS is the dB error of
the output's harmonics against the envelope asked for, over 82 to 880 Hz. Each sweep ran with the
settings of its day, so read each one's direction, not its absolute figures.

- **48 poles.** 24: 5.3 dB; 32: 4.2 dB; 40: 3.4 dB; 48: 3.2 dB; 64: 3.3 dB. The low voices keep
  improving to 80 poles, 2.0 dB to 1.4 dB, at twice the cost. A voice moved down keeps less of
  what the model sees: a scaled lag falls short of what was measured. That is why shifts down read
  worse than shifts up, and past an octave down the error reaches 3 to 5 dB. Putting the filter
  ahead of the line on a shift down took 1 dB off at -6 and 2 dB at -12, offline; that needs a
  second history ring and a plan for crossing zero, so it waits for the listen to ask.
- **A noise floor of -20 dB in the model.** The most important knob here. At -35 dB a real voice's
  periodicity read back at 0.74 to 0.91; at -20 dB, 0.91 to 0.96, with tones unchanged. A model free
  to fit the valleys between formants lifts a singer's breath by 20 dB.
- **First-difference pre-emphasis, 0.97.** Without it, one clip's first reflection reached 0.9995,
  the model collapsed, and the output burst +40 dB for 50 ms. Low voices on tones gained about 1 dB
  shifted down and lost about 0.3 dB shifted up.
- **Keeping 70 % of the last reading.** A click in a short window flattens that window's spectrum.
  Without this, concealment's level swung 7 to 9 dB and periodicity read 0.70 to 0.82; with it, 2 to
  7 dB and 0.83 to 0.94.
- **The output's level is measured, not derived.** The models' own prediction errors ought to give
  the gain, and on a hiss they were out by 7 to 14 dB. Matching measured power holds it within
  0.3 dB across every clip and setting. A singer who turns brighter does not turn louder.
- **Rejected: a low-order fit to the ratio of two envelopes,** which would have cost a third of the
  lattices. 5.8 to 6.8 dB against 2.7.

## Two findings for `PitchShifter`

The line here differs from today's shifter in two ways that owe nothing to formants, and both show
at correction sizes. Today's side of these was measured on a Rust port of its method, since the C++
shifter has no shim of its own:

- A floor that follows the ratio, rather than holding room for six semitones up: about 1 ms less
  at every pitch, on both the bound and the average.
- Jumps finer than a sample. Whole-sample jumps bias a high voice by up to 1.8 cents at 880 Hz and
  two semitones, against 0.00.

## Should the corrector keep its shifter?

Yes, for small moves, for now. `PitchShifter` is this delay line's method at 0.28 % of a core. With
formants left to move, this kernel's own line is 0.32 %. Holding formants takes that to about 1 %,
to fix an envelope error of 1.5 dB at half a semitone and 5 dB at two: Aeon Karaoke calls that too
little to hear, and nobody has listened since. The two findings above belong in `PitchShifter`
regardless, and #45 or #46 can take them; they do not need this kernel.

## Sources

Moulines and Charpentier 1990; Moulines and Laroche 1995, Speech Communication 16(2);
Lent 1989, Computer Music Journal 13(4); Bristow-Johnson, AES preprint 3718, 1993;
Röbel and Rodet, DAFx 2005, on LPC modelling single harmonics of a high voice; US patents
5,231,671 and 5,567,901; Autotalent 0.2's and zita-at1's (fat1.lv2) sources; Signalsmith Stretch
1.4.0, as vendored here; Rubber Band's `RubberBandLiveShifter.h`; Eventide's forum staff quoting
the H90 manual; Sadov et al. 2023 (LLVC); Yang et al. 2024 (StreamVC). The paywalled papers were
read at abstract level; the source code and the prototypes here are what the numbers come from.
