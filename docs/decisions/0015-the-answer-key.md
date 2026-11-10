# 0015: The original vocal is the answer key

## The problem

Whether correction sounds good rested on three listeners, and a corrector change could not be
judged by a number. Decision 0005's rows say how far pitch moved and how much the voice's movement
changed, but nothing in them knows what the singer should have sounded like. A studio vocal does:
it is a performance of its own melody. AGENTS.md's "sounding incredible" gives two things to test
against it. A singer who sings well is left as they are, and a singer who does not is brought to
the melody without damage. #83 is the brief; this is what was built and why.

## How quality is judged without a listener, and what transfers

- **PESQ** (ITU-T P.862) and **POLQA** (P.863) score telephone speech against a reference. P.862
  lists music outside what it was validated on, Muskat (DAFx 2008) found PESQ not good enough for
  time-scale changes, and both are licensed rather than open. Not used.
- **ViSQOL v3** (Chinen et al. 2020) has a 48 kHz audio mode under Apache 2.0, but its mapping to
  MOS was trained on codec artefacts, and nothing shows it reading pitch-shifted singing. It would
  bring a C++ and Bazel build into the tool. Not used.
- **Log-spectral distance and mel cepstral distortion** (Kubichek 1993) need only an FFT. Voice
  conversion and singing synthesis report MCD, and it correlates only weakly with listeners there
  (SingMOS-Pro, 2025). Both are kept, measured below for what they charge.
- **Pitch against a known answer** is how correction research scores itself. Deep Autotuner
  (Wager et al. 2020) and BERT-APC (2025) build their training data by detuning singing per note,
  up to 100 cents either way. BERT-APC reports pitch tolerance, MAE and raw pitch accuracy beside
  a MOS for accuracy and one for expression, and no spectral or artefact measure. Molina Villota
  et al. (DAFx 2023) judge a corrector by the MSE and MAE between its output's f0 and the curve it
  should have made, region by region: staircase, vibrato, free path. None of them checks a number
  against listeners. The answer key is that method on our chain.
- **Clicks** are found in audio restoration by thresholding an autoregressive model's prediction
  error (Vaseghi and Rayner 1990; Godsill and Rayner 1998). **Warble** is the restoration
  literature's flutter: pitch moving faster than any vibrato.

## The key

The raw vocal is not the key. vocadito's singers miss their notes by 17 to 30 cents at the median,
and a melody corrector rightly moves those misses, so the raw vocal would charge it for doing its
job. Each note's centre is moved onto its note first, by the degrader's shifter, with the singer's
vibrato, drift and glides left on it: a singer in tune who still sounds like themselves. The key
moves vocadito 2, 10, 19 and 6 by 12.4, 17.6, 21.8 and 28.0 cents at the median. A tuned studio
vocal would move little; the golden vocal moves 0, for a reason under "What the rows found".

## The degrader and its floor

The degrader is seeded per clip. Each note is sung 10 to 35 cents off, a drift of up to 12 cents
runs across the phrase, half the notes of 250 ms or more are scooped into from 60 to 150 cents
under over 80 to 150 ms, and 40 percent of notes of 600 ms or more get an unsteady wobble of 15 to
30 cents at 5 to 6.5 Hz. A held note's miss stays under 50 cents, so every mode can tell which note
was meant. On vocadito the degradation reads back within 2 cents of what was asked.

It bends with Signalsmith Stretch at its 120 ms preset, offline, latency taken off so the bent
voice lines up sample for sample. It is not the corrector's shifter, so the two cannot share an
artefact and cancel. The voice shifter from decision 0004 was measured as the alternative. On
vocadito 6, bent 20 cents and back:

| Shifter                 | Pitch left | LSD     | MCD     |
| ----------------------- | ---------- | ------- | ------- |
| Stretch, 20 cents       | 2.0 c      | 2.03 dB | 2.26 dB |
| Voice shifter, 20 cents | 0.7 c      | 1.24 dB | 1.89 dB |
| Voice shifter, 50 cents | 0.9 c      | 2.70 dB | 3.74 dB |

The voice shifter is cleaner at small shifts and no better at the shifts the degrader makes, and
its delay moves with the voice, which every comparison would then have to undo. Stretch it is.

The floor is the key bent by the degradation and bent back, against the key: 1.6 to 3.4 cents of
pitch at the median, 9.6 to 15.2 at the 95th percentile, LSD 1.5 to 2.8 dB and MCD 2.8 to 4.8 dB.
Stretch overshoots a shift up by about 5 percent and not one down, which is most of the pitch.
The table prints `under floor` for any restored number at or below it.

## The rows, and how far to trust each

All of them compare the output with its input on the input's clock. The slot reports its delay
every block, and a reading of the output is moved back by it; 5 ms late on a 5.5 Hz vibrato of 50
cents would otherwise read as 9 cents of error.

