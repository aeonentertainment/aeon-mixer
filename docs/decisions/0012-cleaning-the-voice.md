# 0012: A high-pass, gate and compressor clean the voice, with no lookahead

**Status:** accepted, 2026-11-11

## Decision

A slot's voice can be cleaned by three stages that read no sample ahead and hold none back. A
host turns them on together, per slot, as one effect, `cleanup`:

| Stage      | Where                             | What                                                                                    |
| ---------- | --------------------------------- | --------------------------------------------------------------------------------------- |
| High-pass  | after echo cancel                 | 12 dB per octave Butterworth at 80 Hz                                                   |
| Gate       | after the high-pass               | follows the room's level; opens 7 dB over it, closes under 4, holds 150 ms, 18 dB range |
| Compressor | after the meter, before the voice | 3:1 over -30 dBFS RMS, 12 dB knee, 1 ms attack, 60 ms and 600 ms release, 6 dB makeup   |

Every number is a default with its reason below; a phone recording or a listen that shows a better
one changes it.

## Why no lookahead, no frame and no suppressor

The budget is 20 to 30 ms mouth to speaker with effects on (decision 0008), and the stages
measured so far already sum past it. A stage that buffers is a cost every singer pays on every
sample, so these stages read only samples they have already passed. What that costs is named per
stage below.

A frame-based noise suppressor was ruled out. RNNoise works on 10 ms frames, so it waits 10 ms
before it can start, and WebRTC's audio processing, its gain controller included, runs on 10 ms
frames too. That is a third to a half of the whole budget for one stage. Spectral suppression is
the only thing here that would remove noise under the singing, and it stays out until a number
shows the budget can hold it.

## The stages

**High-pass.** Live vocal practice starts a high-pass at 80 to 100 Hz with a 12 dB per octave
slope, and goes steeper or higher for plosives and wind. A causal IIR adds no samples, but its
phase shift delays the fundamental near the corner, so the slope and corner were chosen on that:

| Filter            | Delay of a 110 Hz fundamental | Of 220 Hz | Loss at 82 Hz (E2) | At 50 Hz |
| ----------------- | ----------------------------- | --------- | ------------------ | -------- |
| 2nd order, 80 Hz  | 1.8 ms                        | 0.41 ms   | 2.8 dB             | 8.8 dB   |
| 2nd order, 100 Hz | 2.0 ms                        | 0.54 ms   | 5.1 dB             | 12.3 dB  |
| 4th order, 80 Hz  | 3.8 ms                        | 0.73 ms   | 2.6 dB             | 16.4 dB  |

These are group delays of the digital filters at 48 kHz, which match the analog formula for a
2nd-order Butterworth. The 4th order costs a 110 Hz voice another 2 ms of its fundamental, and an
82 Hz one 7.2 ms against 2.7, to buy 8 dB more on handling noise. 100 Hz takes 5 dB off a low E.
The corrector reads voices down to 82 Hz, so 2nd order at 80 Hz.

**Gate.** Live and broadcast gates use hysteresis (a close point a few dB under the open point),
a hold of about 100 ms, an attack under 5 ms and a release of a few hundred ms, and a limited
range rather than a mute, so a misjudged breath or tail comes out quieter rather than gone. This
one does all four. What it does differently is its threshold: a fixed threshold is wrong for a
phone at the lips and one a foot away, and for a quiet room and one with the backing in it. So
the gate tracks the room's own level, falling at once to anything quieter and rising by 6 dB a
second while closed and 0.25 dB a second while open, so ten seconds of soft singing raise the
floor 2.5 dB and do not close the gate on themselves. Both the room and the voice are read by the
same level follower, so the crest of a noise does not read as singing.

Having no lookahead costs a soft onset its first milliseconds. A note 10 dB over the room that
starts as a step opens the gate 4 ms in, and those 4 ms come out up to 18 dB down. A note 20 dB
over it opens within a millisecond. At stream start the floor is -50 dBFS, so on a louder room the
gate stays open, never closed, for the second or two it takes to learn it.

**Compressor.** A compressor with no lookahead lets an onset through before it reacts, and one
that reacts within a cycle of the voice distorts it. Both were measured on a 2 ms two-pole level
detector against a single fast one:

| Detector, attack      | Harmonics on 82 Hz, 12 dB of reduction | A 30 dB onset overshoots by | For   |
| --------------------- | -------------------------------------- | --------------------------- | ----- |
| single pole, 1 ms     | -38 dB                                 | 8.6 dB                      | 4 ms  |
| two poles, 2 ms; 1 ms | -58 dB                                 | 12 dB                       | 6 ms  |
| two poles, 3 ms; 5 ms | -66 dB                                 | 13 dB                       | 17 ms |

