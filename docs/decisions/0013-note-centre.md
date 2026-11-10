# 0013: The corrector moves a held note's centre and leaves its vibrato

## The problem

The corrector had one speed: the distance from the voice to the note, scaled by the strength and
eased in over the retune time. That is a one-pole low-pass on the voice's pitch, and what it leaves
of a vibrato is `ωτ / √(1 + ω²τ²)`. At the default 40 ms that predicts 0.71 of a 4 Hz vibrato and
0.81 at 5.5 Hz. `measure-corrector` read 0.70 and 0.82 on #67's head. A shorter retune lands notes
sooner and keeps less of the singer: at 0 ms it kept 0.08. #46 asked for a corrector that settles a
note and keeps the movement inside it. Its gate: a voice 30 cents flat with 50 cents of vibrato at
5.5 Hz comes out within 5 cents of the note, with its vibrato within 10 percent.

## What it does now

`NoteCentre` decides, every reading, whether a note is landing or held.

**A note lands at the singer's retune.** After silence, at a guide's edge, or when the held note
turns into another, the correction eases in at the retune time, as before. Scoops, steps and a
voice settling onto its note are corrected the way they were.

**A note is held once the voice swings across its centre.** The centre follows the voice over 40 ms
while the note lands. When the voice has crossed it twice, the later crossing within 0.15 s and the
earlier within 0.3 s, the note is held. Then the correction eases over 120 ms, and so does the
centre. A one-pole at 120 ms keeps 0.95 of a 4 Hz vibrato and 0.97 at 5.5 Hz, and never more than
the input's depth, since `|1 − H|` is under 1 at every rate. A crossing has to pass the centre by 8
cents. A reading wobbles by up to 7 cents on a low voice, and the narrowest vibrato the expression
rows count swings 14.

**A held note picks its note from the centre.** Vibrato across the midpoint of two notes no longer
flips the target, because the centre does not cross it.

**A held voice that strays lands again.** Straying means 2.1 times the RMS of the held swing, which
is 1.5 times a sine's peak, kept between 0.4 and 1 semitone. Only a held swing teaches its size, so
a leap between notes cannot widen it. A voice that stops swinging for 0.15 s lands again too.

All of it is in the control signal. The shifter and its delay are untouched.

## What it moved

`measure-corrector run` on #67's head and on this change, on one machine. Full is strength 1 with
a 40 ms retune, natural is 0.6 with 40 ms.

| Known voice                                         | Before                 | After                  |
| --------------------------------------------------- | ---------------------- | ---------------------- |
| 30 c flat, 50 c vibrato at 5.5 Hz, full             | +0.2 c, depth 0.82     | −0.0 c, depth 0.98     |
| the same at 4, 7, 10 Hz, full                       | depth 0.70, 0.89, 0.98 | depth 0.95, 0.99, 1.01 |
| the same at 5.5 Hz, natural                         | −11.9 c, depth 0.85    | −12.0 c, depth 0.99    |
| vibrato across a midpoint, 19.7 c in, full          | 16.2 c out             | 19.2 c out             |
| the same at a hard retune                           | 41.1 c out             | 19.2 c out             |
| 100 c scoop over 150 ms from silence, full, natural | lands 39, 19 ms after  | 39, 19 ms              |
| the same from a held note                           | 39, 29 ms              | 39, 29 ms              |
| the same from a note with 50 c vibrato              | 39, −31 ms             | 89, 59 ms              |

A glide into a note reaches it within 10 cents 39 ms after the glide ends at full strength, as
before. A glide out of a held note with vibrato takes 50 to 90 ms longer, which is the cost of
keeping the vibrato. Until the voice turns, half a cycle of a 50 cent vibrato at 5.5 Hz and a 100
cent scoop in 90 ms are the same movement. Waves Tune Real-Time waits for ±50 cents over 100 ms
before it calls a move a transition, which is the same trade.

Real singing, direct, each median with its span over start offsets under 5 ms:

| Clip, setting | Median miss                  | Motion kept | Vibrato kept |
| ------------- | ---------------------------- | ----------- | ------------ |
| 2, natural    | 7.2 (6.9 to 7.2) → 7.4 c     | 0.83 → 0.90 | 0.88 → 0.92  |
| 2, full       | 2.7 (2.6 to 3.3) → 4.3 c     | 0.82 → 0.87 | 0.85 → 0.94  |
| 6, natural    | 14.8 (13.6 to 15.6) → 13.8 c | 0.87 → 0.86 | 0.86 → 0.86  |
| 6, full       | 11.9 (11.8 to 13.1) → 12.4 c | 0.85 → 0.84 | 0.84 → 0.83  |
| 19, natural   | 15.8 (15.8 to 18.2) → 17.9 c | 0.85 → 0.88 | 0.85 → 0.87  |
| 19, full      | 12.0 (11.3 to 15.0) → 12.0 c | 0.85 → 0.88 | 0.85 → 0.87  |
| 10, natural   | 13.1 (12.9 to 17.7) → 12.6 c | 0.88 → 0.91 | 0.87 → 0.92  |
| 10, full      | 11.2 (9.5 to 11.6) → 13.3 c  | 0.86 → 0.91 | 0.82 → 0.91  |

