# measure-corrector

The pitch corrector's numbers, printed beside the GStreamer baseline in Aeon Karaoke's
`docs/open-problems/auto-tune.md`, a JSON result to diff, and the blind-listen set. It runs the real
slot chain on the offline driver and opens no device or socket. Why it is built this way, and how
far to trust each number, is [decision 0004](../../../docs/decisions/0004-corrector-measurement.md).

It needs two things it does not carry:

- **vocadito** (CC BY 4.0, [zenodo.org/records/5578807](https://zenodo.org/records/5578807)),
  unzipped outside this repo, 58 MB.
- **The golden fixture**, Aeon Karaoke's `app/cloud/pipeline/fixtures/golden`, for the golden vocal
  row and the listening set. The run works without it and says so.

## Run the rows

```bash
cargo run --release -p measure-corrector -- run \
  --vocadito ~/Library/Caches/aeon-mixer/vocadito \
  --golden ../aeon-karaoke/app/cloud/pipeline/fixtures/golden \
  --out before.json
```

It takes about a minute. Build with `--release` or the CPU row says it is not a number to quote.

## Before and after in a PR

```bash
git switch main       && cargo run --release -p measure-corrector -- run ... --out before.json
git switch your-branch && cargo run --release -p measure-corrector -- run ... --out after.json
cargo run --release -p measure-corrector -- diff before.json after.json --tolerance 0.05
```

Every row but CPU is deterministic on one machine, so anything the diff shows is the change. Quote
the diff. Three things decide whether a move means anything:

- `start_offset_lowest_cents` and `start_offset_highest_cents` are the row over start offsets under
  5 ms, which nobody would hear. A change inside that span is not a change. It is half a cent on
  vocadito 2 and more than 6 cents on vocadito 10, which has 25 scored notes.
- `annotator_2_median_cents` is the row against the other annotator's notes. A change that shows
  against one annotator and not the other has found the annotation, not the voice.
- The listen decides whether something sounds better. These rows only say what moved.

To compare settings without changing code, add a variant:

```bash
... run ... --variant quick:melody:1:10 --variant slow:melody:0.6:80
```

To add a condition or a standard variant, add a line to `Condition::standard` or
`Variant::standard` in `src/variant.rs`, and every row picks it up. `--variant` needs no
recompile, but a new knob needs a field on `Variant` that `Variant::params` passes along.

## The conditions

| Label             | What happens to the voice before the slot                                                                                                             |
| ----------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `direct`          | nothing                                                                                                                                               |
| `noise-20db`      | synthetic room-like noise 20 dB under the singing, steady, for #50                                                                                    |
| `opus-lan`        | a phone's Opus (20 ms, 32 kb/s, voice mode, DTX and in-band FEC, as `useAudioProducer.ts` and libwebrtc's defaults set it) and the real jitter buffer |
| `opus-lan-no-dtx` | the same with DTX off, which says what DTX costs                                                                                                      |
| `opus-wifi`       | the same over a LAN losing 5 percent of packets, with 30 ms spikes on 3 percent and neighbours swapped on 5 percent                                   |

A vocadito row under `opus-*` is vocadito through the codec path, not a phone clip. It says what
the codec path costs.

## The blind listen

```bash
cargo run --release -p measure-corrector -- listen \
  --vocadito ~/Library/Caches/aeon-mixer/vocadito \
  --golden ../aeon-karaoke/app/cloud/pipeline/fixtures/golden \
  --out ~/Library/Caches/aeon-mixer/listen/clips \
  --key-out ~/Library/Caches/aeon-mixer/listen-key
```

It refuses to write inside a repo, over existing files, or with the key beside the clips. Hand a
listener the clips folder and nothing else: it holds [LISTENING.md](LISTENING.md), which is the whole
procedure, the sheet and the audio credits. Then score the sheets:

```bash
cargo run --release -p measure-corrector -- score --sheet all-sheets.csv --key answer-key.json
```

The score prints each take's measured added motion beside what people reported, which is how the
objective measure gets checked against ears.

## Recording the phone clip

Only a person can record this. The tool never fakes one, and the row says it is pending until there
is one.

**Who.** One person who sings the way a room of friends does, not a trained singer. Their misses
are the point.

**The guide.** The golden fixture, so the take has notes to be scored against. Play `backing.mp3`
and `vocals.mp3` together from a laptop into one wired earbud, with the other ear free. Wired,
because Bluetooth adds a delay that the singer would chase. The phone must not hear the guide. Listen
through three times before recording, and sing the melody, not your own harmony.

**The phone.** The one that sings in the room: at the lips, about 5 cm away, held the way a singer
holds it. Airplane mode, so nothing interrupts.

- iPhone: Voice Memos, with Settings, Apps, Voice Memos, Audio Quality set to Lossless. Stereo off.
  Enhance Recording off on the recording.
- Android: any recorder that saves WAV or another lossless format at 44.1 or 48 kHz, with noise
  reduction, gain control and any "voice" or "speech" mode off.

The app captures with `echoCancellation`, `noiseSuppression` and `autoGainControl` off, and these
settings aim at the same thing. Say in the PR which phone, OS and app you used, because a recorder's
own processing is not something this tool can see.

**The takes.** A quiet room. Press record, start the track within two seconds, and sing it through
twice, about 40 seconds. The tool finds where the singing sits against the guide, to within 3 seconds,
and prints the offset it found. A second take with the backing on the room's speakers at singing
level is worth having: that is what the corrector hears in a real room.

**Then.** Copy the file somewhere outside this repo and run:

```bash
cargo run --release -p measure-corrector -- run ... \
  --phone ~/Library/Caches/aeon-mixer/phone/take.m4a \
  --phone-guide ../aeon-karaoke/app/cloud/pipeline/fixtures/golden/pitch-guide.json
```

The clip goes through real Opus encoding and the real jitter buffer, and its row prints beside the
others. Add the same `--phone` flags to `listen` to put it in the blind set.