The middle row is the default: 20 dB cleaner than the fast detector, and an onset back in line
within 6 ms. The master's soft clipper is what stands behind the overshoot. Release is
program-dependent: a fast branch lets go of a peak in 60 ms and a slow one charges over 300 ms and
lets go over 600, and the larger wins, so a 20 ms shout releases within 250 ms while a phrase
holds its gain steady between syllables instead of pumping.

Makeup is gain on the mic, and gain on the mic is what feeds back. So the makeup is a cap: the
stage never raises a voice by more than 6 dB at any level, and a voice at -21 dBFS RMS passes at
unity. Between lines the gate's 18 dB is under it, so a slot nobody sings into is 12 dB quieter
with cleanup on than off. A 12 dB swing, -30 to -18 dBFS, comes out 5 dB wide.

## What it costs

No stage buffers: a click through a slot with cleanup on leaves on the frame it entered. The
high-pass's phase shift is still a cost on the mic path, and it is counted as one. A voice's
fundamental comes out late by the table's figure, 1.8 ms at 110 Hz and 2.7 at 82, while the
harmonics that carry most of what a singer hears are all but on time: 0.1 ms at 440 Hz. Until a
listen says how much of that a singer hears, it counts against the 20 to 30 ms at the
fundamental's figure, and the implementation quotes `measure-latency`'s mic-path number with
cleanup on beside the budget, as decision 0008 asks of every stage. CPU, on the
release build through `measure-corrector`: 0.11 % of one core per slot, from 0.34 to 0.45 % for a
slot whose corrector is bypassed. Off, each stage fades to unity over one block and then does
nothing.

## Where they sit, and where the meter reads

Pitch correction guidance for live sound puts the corrector after the gate and before EQ and
compression, and this order follows it. The high-pass and gate go before the corrector, so its
detector hears less rumble and reads the room between lines as silence (it calls anything under
-60 dBFS silence), and the corrector has nothing to shift there. The compressor goes after it:
the detector is blind to level above that silence floor, so compressing first would change nothing
it reads.

The compressor goes before the voice stage. It levels the singing itself, and the voice is the
character the singer chose on top of it, the order a vocal processor keeps between its tone
section and its effects. Keeping it beside the meter also keeps the meter just ahead of it.

The meter stays after the corrector and moves to before the compressor. The level words describe
the singing; a compressor's whole job is to hide how loud the singing is, so a meter after it
would tell a whisperer and a belter the same thing. It reads after the gate, so between lines it
reads the singer's silence and not the room.

## What it did to the corrector

#43 found that noise 20 dB under the singing drops how often the corrector engages. Cleanup does
not bring that back: through `measure-corrector`, `engaged_pct` in the `noise-20db` row moved by
-0.5 to +0.8 points across the four clips, inside what the start offsets move. That is expected.
A gate or compressor scales voice and noise together, so the detector's signal-to-noise ratio
does not change, and an 80 Hz high-pass removes about 4 % of this noise's power.

What does move it is filtering the detector's input, not the voice. A 4th-order low-pass at 1 kHz
on what YIN reads, leaving the audio alone, raised `noise-20db` engagement from 78 to 85 % on the
lowest clip and from 81 to 95 % on the best. It also raised false engagement by 2 to 8 points,
the median error by about 1.5 cents, and took 4 points off within-a-semitone on vocadito 2. The
detector is the kernel's, so that is its own change, weighed there.

## What this does not decide

- Whether the default numbers sound right. They were chosen on measurements; a listen through a
  real room and a phone recording decides them.
- Whether a host turns cleanup on by default. It is off until a host asks, like every effect, and
  a house mic with its own outboard chain may want it off.
- A slot's knobs. There are none until a host needs one; a field can be added.

## Sources

- [Using gates and expanders](https://www.soundonsound.com/techniques/production-techniques-using-gates-expanders),
  Sound On Sound; FabFilter Pro-G [time controls](https://fabfilter.com/help/pro-g/using/timecontrols).
- High-pass on vocals:
  [will a mic's high-pass counter proximity effect](https://www.soundonsound.com/sound-advice/q-will-mics-high-pass-filter-counter-proximity-effect),
  Sound On Sound, and [ProSoundWeb](https://www.prosoundweb.com/?p=142224).
- Attack within a cycle and distortion:
  [the shortest attack time](https://www.audiomasterclass.com/blog/how-to-set-the-shortest-attack-time-in-your-compressor),
  Audio Masterclass.
- Pitch correction's place in a live chain:
  [natural-sounding correction for live performance](https://www.antarestech.com/blog/autotune-in-gospel-and-worship-music-getting-natural-sounding-correction-for-live-performance),
  Antares.
- RNNoise's frame: [its 10 ms minimum](https://gitlab.xiph.org/xiph/rnnoise/-/issues/9) and
  [the demo](https://jmvalin.ca/demo/rnnoise). WebRTC's
  [gain controller](https://webrtc.googlesource.com/src/+/HEAD/modules/audio_processing/gain_controller2.cc)
  works on 10 ms frames.
