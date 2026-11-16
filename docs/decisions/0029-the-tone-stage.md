# 0029: The tone stage holds no sample, and caps the delay its filters cost

**Status:** accepted, 2026-11-15, ahead of the code for #205. Nothing here has been heard, and
no phone has been through it. The delays are computed from the filters' own formulas at 48 kHz.
The code that follows measures them.

## Decision

A slot gets a tone stage, a new `SlotEffect` of kind `tone`, off until a host turns it on. It is
there to make a phone sound like a good vocal mic. It runs after the corrector and the meter and
before the compressor, and it is five parts in this order:

| Part          | What                                                                    | Asked for by        | Late, at worst               |
| ------------- | ----------------------------------------------------------------------- | ------------------- | ---------------------------- |
| Plosive guard | a first-order low shelf under 250 Hz whose cut follows a pop            | `plosiveDb` 0 to 12 | 0.17 ms, only while it ducks |
| Bands         | up to 8 filters a host hands in for a mic it knows                      | `bands: [...]`      | 0.2 ms, refused over that    |
| Match         | 3 broad filters the stage moves itself, for a mic the host has none for | `bands: "match"`    | 0.19 ms                      |
| Air           | a high shelf at 10 kHz                                                  | `airDb` 0 to 6      | 0.01 ms                      |
| De-esser      | a high shelf from 5 kHz whose cut follows the sibilance                 | `deEssDb` 0 to 12   | 0.04 ms, only while it cuts  |

No part holds a sample or reads one ahead, so a click leaves on the sample it entered. Off, the
stage hands the voice over bit for bit. Every number is a default with its reason below, and a
bench recording or a listen that shows a better one changes it.

## Minimum-phase is not the same as no delay

#205 proposed that minimum-phase filters with no lookahead cost the mic path nothing. That is
true of samples and not of phase. A filter that holds no sample still delays the frequencies it
lifts, and the skirts of the ones it cuts:

| Filter, 48 kHz                         | Latest frequency | Late by |
| -------------------------------------- | ---------------- | ------- |
| Peak, +12 dB, Q 4, 100 Hz              | 100 Hz           | 19.0 ms |
| Peak, +6 dB, Q 1, 100 Hz               | 96 Hz            | 2.3 ms  |
| Peak, +6 dB, Q 2, 1 kHz                | 1 kHz            | 0.45 ms |
| Peak, +6 dB, Q 1, 3 kHz                | 2.9 kHz          | 0.08 ms |
| Peak, -12 dB, Q 4, 200 Hz              | 155 Hz           | 1.1 ms  |
| Peak, -9 dB, Q 4, 9 kHz                | 7.4 kHz          | 0.02 ms |
| Low shelf, +6 dB, 200 Hz               | 95 Hz            | 0.53 ms |
| Low shelf, +2 dB, 200 Hz               | 97 Hz            | 0.18 ms |
| High shelf, -12 dB, 5 kHz              | 2.3 kHz          | 0.04 ms |
| Low shelf, -12 dB, 150 Hz, 2nd order   | 227 Hz           | 0.45 ms |
| Low shelf, -12 dB, 250 Hz, 1st order   | 233 Hz           | 0.17 ms |
| Linkwitz-Riley split at 150 Hz, summed | 110 Hz           | 3.6 ms  |

These are group delays from 80 Hz to 16 kHz. For a peak the centre's is `2Q(A - 1/A) / (2 pi f)`
with `A` the square root of the gain, which is why a lift costs most when it is low and narrow.
The engine cannot know what a host's table holds, so a profile cannot be trusted to be cheap.
The first row is most of the budget in decision 0008 from one band.

## The two caps

`ToneParams::check` judges the bands and the air shelf as one cascade from 80 Hz to 16 kHz, and
`setSlotEffects` throws, naming the frequency and the figure, when any frequency comes out:

- **Over 0.2 ms late.** That is the phase decision 0014's voice stage already costs, the most
  this repository has shipped as "no added delay". A stage that keeps to it adds no new kind of
  cost to the mic path.
