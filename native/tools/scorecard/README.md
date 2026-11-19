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

| number                | what it is                                                                                    |
| --------------------- | --------------------------------------------------------------------------------------------- |
| `late`                | packets that arrived after their samples fell due                                             |
| `concealed`           | frames the decoder filled in                                                                  |
| `playoutDelayMs`      | the jitter buffer's delay at the end of the song                                              |
| `correctorDelayMs`    | how far the corrector held the voice, the mean over the blocks it held any                    |
| `pitchErrorCents`     | the median distance from the guide note of each reading with one, at least 0.9 sure and 82 Hz |
| `correctedErrorCents` | the same median after correction: where the corrected voice sat                               |
| `letGoes`             | times correction let go of a guide note and took it again while the note held                 |
| `echoRemovedDb`       | the echo canceller's removal, the median of its findings                                      |
| `bleedDb`             | the echo it found against what the room was played, the median of its findings                |
| `howlCatches`         | times the howl guard ducked                                                                   |
| `levelTopTenthDb`     | the 100 ms level 10 percent of the meter's readings are above                                 |
| `levelMedianDb`       | the median 100 ms level                                                                       |

The master has the two levels. A number with nothing to read is left out: pitch error with
correction off or no guide, corrector delay with correction off, the echo numbers before the
canceller has found the room. A `push` or `capture` slot has nothing recorded and scores nothing.

`pitchErrorCents` counts a `pitch` reading only when it has a `targetMidi` and passes the
corrector's own bar, a confidence of 0.9 and 82 Hz. It reads the voice as it reached the
corrector, so it says how far the singer was from the guide. `correctedErrorCents` reads the same
readings less the shift the voice at each one's centre came out with, so it says how far the
corrected voice was. `letGoes` counts each gap in the corrector's pull inside a run of readings on
one guide note, with a pull either side: a voice that stops as its note ends lets go of nothing.
The shift and the pull are on the engine's reading and not yet on the binding's `pitch` event.
The levels count every reading, silence too, as `replay`'s table does.

## The paired rows

A slot whose bundle holds `reference/<n>.wav` and `reference/<n>.json` (docs/design.md 8.11) is
a paired take: the same singing recorded at once by the phone and by a reference vocal mic at
the same distance. Its rows say how far from the mic the phone sounds, and how much the tone
stage changes that (decision 0029). The scorecard replays the slot alone with no stems, once
with its stage off and once as it ran, lines the mic's recording up with it by level, and reads
each on the mic's moments of singing, s and room. So a pair is sung with reverb and voice off.

| number                                                  | what it is, for `off`, `on` and, where it has one, `reference`                  |
| ------------------------------------------------------- | ------------------------------------------------------------------------------- |
| `pairDistanceDb`                                        | how far the phone's tone sits from the mic's at the same loudness; 0 is the mic |
| `pairLowDb`, `pairMidDb`, `pairPresenceDb`, `pairTopDb` | the same gap, signed: 100 to 400 Hz, 400 Hz to 2 kHz, 2 to 6 kHz, 6 to 12.5 kHz |
| `sibilanceDb`                                           | power over 5 kHz in an s, against the singing                                   |
| `plosivesPerMinute`                                     | bursts under 100 Hz that stand over the singing                                 |
| `noiseFloorDb`                                          | the room between lines, against the singing                                     |

It prints a table of them, mic, stage off and stage on, and how many dB closer the stage
brought the phone. A bundle with no `reference/` scores as before and says its paired rows were
not scored. The tests write synthetic pairs: a voice, and the same voice through known bands,
or with its s, its pops or its room at known gains. The spectral rows read the bands within
0.15 dB, sibilance and the room read their gains within 0.1, the pops cut by 12 dB count half,
and the stage with the inverse bands brings the distance from 2.5 dB to under 0.1.

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

## The fixtures

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
are a dB for a level, 5 cents of pitch before or after correction, half a millisecond of
corrector delay and two concealed frames. Late packets, howl catches, let-goes and the playout
delay have none. Slot 0's correction runs at the profile's strength, 0.6, so 22 of its 48 cents
are left after correction; at 0.3, 34 are, and nothing else moves past its tolerance.

The echo canceller and the howl guard are off in the duet. `fixtures/bundles/echo` holds the
canceller's numbers instead: a 16 s song on one house mic that hears the backing, pink noise
from `fixtures/stems`, through a room 30 ms away and 10 dB down. The room plays alone for 10 s
and a synthetic singer sings over it for the last 6. It is an `l16` slot because a phone's Opus
at 32 kb/s fills noise in where the backing's waveform was. Through a phone, the canceller took
out at most 5 dB, and its bleed read from -8.7 to -12.3 dB as the delay moved. Through PCM it
takes out 36 dB by the tenth second, and across playout delays from 10 to 60 ms its bleed moves
0.08 dB, its removal 0.5 dB, the slot's median level 1.2 dB and its top tenth 0.5 dB. The
baseline gives a dB to the
removal and each level but one, and half a dB to the bleed. The slot's median level is what the
canceller left of the room alone, near -46 dBFS, which moves further than a sung level, so it
has 1.5 dB.

The guard is off in both. On over a singer it catches the synthetic voice at some delays and
not others (#245), so `howlCatches` waits for a fixture where it holds still.

To change a fixture, run `cargo test -p replay -- --ignored writes_the_scorecard_fixture` or
`writes_the_echo_fixture`, then score it into the baseline with
`--baseline fixtures/baseline.json --write fixtures/baseline.json`, which keeps the tolerances.
