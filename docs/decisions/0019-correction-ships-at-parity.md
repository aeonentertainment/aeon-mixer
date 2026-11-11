# 0019: Pitch correction ships at GStreamer parity, over the latency budget

**Status:** accepted, 2026-11-11. Cloud agreed: "Yes, but we need to further enhance auto tune
with an innovative way to reduce latency." That follow-up is #112.

## Decision

A slot whose singer turns pitch correction on runs the corrector on the mic path, with the delay
its shifter needs: about 1.2 ms at rest, up to 15 ms for a low voice. Engaged, that is a stage over
the 20 to 30 ms budget of decision 0008. A slot with correction off pays nothing, bit for bit.

This is the one exception to never-compromise 1, and it is held to these terms:

- It ships at parity with what Aeon Karaoke runs today. GStreamer's `aeonpitch` element is the
  same corrector core with the same 15 ms bound, and #43 measured this engine against it: the
  detector within a tenth of a point, intonation within about a cent, and melody sync within a
  frame.
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
30 ms mouth to speaker, or when the corrector's delay is cut enough that it does. The cuts in
reach, each with its own number:

- The playout delay chosen per slot (#56): about 11 ms off the engine's share.
- Short Opus frames from the phone (aeon-karaoke#335): about 21 ms off a phone's path.
- A shifter floor that follows the ratio, measured on the voice shifter in #47: about 1 ms off
  the corrector at every pitch.
- Faster pitch reading and a steadier note centre (#45 and #46), which change the control signal,
  not the delay. They hold for the blind listen.

#112 looks for an innovative way to cut the corrector's own delay below GStreamer parity, which is
the follow-up Cloud asked for when agreeing to this record.

Each change to the corrector quotes its delay against this record. When the exception ends, this
record is rewritten to say so.