- **Lifted over 6 dB.** Gain on a mic is what feeds back. Decision 0012 caps cleanup's makeup at
  6 dB for that reason, and this is the same figure, so the two together can bring one frequency
  12 dB toward a howl and no more.

80 Hz is where cleanup's high-pass sits and just under the lowest voice the corrector reads. The
check runs in the engine, on the control thread, so the filters it judges are the filters that
run and the two cannot drift apart.

What 0.2 ms allows: any high shelf; a presence peak or a dip of 6 dB at Q 1 from 3 kHz up; a
narrow notch on a port resonance near 9 kHz; a 4 dB cut of Q 1 at 400 Hz; a low shelf that lifts
2 dB at 200 Hz, 3 at 300 or 5.5 at 500.

What it refuses, said plainly: **the warmth of a close cardioid mic.** A 6 dB low shelf at 200 Hz
leaves a low voice's fundamental 0.53 ms late and a 4 dB one 0.35 ms. It also refuses a 6 dB cut
of Q 1 at 300 Hz, 0.35 ms late on its lower skirt, and any narrow lift under 500 Hz: 3 dB of Q 2
at 400 Hz is 0.56 ms. Raising the cap is a
new cost on the mic path under never-compromise 1. It takes a listen on the bench's pairs that
says the low end is worth it, and the developer's agreement. Until then it stays out.

## Where it sits

After the corrector and the meter, before the compressor.

- **After the corrector**, so pitch readings judge the singing. Decision 0012 found that
  filtering what the detector reads changes how often correction engages, and a stage that
  differs per phone would make that differ per phone. The corrector moves a voice by a semitone
  or so and a capture's colour is broad, so the colour is still where the bands expect it. The
  match also needs this block's reading, which the corrector has just made.
- **After the meter**, so level words describe the singing and not a profile. A pop would read
  as a peak, and it does today.
- **Before the compressor**, so it levels the voice the room will hear. A compressor ahead of the
  guard would duck a whole line for a pop the guard was about to remove.
- **Before the voice and the reverb.** The voice is the character a singer chose on top of a
  clean capture, and sibilance sent into a reverb splashes.
- **After the echo canceller and the howl guard.** The canceller learns the room's path as the
  source hands it over, and the match, the guard and the de-esser all move.

Inside the stage the guard is first, so the match never averages a pop, and the de-esser is
last, so it judges the sibilance that leaves the stage, the air's lift included.

Rejected: ahead of the corrector, where it would also need last block's reading; and after the
compressor, the order a studio uses for sweetening, which here would leave the compressor
reacting to the faults the stage exists to remove.

## Bands

A band is `{ kind: "peak", hz, gainDb, q }` or `{ kind: "low-shelf" | "high-shelf", hz, gainDb }`:
`hz` 80 to 16 000, `gainDb` -12 to 6, `q` 0.3 to 8. Each is the Audio EQ Cookbook's biquad at
48 kHz, a shelf at slope 1. The contract names the formulas because a host fits its bands with a
tool of its own, and the two must draw the same curve from the same numbers. A shelf takes no Q:
nothing a capture profile does needs a resonant shelf, and a knob that does nothing is a promise.

Eight is enough for a low shelf, a high shelf and six peaks, about what a parametric fit to a
measured response uses. `Params` is fixed-size (design section 2), so the count is a ceiling and
not a list.

The engine holds no phone models. The host owns the table and whatever keys it: a model, a model
and a capture mode, or one singer's own phone. A profile is only true for audio as the mic heard
it. A phone that sends its own voice processing or automatic gain has a colour that moves with
the level, and no fixed band undoes that, so a host turns that off first.

## The match

For a mic with no profile, the stage averages the singing in a few broad bands, each against the
mean of them all so that loudness cancels, and moves three filters toward a target: a low shelf
at 400 Hz from -6 to +3 dB, a peak at 3 kHz of Q 0.7 and a high shelf at 8 kHz, each 6 dB either
way. Their worst combination is 0.19 ms late. The code trims those ranges to whatever its sweep
of every combination shows keeps both caps, air included.

