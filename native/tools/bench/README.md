# bench

The mixer alone on a real clock, with no host application around it. `bench play` plays a
session bundle exactly as `replay` plays it on the offline driver, sample for sample, and says
whether the clock got every block in time. `bench loopback` measures a house mic's mouth to
speaker on this machine. docs/design.md 8.9 has the contract.

```
cargo run --release -p bench -- play BUNDLE [--backing STEMS] [--profile FILE] [--out DIR] [--baseline FILE] [--write FILE]
cargo run --release -p bench --features hardware -- play BUNDLE --open-device [--output ID] [--period 64|128]
```

Without `--open-device` it runs on the timer clock: it takes the song's length, opens nothing and
nobody hears it. With it, it plays through a real output, the default one unless `--output` names
an id from `measure-latency list`. It follows no output: one that goes away ends the run.

## What it prints

What it played and on what; each slot's `late`, `concealed` and playout delay once a second; then
three numbers and a verdict in words:

| number               | what it is                                               |
| -------------------- | -------------------------------------------------------- |
| `output.underruns`   | periods the output was not handed in time                |
| `output.renderP99Ms` | the block render time 99 percent of blocks stayed within |
| `output.renderMaxMs` | the slowest block's render, against the time a block has |

A run kept up when nothing underran and every block rendered within the time a block has: the
output's period when it is shorter than a block, else a block's own 2.67 ms, since a longer
callback renders several blocks back to back. A run that cannot stand for the bundle's song is
refused with its reason and measures nothing: an engine fault, an output that went away or
failed, or a command that could not reach the engine in its block.

`--write` saves the numbers as a scorecard file and `--baseline` compares a run with one, under
the scorecard's rules: a person sets each tolerance. A bench figure belongs to its machine, so
compare runs on one machine only. `--out` writes the run's bundle, which `replay` plays back to
the same samples and `scorecard` scores.

Try the fixture: `cargo run --release -p bench -- play tools/scorecard/fixtures/bundles/duet
--backing tools/scorecard/fixtures/stems`.

## The loopback

```
cargo run --release -p bench --features hardware -- loopback --open-device --rig cable
cargo run --release -p bench --features hardware -- loopback --open-device --rig mic --air-cm 10
```

**When two runs disagree,** read this first: two openings of the same rig can differ by up to one
input period, 2.67 ms at the default 128-frame period and 1.33 ms at `--period 64`, with nothing
changed. A capture slot's wait spans a period, since a sample waits up to a period on the hardware
before its delivery, and where the input's deliveries fall against the output's wakes picks the
point in that span afresh at each opening. So give `loopback.roundTripMs` a tolerance of 2.67 in a
baseline taken at 128 frames, or 1.33 at 64. A move larger than that is the path's.

It plays decision 0006's eight chirps out of the output's left channel and reads them back on a
house mic's `capture` slot, from a cable on that channel or a mic `--air-cm` from the speaker. The
slot is never heard, so the loop cannot feed itself. Both ends are frames of the output's clock, so
the gap is the output, the cable or air, the input and what the slot holds: the path a house mic's
voice takes to the room. It finds each chirp with `measure-latency`'s detector and refuses where
that tool does, then prints four numbers and sets the round trip against decision 0008's 20 to
30 ms:

| number                 | what it is                                                  |
| ---------------------- | ----------------------------------------------------------- |
| `loopback.roundTripMs` | the median chirp's round trip, the air taken off            |
| `loopback.spreadMs`    | how far the chirps disagree; over 0.75 ms is refused        |
| `loopback.gainDb`      | the chirp as heard against as played                        |
| `loopback.snrDb`       | the weakest chirp over what the slot hears before the first |

The chirps start once the slot says it has played its input, so a slow input to open is waited
for, and a lossless loop (S/PDIF, ADAT, a virtual cable such as BlackHole) measures as well as a
mic. A run that cannot stand for the path is refused, says what to check, and measures nothing:

| refusal                                                       | what to do                                                                 |
| ------------------------------------------------------------- | -------------------------------------------------------------------------- |
| the output, or the input, would not start                     | check `--output` or `--input` against `measure-latency list`               |
| the input delivered no sound in the 5 s after the slot opened | check `--input` and `--input-channel`; on macOS, the microphone permission |
| the input is silent                                           | on macOS let the terminal use the microphone; elsewhere check the cable    |
| no chirp came back                                            | check the rig, the input channel and the volume                            |
| a chirp came back under 30 dB over the noise                  | raise the volume, bring the mic closer, or quiet the room                  |
| the chirps disagree by more than 0.75 ms                      | rerun with nothing else playing or recording                               |
| the input stopped delivering sound while the chirps played    | check the cable or the mic's connection, and that nothing else uses it     |
| a device went away, a stream failed, or the output stopped    | plug it back in or pick another device, and rerun                          |
| `--level-db is at most -3`                                    | the chirps would reach the clipper: use the default -12, or up to -3       |
| a flag `is for play`, or `is for loopback`                    | it belongs to the other command, and that run would not use it             |
| `--write` would overwrite a file that is not a scorecard      | name a new file, or an earlier result to replace                           |
| `--baseline is not a scorecard bench wrote`                   | name a file a `--write` wrote                                              |
| a reading came out as not a number                            | the distance or the level given was not a number                           |

The loopback never hands the output a sample past -3 dBFS, the cap above, whatever it was given.
