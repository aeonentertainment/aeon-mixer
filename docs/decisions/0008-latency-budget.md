# 0008: The latency budget is 20 to 30 ms, effects included

**Status:** accepted, 2026-11-07

## Decision

Mouth to speaker, the mic path targets 20 to 30 ms. The budget covers every stage on a slot's
path, including pitch correction and any voice effect the singer turns on, with one exception:
pitch correction a singer turns on ships at GStreamer parity under decision 0019 until that
record's measurements bring it inside. It replaces 40 to 60 ms in AGENTS.md's never-compromise
list.

## Why

- The singer is the listener who matters. They hear their own voice through bone conduction with
  no delay, so the speakers deliver a late copy of it. Hearing aid research on that same setup
  found the gap starts to disturb at about 20 ms (Stone and Moore, 1999), and later studies put
  the limit between 14 and 30 ms.
- 40 to 60 ms was the room's number. The precedence effect fuses a delayed copy of speech with
  the original up to about 40 ms, so the audience hears one voice. A singer at 40 ms hears the
  room fine and hears themselves late.
- Performers tolerate more delay from wedge monitors than from in-ear monitors (Lester and Boley,
  AES 2007). A room's speakers sit on the wedge side, which is why the ceiling is 30 ms and not 10. Lower is still better.
- Pitch correction and voice transforms are part of the product. A budget that exempts them makes
  the mode a singer chose the one that sounds late. That is why decision 0019's exception for
  correction is held to terms and an exit, rather than being an exemption.

## What it costs

The stages measured so far sum past 30 ms before a phone is involved: the output alone reports 9
to 20 ms on the machines in 0003, the playout delay is 15 ms, and the corrector adds up to 15 ms
for a low voice when engaged. Phone capture and Wi-Fi have no number yet. Meeting the budget takes
time out of every stage, so each change to one quotes `measure-latency` against 20 to 30 ms.

## What this does not decide

How the budget splits across stages. That waits for `measure-latency` to give the mic path a
number on real hardware.

## Sources

- Stone and Moore, delay tolerance for own voice, summarised in
  [Hearing Review](https://hearingreview.com/2010/01/acceptable-processing-delay-in-digital-hearing-aids/)
  and [Goehring et al.](https://eprints.soton.ac.uk/414900/1/Goehring_DelayTolerance_Manuscript.docx).
- Lester and Boley,
  [The Effects of Latency on Live Sound Monitoring](https://aes2.org/publications/elibrary-page/?id=14256),
  AES Convention 123, 2007.
- [Precedence effect](https://en.wikipedia.org/wiki/Precedence_effect).
