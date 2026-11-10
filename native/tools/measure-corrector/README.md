# measure-corrector

The pitch corrector's numbers, printed beside the GStreamer baseline in Aeon Karaoke's
`docs/open-problems/auto-tune.md`, a JSON result to diff, and the blind-listen set. It runs the real
slot chain on the offline driver and opens no device or socket. Why it is built this way, and how
far to trust each number, is [decision 0005](../../../docs/decisions/0005-corrector-measurement.md).

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

## The answer key

A studio vocal is a performance of its own melody, so it can be the answer to what correction
should do. Every vocadito clip, the golden vocal and each `--song` gets these rows, direct, under
every variant. [Decision 0015](../../../docs/decisions/0015-the-answer-key.md) has the research and
how far each can be trusted. They inform and gate nothing until the blind listen has ranked clips
the way they do, and the table says so.

- **The key.** Each note's centre is moved onto its note, with the singer's movement left on it.
  `key_moved_median_cents` is how far: 12 to 28 cents on vocadito, less on a tuned studio vocal.
- **Left alone.** The key through the slot. A singer already in tune should come out as they went
  in, so everything here is damage: how far pitch moved inside notes, the vibrato's depth and rate,
  the log-spectral distance (LSD) and the mel cepstral distortion (MCD) from the key.
- **Restored.** A seeded degrader makes the key sing badly: each note 10 to 35 cents off, a slow
  drift of up to 12, scoops into half the notes and an unsteady wobble on some held ones. The
  slot then follows the melody. The error against the key is split into held notes, onsets and
  wobbled notes. `off` is the degradation itself.
- **The floor.** The degrader bent and bent back, against the key. No row can claim less, and the
  table prints `under floor` instead of a number that does.
- **Artefacts,** on every take, a phone's included, as the output against its own input: clicks
  the input did not have, octave jumps, pitch movement between 8 and 30 Hz the input did not have,
  and held notes gone still. Through Opus, `off` is the codec's share.

LSD charges pitch as much as damage: 20 cents of pitch alone costs about 5.5 dB of it on a real
voice, and MCD about 1.5. A strength-0.6 corrector leaves more pitch error than a full one, so a
lower LSD there does not mean fewer artefacts. Read MCD and the artefact columns for damage, and
LSD as everything that changed.

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

## Recording the phone takes

Only people can record these. The tool never fakes one, and the row says it is pending until there
is one. #83 wants three.

**Who.** Three people who sing the way a room of friends does, not trained singers: their misses
are the point. Each says yes to being recorded for this, knowing the file stays on the machine
that runs the tool and is never committed or uploaded. A take with their voice goes nowhere else.

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

**Then.** Copy each file somewhere outside this repo and give each with its guide, in order:

```bash
cargo run --release -p measure-corrector -- run ... \
  --phone ~/Library/Caches/aeon-mixer/phone/take-1.m4a --phone-guide $GOLDEN/pitch-guide.json \
  --phone ~/Library/Caches/aeon-mixer/phone/take-2.m4a --phone-guide $GOLDEN/pitch-guide.json \
  --phone ~/Library/Caches/aeon-mixer/phone/take-3.m4a --phone-guide $GOLDEN/pitch-guide.json
```

They are `phone`, `phone-2` and `phone-3` in the result. Each goes through real Opus encoding and
the real jitter buffer, and its rows, its artefacts included, print beside the others. A phone take
has no answer key: nobody knows what its singer meant beyond the guide. Add the same `--phone`
flags to `listen` to put them in the blind set.

## Recording a song for the answer key

#83 wants at least one song the developer wrote. Only its writer can supply it.

**The vocal.** The lead vocal as sung, comped if need be, with no pitch correction on it: a tuned
vocal is a corrector's answer, not a singer's. Dry, with no reverb, delay, doubling or harmony.
Mono WAV at 44.1 or 48 kHz, exported from the start of the song so it lines up with the guide.

**The guide.** The `pitch-guide.json` Aeon Karaoke's pipeline writes for the song, the same shape
as the golden fixture's. The key reads a note's centre past its first 30 ms, so it needs notes of a
few hundred milliseconds. The golden guide's notes run 50 to 170 ms, which is why its key moves
nothing and its rows read only the degradation.

**Then.** Keep both outside this repo and run:

```bash
cargo run --release -p measure-corrector -- run ... \
  --song ~/Library/Caches/aeon-mixer/songs/my-song.wav \
  --song-guide ~/Library/Caches/aeon-mixer/songs/my-song-pitch-guide.json
```

`--song` repeats, each with its own `--song-guide`. The song is `song-my-song` in the result.