It is one field with the bands, `bands: [...] | "match"`, not a switch beside them. A host that
has a profile wants it used, and one that has none wants the match. Running both would need a
rule for which wins, and nobody has asked for it. A variant can be added without breaking a
host.

**What it aims at** is one curve, an engine constant: the average spectrum of singing through
the bench's reference mic, as each band's level against the mean. A target per host would make
"a good vocal mic" mean something else in every host, and the scorecard judges against one mic.
The numbers come from the reference side of the bench's pairs (#197) and are written here when
they exist. Until then the match has nothing to aim at and does not ship.

**It cannot tell a phone from a singer.** A dark voice and a dull phone average the same. That
is why it is three broad filters with 6 dB of range and not a full equaliser, and why a profile
is always the better answer where a host has one. Its safety is these rules:

| Rule                                                                 | Why                                                                                                                                      |
| -------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------- |
| Counts a block only on a reading YIN is sure of, confidence over 0.9 | noise and an empty room get a guess YIN is unsure of, and silence gets none                                                              |
| and only 12 dB over the room's level, by a follower like the gate's  | the backing in the mic between lines is the room's level, so it is never 12 dB over itself for long                                      |
| Time that does not count moves nothing                               | a quiet room cannot drift it                                                                                                             |
| Nothing moves in the first 5 s counted                               | a few syllables are one vowel, not a voice                                                                                               |
| At most 0.5 dB for each second counted                               | slow enough that no one hears the tone move: the whole range takes 12 s of singing                                                       |
| At most 6 dB from flat, and never past either cap                    | the most a wrong answer can cost                                                                                                         |
| Reads ahead of its own filters                                       | feed-forward, so it cannot chase its own correction                                                                                      |
| Starts over only on `resetSlot`                                      | a new sender may be a new phone; a pause, a gap or a new SSRC is the same phone, and DTX makes a new stream of every pause over a second |

The follower sits on the stage's own input, so with cleanup on it reads the room after the gate.

Not known: whether these hold on a real room recording. A singer's own tilt moves several dB
between soft and loud, and a loud passage of the backing can stand over the room's level for a
second or two before the follower rises to it. The range and the rate bound what either can do,
and a replay of real rooms says whether that is enough. The match stays off by default in any
host until one has.

## The guard, the de-esser and the air

**Not split-band.** #205 proposed a de-esser and a plosive guard that split the band. A
crossover that sums back to flat is either first order or an all-pass, and an all-pass is not
minimum-phase: a Linkwitz-Riley split at 150 Hz leaves everything under it 3.6 ms late, all the
time, pop or no pop. A shelf whose gain moves does the same job, is unity at rest, and costs
phase only while it cuts.

**The plosive guard** is a first-order low shelf with its pole at 250 Hz. At 12 dB it takes 10 dB
off 50 Hz and 8 off 80 Hz, and leaves 233 Hz 0.17 ms late while it does. The second-order shelf
at 150 Hz that would be the usual choice takes 2 to 3 dB more off and costs 0.45 ms. Its detector is
the code's to choose, on two tests: a pop goes, and a bass holding a low E does not set it off.
With no lookahead the first millisecond of a pop passes, which the master's soft clipper stands
behind.

**The de-esser** is a high shelf from 5 kHz. It cuts by how far the voice over 5 kHz stands above
the rest of it, so it needs no threshold per phone and treats a quiet singer as it does a loud
one, the reasoning behind decision 0012's gate. The first millisecond of an s passes.

**Air** is a high shelf at 10 kHz, up to 6 dB. It lifts hiss as much as voice, which is why the
noise floor is a row below. An exciter is left out. It makes harmonics the capture never had,
which at 48 kHz alias unless it oversamples, and oversampling brings filters of its own. It comes
back if the bench shows captures with no top octave for a shelf to lift.

## Parameters, and why the amounts are not in the profile

`plosiveDb`, `airDb` and `deEssDb` are amounts in decibels, where 0 is off. One number each
turns a part on, says how far it may go and can be swept, so there is no separate switch.
Starting values for a host: 12, 3 and 6.

