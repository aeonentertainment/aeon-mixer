# 0026: The session bundle, one song in a directory

**Status:** accepted, 2026-11-14, ahead of the code in #180.

## The problem

Decision 0017 made a slot's packets replayable. A replay of them still has to be told everything
else by hand: `replay` takes the playout delay as a flag and knows no effect, guide or backing. So
a recording answers "what did this slot decode" and cannot answer "what did the room hear", two
songs cannot be compared, and a change to the engine cannot be tried against real singing.

A replay of a whole song is missing five things: which effects were on and when they changed, the
melody guide, where the backing was, what `stats()` said live, and where the blocks fell against
the packets. 0017 names the last one: "the phase of the blocks against the live callback is not in
the file".

## A directory of plain files

`manifest.json`, `profile.json` (decision 0027), one pcap per recorded slot, `events.jsonl`, `stats.jsonl` and a `host/` directory
the engine never reads. docs/design.md 8.11 has the contents.

- **The pcaps are 0017's, unchanged in format**, so Wireshark and libwebrtc's tools still open
  each one.
- **JSON lines for the rest.** Each line is one write, so a process that dies leaves a file that
  reads to its last line, as the pcap does. A person reads it with no tool, and any language
  parses it.
- **`host/` in place of a field.** A host learns things about a song after it starts, such as who
  sang and whether they finished. A directory it writes when it likes needs no API here, and the
  engine cannot misread what it never opens.

Rejected:

- **One archive or container file.** It has to be finalised, so a crash loses the song, and the
  pcaps stop opening in other tools.
- **A database file.** A dependency, for data that is written once and read in order.
- **Storing `level` and `pitch`.** They are the engine's output, not its input. A replay makes
  them again, and keeping the live ones would invite comparing a change against itself.
- **Copying the stems in.** A song is tens of megabytes a stem and may not be the host's to copy.
  A hash says which file it was, and a replay that is not given the stems still has the mic path.

## One clock

The audio thread calls nothing in the OS (never-compromise 2), so it cannot stamp a wall-clock
time. It can count frames. It stores the count once a block in an atomic, and the control thread
reads that count and the wall clock together: on every event, and once a second with the stats.

Those pairs are the block phase 0017 lacked. A replay turns a packet's arrival time into a frame
through the pairs either side of it. The pairs also show the output's clock drifting against the
wall clock, which nothing recorded before.

The pair is read on the control thread, so it is good to the audio callback's own jitter and no
better. A packet read at a block's edge can replay a block away from where it played. That is
2.67 ms on one packet, against a playout delay of 3.67 ms or more, and it is the same from one
replay to the next, which is what comparing two engine versions needs.

Rejected: **the network thread reading the frame count as it reads each datagram.** It is exact
and costs one atomic load. A classic pcap record has nowhere to put it, so it would be a second
file per slot kept in step with the first. It is the next step if replays are seen to differ from
the live song in ways that matter.

## Asked for at `createMixer`

A field on `MixerConfig`, not a method. A mixer can sit armed with phones already sending, and a
recording that begins at a later call misses those packets and the settings made before it.
`recordSlot` stays for the case it was built for: one stream, turned on while chasing one problem.

## Privacy

A bundle holds every singer's voice in a song. Three things keep it to what a replay needs:

- **Sender addresses are written as 0.0.0.0.** `replay` parses the datagram and never reads where
  it came from. `recordSlot` keeps the address, since a single recording is often made to debug a
  network.
- **No path and no device id.** The manifest names the output and hashes the stems.
- **The package sends nothing** (never-compromise 3). A bundle is made only when the host asks,
  and the host owns the directory and answers for each singer's consent, as 0017 now says of a
  recording.

## What it costs the mic path

One atomic store a block on the audio thread. On the network thread, the tap 0017 measured at 0.4
to 0.8 µs a datagram at the median, for every slot at once where 0017 measured one. The code PR
quotes `tap_cost` with eight slots recording. Everything else is the control thread's.

## Not yet

- A `push` or `capture` slot's PCM. A house mic has no packets, so format 1 lists the slot as
  unrecorded and replays it as silence.
- Scoring a replay against a baseline. `--json` is the output a scorer would read.
