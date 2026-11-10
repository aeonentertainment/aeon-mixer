# 0014: The first voices move formants only

**Status:** accepted, 2026-11-10, ahead of the code for #48.

## Decision

A slot gets a voice stage, a new `SlotEffect` of kind `voice` with one knob: `formantSemitones`, how
far the formants move from where the singer has them, from -12 to 12. It runs
[decision 0004](0004-voice-shifter.md)'s `VoiceShifter` with no transposition, after the corrector
and the meter and before the reverb, and adds no sample. Off, or on at 0, it hands the voice over
bit for bit.

The API carries a knob, not voice names. Which voices a singer sees, what they are called and how
they are set are presets, and presets live with the host. Two voices ship first:

| Voice    | Pitch | Formants          | Tilt | Added delay | CPU, one slot, M2 Pro |
| -------- | ----- | ----------------- | ---- | ----------- | --------------------- |
| Deeper   | 0     | -2, a factor 0.89 | none | 0 samples   | 0.78 to 0.81 %        |
| Brighter | 0     | +2, a factor 1.12 | none | 0 samples   | 0.79 to 0.83 %        |
| Off      |       |                   |      | 0 samples   | 0.003 %               |

CPU is the kernel at 48 kHz in 128-frame blocks over vocadito clips 2, 4, 10, 11, 19 and 23, release
build, across two runs. Nobody has heard either voice. Each needs a listen before it ships, and the settings above
are where the listen starts, not where it has to end.

Higher and lower, an octave up and down, wait. They will come through the corrector's shifter, so a
voice and correction cost one delay and not two.

## Why the octave voices wait

Decision 0008 gives the mic path 20 to 30 ms, effects included. A formant move costs none of it. A
pitch move through this stage's own shifter would cost the bound in decision 0004: an octave up
16.5, 13.0, 7.9 and 2.6 ms at 82, 110, 220 and 880 Hz, any shift down 13.9, 10.4, 5.3 and 1.4 ms. On
vocadito an octave up averaged 4.3 to 6.9 ms per clip and reached 15.9; down, 2.6 to 5.3 and 13.8.

The corrector already holds the voice in a delay line of its own, up to 15 ms. A slot with
correction and an octave voice on would pay both, up to 31.5 ms for an 82 Hz voice, over the whole
budget before a phone or the network is counted. A corrector's shift and an octave are both a
ratio, so one line can read the voice at their product and the octave adds nothing that correction
does not already cost. The corrector's shifter is where that line is, and work on it stands on #67
and #90, which hold for the corrector's blind listen. Whether that shifter stays today's
`PitchShifter` or becomes this kernel is that work's to decide. Until it lands, the stage moves
formants only.

## Why the pitch knob is not in the contract yet

A `semitones` knob held at 0 would be a field that does nothing. A host would build a menu on it,
and the knob's home is not settled: a transposition that rides the corrector's shifter may belong
with the corrector's knobs, or need the corrector engaged, or apply only with a guide. Adding a
field later breaks no host. Removing or moving one is a major bump under AGENTS.md. So the knob joins
the contract with the work that makes it do something.

## When the octave voices come

The research for them is done, and recorded here so that work starts from it.

**An octave, and only an octave.** A singer performs over a backing track in a key. A transposition
of anything but a whole octave moves every note off it: four semitones up turns a C major melody
into an E major one over a C major band. People also sing songs written for another voice an octave
from the record, so an octave up puts a man singing a woman's song back in the record's register,
and an octave down does the same the other way. Earlier work for #47 rendered a listen set at plus
and minus four semitones. Those judge the kernel on its own and are wrong as voices over a song.

**Formants 3 semitones with the octave**, +3 up and -3 down: about the gap between adult men's and
women's formants, below. An octave with formants held keeps a large throat on a high voice; with
them following all 12 semitones it is the chipmunk, which TC-Helicon and Roland both give a
separate formant control to avoid.

**What the stage measured with this kernel**, before the octave was taken out:

- Pitch within 0.8 cents of the asked octave on a synthetic vowel from 82 to 330 Hz.
- An octave up on 82 to 165 Hz voices: envelope 1.1 to 1.2 dB RMS from the one asked for.
- An octave down on a 440 Hz voice kept the singer's own formants more than the asked ones: 9.9 dB
  from the asked envelope, 4.2 from its own. A voice that high shows its envelope at 440 Hz steps,
  which decision 0004 also found.
- Shifting down, a delay line reads half its input and splices over the rest. A one-sample click
  after a phrase was lost in 9 to 21 of 24 placements across 82 to 440 Hz. Consonants last longer
  than a click, and whether onsets suffer is for a listen.

## Why the formants move as they do

Pitch and formants carry different things. In Smith and Patterson (2005) vocal tract length, which
sets the formants, drove how large listeners judged a speaker to be, and pitch and formants together
decided man, woman, boy or girl. Hillenbrand and Clark (2009) shifted men's sentences toward women's
values and found that pitch alone or formants alone moved the judgement only part of the way; both
together moved it. Assmann, Dembling and Nearey (2006) found frequency-shifted speech sounded most
natural when pitch and formants moved together the way they covary across real voices, and Assmann
and Nearey (2008) found vowels shifted both the same way were identified better than ones shifted
in opposite directions.

Across adults, women's formants sit about 18 % above men's averaged over the vowels (Fant 1966, who
also notes the scaling is not uniform), about 3 semitones, while their pitch sits about 9 semitones
higher.

Deeper and brighter move formants 2 semitones with no pitch change. That is less than the 3 that
separates men and women, so the voice should read as the same person larger or smaller and not as
someone else. The listen set carries 1, 3 and 4 beside 2, so the people listening choose the size
instead of taking this one.

## No spectral tilt

A tilt is an equaliser, a shelf that brightens or darkens without moving any resonance. It would
change the sound and not the speaker, and the kernel matches its output's level to its input's, which
a tilt after it would undo. Moving the formants up carries the spectrum's energy up with them, which
is the brightness a voice change needs. A tilt can join `VoiceParams` as a field later without
breaking anything, if the listen asks for it.

## What other voice processors do

| Unit                         | Pitch                  | Formants                                       | Delay published                                 |
| ---------------------------- | ---------------------- | ---------------------------------------------- | ----------------------------------------------- |
| TC-Helicon Perform-VE, Morph | Shift, ±12 semitones   | Gender, male to female, separate from pitch    | none found                                      |
| TC-Helicon VoicePro          | separate from formants | formant offset, up toward a female or chipmunk | none found                                      |
| Roland VT-4                  | ±1 octave              | formant, minus masculine, plus feminine        | none found                                      |
| Antares Mutator              | ±24 semitones          | throat length, under 1 raises the formants     | none found                                      |
| Antares Throat               | none                   | formants only                                  | none found                                      |
| Eventide H90 Harmonizer+     | per voice              | formant control, with a high latency mode      | main path 3.8 to 4.5 ms; mode figures not given |
| Eventide VocalShift          | correction             |                                                | about 25 ms low, 50 ms high                     |

Every one of them gives pitch and formant as separate controls and leaves the presets to the person,
which is what this API does. None publishes the delay of a formant move on its own. This kernel's is
zero, and that is why deeper and brighter can be voices under a budget that has no room to spare.

## What the stage does on known signals

Measured through the stage on a synthetic /a/ with every harmonic to 10 kHz, which the stage's tests
hold to. Envelope error is the RMS in dB of the output's harmonics under 5 kHz against the envelope
asked for, over those within 30 dB of its loudest.

| Voice    | Sung at      | Pitch moved     | Envelope error | Against the singer's own |
| -------- | ------------ | --------------- | -------------- | ------------------------ |
| Deeper   | 82 to 220 Hz | within 0.6 cent | 0.8 to 2.2 dB  | 3.6 to 4.8 dB            |
| Brighter | 82 to 220 Hz | within 0.4 cent | 0.5 to 1.4 dB  | 4.1 to 5.0 dB            |

