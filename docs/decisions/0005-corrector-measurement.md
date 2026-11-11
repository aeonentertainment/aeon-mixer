# 0005: How the corrector is measured

## The problem

Every corrector number came through GStreamer's `aeonpitch`, and the row that decides whether
correction sounds good had never run. `native/tools/measure-corrector` is what #44, #45, #46, #49 and #50
are judged against, so what it can and cannot say spans them all.

## Reaching the engaged corrector

The tool builds its slots with `Slot::new`, the slot singers get, so what it measures is what ships.
Until #44 the slot forced the corrector's strength to zero and only a second constructor lifted that.
A cargo feature was the other choice and is worse: a workspace build unifies features, so `cargo test`
would have built the addon's engine with correction live before anyone had decided to ship it.

The tool renders `Graph` on `OfflineDriver` directly rather than through `AudioThread`, because the
delay row reads the slot between blocks. What it skips is the command drain and the swap fade, which
neither touch a voice in steady state.

## What is measured, and against what

**Intonation is a note against the note meant, and the note meant is an opinion.** The rows keep the
harness's score so they sit beside the baseline: each note's median pitch against its rounded
annotation, annotator 1. But vocadito's two annotators agree at only F = 0.64 (Bittner et al. 2021),
and annotator 1 labels ornaments as notes. Scoring against annotator 2 moves a row by up to 7 cents,
so both print. In-tune share uses the -10 to +15 cent tolerance BERT-APC takes from the intonation
literature, since a median cannot say how many notes landed.

**A median has a noise floor nobody had measured.** Starting a clip under 5 ms later moves the
median on vocadito 10 (25 scored notes) by up to 8 cents, and on 2, 6 and 19 by half a cent to 3.
The floor prints with the row, so a PR cannot claim what the hop's phase gives for free. The same
sensitivity moves the golden vocal's delay: p95 10.9 to 12.0 ms in the harness's key of C over four
start offsets.

**Raw pitch accuracy is not enough.** Luo et al. 2020 found that melody extractors with higher raw
pitch accuracy scored worse with listeners, and that voicing recall correlated positively: frames
called unvoiced hurt more than slightly wrong ones. Their pipeline resynthesises from the contour,
so this is a pointer rather than a law. It costs one column to take the hint: `engaged_pct` is how
many voiced frames clear the corrector's own 0.9 bar and 82 Hz floor. It runs from 82 to 94 percent
where raw pitch accuracy reads 95 to 98.

**Movement is measured apart from pitch centre.** Melodyne's split, centre, drift and modulation, is
the model, and BERT-APC's listeners score pitch accuracy and expression preservation separately for
the same reason. A note's contour between 3 and 12 Hz says how much it moves in the band where a
wobble is heard as one. `motion_kept` and `vibrato_kept` compare that with the voice. Molina Villota
et al. 2023 judge correctors by comparing the corrected trajectory with the original, which is the
same idea on a trajectory.

## Warble, and how far to trust the measure

PESQ and PEAQ score codecs against a reference and nothing shows them predicting correction
artefacts, so neither is used. No published measure predicts warble. `motion_added_p95` is ours: on
the worst sustained notes, how many cents RMS more the output moves than the voice did. It is a
physical quantity, and three checks say it reads what it claims:

- A wobble injected into a still voice reads back at its level within 10 percent, from 5 to 40 cents.
- The textbook warble reproduces: vibrato across the midpoint of two notes at a hard retune flips the
  target every half cycle, and the output moves half as much again as the voice. A 40 ms retune
  smooths it away, and the measure sees both.
- It tracks settings on real singing. Added motion is 0 off, 2.8 to 13.1 cents at strength 0.6 and
  6.7 to 19.4 at full, on every clip.

Its floor is 0 for the voice against itself, up to 1.5 cents through Opus on a clean LAN, and up to
8.5 under Wi-Fi concealment.

The vibrato rows say something the issues had wrong. Antares' manual has a zero retune suppressing
vibrato, and #46 assumed a short retune flattens it. Here, the detector's delay keeps the correction
behind the voice, so at retune 0 a 50 cent vibrato keeps 53 percent of its depth at 4 Hz, 74 at
5.5 Hz and 115 at 10 Hz. At a 40 ms retune it grows: 110 percent at strength 0.6 and 121 at full,
5.5 Hz. A feedforward correction of strength s, one-pole retune and a 21 ms read delay predicts 111
and 123, so the mechanism is understood. Whether anyone hears it is for the listen.

**So: a regression alarm, not a verdict.** It has never been compared with a listener, so nothing
says what value is audible, and a reading of 13 cents on vocadito 6 says only that this note deserves
a listen. `score` prints it beside each take's warble reports so the comparison can be made once
people have listened. `envelope_added_p95_db`, level movement from the shifter's splices, reads
under 0.1 dB on direct takes against 0.5 to 2.5 dB under Wi-Fi, so splices show little as level
movement. That rules out one mechanism and proves nothing about what a listener hears.

An agent cannot hear. Nothing here claims a take sounds good.

## Determinism and CPU

Every row but CPU is a pure function of the code, the data and the machine's float path, so a diff
shows the change and nothing else. Opus's SIMD paths mean a number may differ between machines, so
compare runs from one machine. CPU is thread time over audio time, the median and best of five, and
is the one row a diff lists last.

## Sources

- Bittner, Pasalo, Bosch, Meseguer-Brocal, Rubinstein, vocadito, 2021. arXiv:2110.05580.
- Luo, Lin, Su, Toward Expressive Singing Voice Correction, 2020. arXiv:2010.12196.
- Salamon et al. and mir_eval: raw pitch accuracy, voicing recall and false alarm.
- Molina Villota, d'Alessandro, Perrotin, Dynamic Pitch Warping for Expressive Vocal Retuning, DAFx 2023.
- BERT-APC, 2025. arXiv:2511.20006.
- Antares, Auto-Tune Realtime Advanced manual: retune speed, humanize, flex-tune.
- Wager et al., Deep Autotuner, 2020. arXiv:2002.05511: reference-free correction, and no listening result to compare with.
