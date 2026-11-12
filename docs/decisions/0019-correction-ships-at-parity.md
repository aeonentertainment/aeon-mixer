# 0019: Pitch correction ships at GStreamer parity, over the latency budget

**Status:** accepted, 2026-11-11. Cloud agreed: "Yes, but we need to further enhance auto tune
with an innovative way to reduce latency." That follow-up is #112.

## Decision

A slot whose singer turns pitch correction on runs the corrector on the mic path, with the delay
its shifter needs: 0.1 ms at rest, up to one period of the voice and 0.25 ms while correcting, 12 ms
for a low voice and 13.5 ms at most. Engaged, that is a stage over the 20 to 30 ms budget of
decision 0008. A slot with correction off pays nothing, bit for bit.

This is the one exception to never-compromise 1, and it is held to these terms:

- It shipped at parity with what Aeon Karaoke runs today. GStreamer's `aeonpitch` element is the
  same corrector core with a 15 ms bound, and #43 measured this engine against it: the detector
  within a tenth of a point, intonation within about a cent, and melody sync within a frame. #112
  took the shifter below parity (decision 0021) with intonation inside the same ranges.
- Correction is the singer's own choice, per slot. A host never turns it on for a singer who did
  not ask.
- The delay is reported, not hidden: `stats()` gives `correctorDelayMs` for display and
  `correctorMaxDelayMs` for budgets (decision 0009).

## Why

Cloud's instruction on 11 November 2026 was to replace GStreamer with this engine. Auto-tune is a
beta feature GStreamer ships today, so the replacement must carry it or the product loses it.
Leaving the corrector bypassed until every path fits 20 to 30 ms would mean either keeping
GStreamer or removing auto-tune. Neither is the better product.

Every mic path is already over 20 to 30 ms before correction (decision 0009), so the budget is not
met with or without this stage. The work that brings the whole path under it is the same work that
makes room for the corrector.

## Until when, and what brings it back

The exception ends when `measure-latency` (#40) shows a slot with correction engaged inside 20 to
30 ms mouth to speaker, or when the corrector's delay is cut enough that it does.

#112 was the follow-up Cloud asked for, and decision 0021 is what it found. The corrector's shifter
now pays at most one period of the voice and 0.25 ms while correcting a quarter tone: 12.4 ms at
82 Hz, 9.3 at 110, 4.8 at 220 and 1.4 at 880, against 15.0, 11.5, 6.4 and 2.5 before. Its ceiling,
`correctorMaxDelayMs`, is 13.5 ms, from 15. A rest moves the delay down, so a sung line starts at
1.1 to 1.7 ms at the median on the clips measured, from 2.2 to 6.5. A period of the voice is what
any splice that keeps the voice whole costs, so what is left of the corrector's share is the
voice's own period, and the rest of the cut has to come from the other stages:

- The playout delay chosen per slot (#56): about 11 ms off the engine's share.
- Short Opus frames from the phone (aeon-karaoke#335): about 21 ms off a phone's path.
- Faster pitch reading and a steadier note centre (#45 and #46), which change the control signal,
  not the delay. They hold for the blind listen.

Each change to the corrector quotes its delay against this record. When the exception ends, this
record is rewritten to say so.