- **No delay.** A click after a phrase, and the onset of a tone after silence, leave a formant voice
  on the sample they entered, so the stage holds back no sample. The filter is causal, and like any
  resonance its phase moves near the formants it moves. On a steady vowel from 82 to 440 Hz the
  output lines up with the input best at no lag for brighter, and 2 to 9 samples late for deeper,
  under 0.2 ms. That is phase, the same thing an equaliser does, not a held sample.
- **Peaks rise at the same power.** On the listen clips the two voices' peaks reached up to 3.3 dB
  over the input's. The master's soft clipper takes anything over full scale; nothing hard-clips.
- **Level holds.** Across the bench clips the level changed by 0.22 dB or less.

## Where the meter and the readings sit

Pitch readings come from the corrector, ahead of the voice, so the display's pitch trail and the
score judge what the singer sang. The meter reads before the voice too. Level words tell the singer
about their singing, and the corrector's output is the singing. The difference is small either way,
since the kernel matches its output to its input, so the choice is about meaning. It agrees with the
cleanup stages in #87, whose meter reads before their compressor for the same reason.

## `stats()`

A formant move adds no delay, so the stage reports none and `SlotStats` gains no field for it. When a
pitch voice comes through the corrector's shifter, `correctorDelayMs` and `correctorMaxDelayMs`
already report that line, on decision 0009's terms.

## Rejected

- **A pitch voice through this stage's own shifter.** It costs a second delay on top of the
  corrector's, above.
- **A `semitones` knob held at 0.** A field that does nothing, whose home is not settled, above.
- **Voice names in the API.** The engine would have to ship a release to rename or retune a preset,
  and every unit above leaves presets to their owner.
- **A character knob now.** A robot voice and a hard-tuned sound are later voices, and neither is a
  formant move. The field comes with the first character that needs it.
- **Formants as a factor.** The kernel takes a factor, and Antares names theirs as a throat length,
  where a smaller number raises the formants. Semitones run the same direction as a pitch knob and are
  symmetric: +2 and -2 are the same distance.
- **The meter after the voice.** It would describe the effect.
- **The voice before the corrector.** The corrector reads pitch and corrects the singer's own voice;
  it has no use for moved formants.

## Not measured

- Whether either voice sounds good. Nothing here has been heard.
- Whether 2 semitones is a large enough move to be worth a menu entry, or 3 or 4 still the same
  person.
- Any hardware figure. A formant voice adds no sample, so `measure-latency` (#61) should read the
  same with it on as off.

## Sources

- D. R. R. Smith and R. D. Patterson, "The interaction of glottal-pulse rate and vocal-tract length
  in judgements of speaker size, sex, and age," JASA 118(5), 2005,
  [PMC2346770](https://pmc.ncbi.nlm.nih.gov/articles/PMC2346770).
- J. M. Hillenbrand and M. J. Clark, "The role of f0 and formant frequencies in distinguishing the
  voices of men and women," Attention, Perception, & Psychophysics 71(5), 2009.
- P. F. Assmann, S. Dembling and T. M. Nearey, "Effects of frequency shifts on perceived naturalness
  and gender information in speech,"
  [Interspeech 2006](https://www.isca-archive.org/interspeech_2006/assmann06_interspeech.html).
- P. F. Assmann and T. M. Nearey, "Identification of frequency-shifted vowels," JASA 124(5), 2008.
- G. Fant, "A note on vocal tract size factors and non-uniform F-pattern scalings," STL-QPSR 7(4), 1966.
- TC-Helicon Perform-VE and VoicePro manuals; Roland VT-4 owner's manual; Antares Mutator and Throat
  documentation; Eventide's H90 manual and its staff's forum posts on H90 and VocalShift latency.
- The papers were read at abstract level and through the passages that cite them. The delays and CPU
  figures are this repository's own.
