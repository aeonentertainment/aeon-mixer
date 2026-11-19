# bench

The mixer alone on a real clock, with no host application around it. `bench play` plays a
session bundle exactly as `replay` plays it on the offline driver, sample for sample, and says
whether the clock got every block in time. docs/design.md 8.9 has the contract.

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
| `output.renderMaxMs` | the slowest block's render, against the output's period  |

A run kept up when nothing underran and every render beat its period. A run that cannot stand
for the bundle's song, an engine fault, an output that went away, or a command ring that filled,
is refused with its reason and measures nothing.

`--write` saves the numbers as a scorecard file and `--baseline` compares a run with one, under
the scorecard's rules: a person sets each tolerance. A bench figure belongs to its machine, so
compare runs on one machine only. `--out` writes the run's bundle, which `replay` plays back to
the same samples and `scorecard` scores.

Try the fixture: `cargo run --release -p bench -- play tools/scorecard/fixtures/bundles/duet
--backing tools/scorecard/fixtures/stems`.