| Row                                  | What it reads                                                          | Trust                                                                                                        |
| ------------------------------------ | ---------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------ |
| Left alone: moved, median and p95    | the key through the slot, cents moved inside notes                     | 0 off; reads damage, but an annotated note can hold a glide the corrector rightly bends                      |
| Left alone: vibrato depth kept, rate | 0005's vibrato depth, plus the 4 to 8 Hz peak's rate                   | rate within 0.1 Hz on a half-second note in tests                                                            |
| Restored: error, held, onset, wobble | cents from the key after correction, by part of the note               | the strongest row: a known answer with a measured floor                                                      |
| LSD                                  | the whole spectrum against the key's, voiced frames in notes           | charges pitch: 20 cents alone costs 5.5 dB on vocadito 6, 10 cents 3.0 dB on a harmonic tone                 |
| MCD, 12 cepstra from 24 mel bands    | the envelope against the key's                                         | a third of LSD's pitch charge: 1.5 dB for 20 cents, 1.1 for 10; 40 bands and 24 cepstra charged twice that   |
| Clicks added per minute              | autoregressive misses in the output with none within 5 ms in the input | finds a step of 0.01 full scale in a held vowel; vocadito's own mouth clicks read 7 to 20 a minute           |
| Octave jumps added per minute        | jumps of 900 to 1500 cents between readings under 50 ms apart          | exact on a known slip                                                                                        |
| Fast warble added, p95               | 8 to 30 Hz pitch movement not in the input, 1024-sample reads          | a 15 Hz wobble reads 0.7 to 1.0 of its RMS                                                                   |
| Still notes                          | held notes moving under 3 cents RMS, output and input                  | a steady tone reads 0.2 cents; vocadito's steadiest held note 3.4, its medians 9 to 51; none reads still yet |

LSD is not the artefact number #83 hoped for. Only pitch was changed, but whatever pitch the
corrector leaves wrong or moves wrongly, LSD charges as if it were damage. A pitch-matched LSD was
built, the key bent onto the output's own pitch before comparing, and dropped: the extra bend cost
more than it took out, 3.90 dB of floor against 2.78 raw on vocadito 6. Read MCD and the artefact
rows for damage and LSD for everything that changed.

## The model-based rows

**Word error, dropped.** whisper.cpp v1.7.6 (MIT) with `ggml-base.en`, 148 MB, ran on vocadito 6,
10 and 19 with no network, 578 s of audio in 20 s on four threads of an M2 Pro. Against each clip's
lyrics it read 6, 45 and 0 percent: clip 10's words are nonsense syllables. Against the key's own
transcript, the degrader's round trip alone read 0, 0 and 8.8 percent, and a hard retune of 0 ms,
the most machined setting, read 0, 11.1 and 7.0, the same as strength 0.6. The recogniser is built
to ignore what the corrector does. It would cost a C++ build and a 148 MB model and see nothing.

**Voice similarity, dropped unmeasured.** SpeechBrain's ECAPA-TDNN (Apache 2.0, 83 MB) is trained
on VoxCeleb speech, and nothing published checks it on singing or on shifts under a semitone. It
needs PyTorch or an ONNX runtime. The envelope row reads what it would, timbre moving, with no
model. Whether the voice is still the singer's after a voice stage is #48's question, and it may
earn the model there.

## What the rows found

On `main`, #67's head and #90's head, one machine. These are readings, not verdicts.

- No build leaves a singer in tune alone. The key moves 4 to 11 cents at the median at strength
  0.6 and 7 to 18 at full, and 25 to 93 at the 95th percentile.
- At full strength `main` makes scoops worse than leaving them: on clips 2, 6 and 19 the onset
  error reads 34, 46 and 41 cents after correction against 22, 32 and 24 off. #67 brings clip 6
  to 30.
- The golden vocal's guide holds notes of 50 to 170 ms. The key reads a centre past a note's
  first 30 ms, so it moves nothing, and correction restores nothing there. A song for the key needs
  notes of a few hundred milliseconds.

## Calibration

None of these rows gates a pull request. A row gates one only after the blind listen ranks clips
the way it does, and the table says so on every run. The listen's sheets hold rankings and warble
and robotic reports per take; when they come back, each row's ranking of the same takes is
compared with the listeners', and #83 records which agreed.

## Sources

- Kubichek, Mel-cepstral distance measure for objective speech quality assessment, 1993.
- ITU-T P.862 (PESQ) and P.863 (POLQA), and the P.863.1 application guide.
- Muskat, Quality estimation of time-scale modified signals, DAFx 2008.
- Chinen, Lim, Skoglund et al., ViSQOL v3, 2020. arXiv:2004.09584.
- SingMOS-Pro, 2025. arXiv:2510.01812.
- Wager et al., Deep Autotuner, 2020. arXiv:2002.05511.
- BERT-APC, 2025. arXiv:2511.20006.
- Molina Villota, d'Alessandro, Perrotin, Dynamic Pitch Warping for Expressive Vocal Retuning,
  DAFx 2023.
- Prame, Measurements of the vibrato rate of ten singers, JASA 1994.
- Vaseghi and Rayner, Detection and suppression of impulsive noise in speech communication
  systems, 1990; Godsill and Rayner, Digital Audio Restoration, 1998.
- Radford et al., Whisper, 2022; whisper.cpp, github.com/ggml-org/whisper.cpp.
- Desplanques, Thienpondt, Demuynck, ECAPA-TDNN, 2020. arXiv:2005.07143.