Two rows move past their span, both worse: clip 2 and clip 10 at full strength, by 1.6 and 1.7
cents. On clip 2 the cause is drift. Its singer settles 25 cents onto a long note after a wide
opening swing, and a centre averaged over 120 ms trails the settle. The per-note means move as much
as the medians, so the row's use of the median is not the cause. Clip 6's singer swings up to 1.5
semitones around a note. That passes the most a voice may stray, so those notes land, and its
vibrato kept does not move. Added motion stays under 1.3 cents on every row. Frames acted on that
the reference calls unvoiced do not move (2.1, 4.3, 9.5 and 24.6 percent), since the reading is
#67's. This changes nothing #50's low-pass trades.

Through the codec at full strength, Opus on the LAN reads 5.2, 11.3, 12.0 and 9.8 cents on clips 2,
6, 19 and 10, against 3.3, 10.7, 11.4 and 9.6. Over Wi-Fi losing 5 percent of packets it reads 4.5,
15.1, 15.8 and 16.8, against 4.7, 12.3, 12.8 and 14.2. Concealed frames are the likeliest cause, and
this did not test that. At natural strength the codec rows move by under 1.5 cents either way.

The voice gets no extra delay: off 0 ms, correcting at rest 1.23 ms, and the held-tone bound 11.2
ms at 110 Hz and 14.6 ms at 82.5 Hz, each the same as before. On the golden vocal the largest delay
seen in the song's key went from 13.1 to 13.9 ms to 9.9 to 14.0 ms over four start offsets. The
shifter's delay rides on the shift it is given, inside the same bound. CPU per correcting slot
reads 0.66 % against 0.67 %.

An agent cannot hear. The set for the blind listen in #46 puts this beside #67's corrector and
off. Whether held notes sound like the singer, and whether the slower glide out of vibrato is
heard, is for that listen.

## What was rejected, and why

| Option                                               | Number                                            | Verdict                                                             |
| ---------------------------------------------------- | ------------------------------------------------- | ------------------------------------------------------------------- |
| A fixed 60 cent stray, no swing test                 | scoop from a held note lands 219 ms after it ends | a glide under the threshold rides through at the centre's pace      |
| Restarting the centre each time a note lands         | 5.5 Hz vibrato kept 0.84                          | the centre restarts on a peak, and the swing then reads as straying |
| A centre that also follows its own trend, Holt's way | clip 2 full 12.4 c, vibrato 1.03, scoop 479 ms    | the trend picks up vibrato and adds motion                          |
| A held centre at 90 ms                               | clip 2 full 4.9 c against 4.8, 4 Hz vibrato 0.91  | drift is not lag alone; costs vibrato                               |
| One crossing to hold                                 | clip 10 full 15.4 c                               | the first sign change after a note starts is not a swing            |
| A 0.2 s swing window                                 | glide out of vibrato 119 ms against 89            | holds a glide longer, no better on real singing                     |
| A 3 cent crossing                                    | Opus LAN full 5.8, 12.1, 14.2, 12.8 c             | reading and codec jitter pass for a swing                           |
| Holding no note at a retune of 0                     | 5.5 Hz vibrato kept 0.08                          | the shortest retune would undo what this is for                     |

Each row was measured on the way, with the other choices as they stood then.

Holding at every retune changes what a retune of 0 does: notes still land at once, and held vibrato
is kept. Aeon Karaoke's device service refuses a retune of 0, so no singer reaches it today. If the
hard effect is wanted, it is a voice effect, not the corrector's floor.

## How others do it

- **zita-at1** (x42 fat1, `retuner.cc`) smooths the pitch error while the note does not change,
  0.1 s by default, from 0.02 to 0.5, and jumps when it does. Its bias, 0.5 by default, prefers the
  current note. That is a one-pole within a note, like this record's. It has no swing test, so a
  glide inside one note waits on the filter.
- **Antares Auto-Tune** applies a slower retune to the held part of long notes (Humanize). Flex-Tune
  passes notes already near the target. Antares suggests a 15 to 50 ms retune with Humanize at 10 to
  20 for transparent tuning, and publishes no algorithm.
- **Waves Tune Real-Time** has a note-transition time, 0.1 to 800 ms and 120 by default, that starts
  only once the voice is past ±50 cents for 100 ms.
- **Melodyne** splits a note offline into pitch centre, drift and modulation, and its pitch-centre
  tool weights the part of the note a listener hears most. A live path has no whole note to weigh.
- **TalentedHack**, a fork of Autotalent, slew-limits the corrected pitch, with no centre. Autotalent's
  own source was not reachable from here, and Revoice Pro is an offline tool that publishes no
  algorithm.
- Shonle and Horan found that a vibrato tone's pitch is close to the geometric mean of its extremes,
  which is the mean in cents this centre takes. D'Alessandro and Castellengo found that short tones
  weigh their ends more heavily, which is one reason a note lands before it is held.

## Sources

- Adriaensen's retuner, as carried in x42's fat1.lv2: `src/retuner.cc` and `lv2ttl/fat1.base.ttl.in`.
- Antares, Auto-Tune Realtime Advanced manual and the help center's guide to transparent tuning.
- Waves, Waves Tune Real-Time user guide.
- Celemony, Melodyne 5 manual, pitch modulation and drift.
- Salwen, TalentedHack, `pitch_smoother.c`.
- Shonle and Horan, The pitch of vibrato tones, JASA 67(1), 1980.
- D'Alessandro and Castellengo, The pitch of short-duration vibrato tones, JASA 95(3), 1994.
