# replay

A slot's recording through the real packet ring, jitter buffer and slot on the offline driver,
beside libopus's own decode of the same packets. It opens no device and no socket, and the same
file replays to the same samples every time. docs/design.md 8.4 and 8.9 and decision 0017 have
the reasons.

```
cargo run --release -p replay -- RECORDING.pcap [--port N] [--codec opus|l16] [--playout-delay-ms MS] [--wav DIR]
```

It reads what `Mixer.recordSlot` writes, or any classic pcap of one RTP stream, such as a
`tcpdump -w` of a phone's port. Convert a pcapng with `editcap -F pcap IN.pcapng OUT.pcap`.
`--port` picks one stream from a capture of several. The playout delay defaults to 15 ms, the one
Aeon Karaoke pins.

## What it prints

```
slot0.pcap: port 40000, 393 RTP packets over 10.0 s, 8 other datagrams left out
from 192.168.1.30:50000
slot stats: pushed 393 lost 7 late 7 concealed 125 resyncs 0 jitter 7.38 ms, ring full 0

100 ms RMS, dBFS     top tenth   median   loudest   readings
libopus alone          -15.2    -15.9     -15.1        100
slot output            -15.2    -16.2     -15.1        100
slot meter             -15.2    -17.2     -15.1        102
slot - libopus           0.0     -0.2       0.0
```

- **slot stats** are the counters `stats()` reports for the slot, so they can be set beside the
  live session's logs.
- **libopus alone** decodes every packet at its RTP timestamp, with libopus's own concealment
  where no packet is. It is what the phone sent.
- **slot output** is what the slot played, read from the anchor the jitter buffer gave the first
  packet, so its windows hold the same voice as the reference's. After a resync the two drift by
  the resync's step.
- **slot meter** is the meter's own `level` readings, the `inputDb` Aeon Karaoke logs. Its windows
  start with the slot's output, not with the voice, so a voice with sharp edges reads a little
  apart from the row above.
- The columns are the 100 ms reading 10 percent of readings are above, the median, and the loudest.
  With `--wav DIR` it also writes `slot.wav` and `reference.wav` to listen to.

A slot that reads well under libopus has lost the voice in the engine. A meter that reads under
the slot's output is a meter fault. Both near libopus, and quiet, is a quiet phone.

## Recording a real session

A recording holds the singer's voice and their phone's LAN address. Make one only with the
singer's agreement. It stays on the machine where it was made, and nothing in the engine or Aeon
Karaoke sends it anywhere. Delete it when the question it answers is closed.

Aeon Karaoke records when its device service starts with `MIXER_RECORD_DIR` set to a
directory that exists, under `MIXER_ENGINE=aeon`. Each song's mixer then writes one file per phone
and house-mic slot into that directory, from the moment the song starts until it ends:
`<started>-<mixer>-slot<N>.pcap`. A slot no one sang on holds only the file header. Replay the one
with the most packets.
