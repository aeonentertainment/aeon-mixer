# scorecard

Session bundles replayed on the offline driver and turned into numbers, so two runs can be
compared by something other than ear. It opens no device and no socket, and the same bundle
scores to the same numbers every time. docs/design.md 8.9 and 8.11 have the reasons.

```
cargo run --release -p scorecard -- BUNDLE... [--profile FILE] [--backing STEMS] [--baseline FILE] [--write FILE]
```

Each `BUNDLE` is a bundle's directory, as `createMixer({ bundle })` writes it, or a directory of
them. `--profile` and `--backing` are `replay`'s: the first replays every bundle under another
settings profile, the second names a directory holding the songs' stems, found by hash.

## The numbers

Each is read from what the replay's own engine reported, as `stats()`, `level` and `pitch` report
it live. Per recorded slot:

| number             | what it is                                                                                    |
| ------------------ | --------------------------------------------------------------------------------------------- |
| `late`             | packets that arrived after their samples fell due                                             |
| `concealed`        | frames the decoder filled in                                                                  |
| `playoutDelayMs`   | the jitter buffer's delay at the end of the song                                              |
| `correctorDelayMs` | how far the corrector held the voice, the mean over the blocks it held any                    |
| `pitchErrorCents`  | the median distance from the guide note of each reading with one, at least 0.9 sure and 82 Hz |
| `echoRemovedDb`    | the echo canceller's removal, the median of its findings                                      |
| `bleedDb`          | the echo it found against what the room was played, the median of its findings                |
| `howlCatches`      | times the howl guard ducked                                                                   |
| `levelTopTenthDb`  | the 100 ms level 10 percent of the meter's readings are above                                 |
| `levelMedianDb`    | the median 100 ms level                                                                       |

The master has the two levels. A number with nothing to read is left out: pitch error with
correction off or no guide, corrector delay with correction off, the echo numbers before the
canceller has found the room. A `push` or `capture` slot has nothing recorded and scores nothing.

`pitchErrorCents` counts a `pitch` reading only when it has a `targetMidi` and passes the
corrector's own bar, a confidence of 0.9 and 82 Hz. It reads the voice as it reached the
corrector, so it says how far the singer was from the guide and not how far the corrected voice
is. The levels count every reading, silence too, as `replay`'s table does.

## Against a baseline

A scorecard file holds each bundle's numbers, each with a tolerance:

```json
{
  "format": 1,
  "bundles": {
    "duet": {
      "slot0.playoutDelayMs": { "value": 15.0, "tolerance": 0 }
    }
  }
}
```

`--write` writes one. Every tolerance is written as the baseline had it, or 0 for a number the
baseline lacks; a person sets them, so which movement fails is the file's to say. Given
`--baseline`, it prints each number that differs from the baseline and exits 1 when one moved
further than its tolerance or is gone. A number the baseline lacks is reported as new and fails
nothing.

## The fixture

`fixtures/bundles/duet` is an 8.5 s duet written on the offline driver, with no real singer in
it. Slot 0 holds a synthetic voice 46 cents sharp of its guide notes, with correction and reverb
on. Slot 1 holds another note on a link that loses one packet in thirty. `fixtures/stems` holds
its backing, silence that runs the guide clock. The tests score it against
`fixtures/baseline.json`, so a change that moves one of its numbers past its tolerance fails
`cargo test`.

Each voice holds its note through 700 ms of every second, so the levels, the pitch readings and
the corrector's hold read the same wherever the windows fall. Across playout delays from 10 to
60 ms, no level moves more than 0.4 dB, pitch error moves under half a cent, the corrector delay
moves a quarter of a millisecond and concealment moves two frames. The baseline's tolerances
are a dB for a level, 5 cents of pitch, half a millisecond of corrector delay and two concealed
frames. Late packets, howl catches and the playout delay have none.

The echo canceller and the howl guard are off in it. With them on and the backing in slot 0's
phone, the canceller's bleed read anywhere from -3 to -9 dB as the delay moved, and the guard
ducked the held synthetic note at some delays and not others. So neither stage gave the same
number twice under a change that should not move it. Their numbers wait for a fixture where
they hold still.

To change the fixture, run `cargo test -p replay -- --ignored writes_the_scorecard_fixture`, then
score it into the baseline with `--baseline fixtures/baseline.json --write fixtures/baseline.json`,
which keeps the tolerances.