Left out, each is 0. They are not in the settings profile (decision 0027). `parseProfile`
refuses a profile with a field missing, so a new required field breaks every stored profile and
every bundle's `profile.json`, and that is a major change for values nobody has heard. They join
the profile, by that record's own rule, in the PR that first sweeps them.

## `stats()`

Each slot gains `tone`, null while the stage is off:

- `match`: where its filters stand, as `{ hz, gainDb }` each, and `countedMs`, the singing it has
  counted since it last started over. Null unless `bands` is `"match"`. A host can show that a
  match has settled, and a scorecard can see one that never engaged.
- `deEssDb`: the most the de-esser took off in the last second.
- `plosives`: how many pops the guard has ducked, as `howlCatches` counts howls.

The stage reports no delay. It holds no sample, and its phase is capped, not measured live. A
bundle already records each `setSlotEffects` call and `stats()` once a second, so it holds the
profile a song ran under and what the stage did with it.

## How it is judged

By a **paired recording**: one take captured at once by a phone, through its slot, and by a
reference vocal mic at the same distance from the mouth, on the bench (#197). How the reference
is stored beside the bundle is that work's to settle. The scorecard (#196) lines the two up and
gains these rows, each for the phone with the stage off, with it on, and for the reference:

| Row             | What it measures                                                                                   | The stage wins when                    |
| --------------- | -------------------------------------------------------------------------------------------------- | -------------------------------------- |
| `pair_distance` | RMS of the gap between the two long-term spectra, third octaves 100 Hz to 12.5 kHz, levels matched | it falls on every test phone           |
| `pair_bands`    | the same gap, signed, in four bands: 100 to 400 Hz, 400 Hz to 2 kHz, 2 to 6 kHz, 6 to 12.5 kHz     | no band moves away from the reference  |
| `sibilance`     | level over 5 kHz in unvoiced frames against the voiced level                                       | the phone moves toward the reference's |
| `plosives`      | bursts under 100 Hz that stand over the voiced level, a minute                                     | the phone moves toward the reference's |
| `noise_floor`   | level between lines against the voiced level                                                       | it rises by no more than `airDb`       |
| `tone_late`     | the stage's group delay at its settings, worst frequency from 80 Hz                                | it is within the cap                   |
| `tone_readings` | `Pitch` and `Level` events with the stage on against off                                           | they are identical                     |

Only voiced frames count in the spectra, so the room between lines is not scored as tone. Every
other row a scorecard already has must hold still. A blind listen on every test phone is the
last word, and it is one of the hands-on runs in #116.

## What stays out

- **Noise and room removal by a model.** RNNoise and WebRTC's suppressor work on 10 ms frames
  (decision 0012), a third of the budget before they start. A replayed take has no budget, so
  that is where a model belongs, unless a measurement shows one inside 20 to 30 ms.
- **The room itself.** A phone's capsule hears in every direction and a vocal mic does not. No
  filter on one channel undoes that.
- **Linear-phase filters and lookahead**, in the bands, the de-esser and the guard alike.
- **Low-end warmth past the cap** and **an exciter**, each above with what brings it back.
- **Voice processing a phone applies before it sends.** The host turns it off; nothing here can.

## Not measured

- Whether any of it sounds good. Nothing has been heard.
- Every delay above on the engine. The code measures tones through the stage against the cap.
- How much of a phone's difference from a vocal mic is tone at all, and how much is the room and
  the phone's own processing. The pairs say.
- The match's target, and whether the match is stable in a real room.
- CPU. Eight biquads and three side-chain followers per slot should be small beside the voice
  stage's 0.8 % of a core. The code quotes it.

## Sources

- R. Bristow-Johnson, [Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/), for the
  peaking and shelf biquads.
- Decisions [0008](0008-latency-budget.md), [0012](0012-cleaning-the-voice.md),
  [0014](0014-the-first-voices.md) and [0027](0027-the-settings-profile.md).
- The delays were computed for this record with SciPy's `group_delay` on those biquads at 48 kHz,
  from 80 Hz to 16 kHz. The first-order shelf is the bilinear transform of `(s + g w) / (s + w)`.
