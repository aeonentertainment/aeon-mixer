# Aeon Mixer

A low-latency audio mixer for [Aeon Karaoke](https://github.com/aeonentertainment/aeon-karaoke).
Phones on the LAN, wired mics and wireless mics are the microphones. The mixer takes those mic
streams and a backing track, and plays one mix to the room.

A native engine owns the audio device's output callback. TypeScript drives it through a Node
addon, and no audio crosses into JavaScript. Mouth to speaker, the target is 20 to 30 ms. The
package ships the addon prebuilt for macOS, Windows and Linux, so installing it needs no Rust
toolchain. Aeon Karaoke's device service is its one consumer.

## Install

Each [release](https://github.com/aeonentertainment/aeon-mixer/releases) carries one tarball.
Depend on its URL, and the lockfile pins its hash:

```bash
npm install https://github.com/aeonentertainment/aeon-mixer/releases/download/v0.3.0/aeonentertainment-mixer-0.3.0.tgz
```

## Use

```ts
import { openEngine } from "@aeonentertainment/mixer";

// Opened once for the life of the process; it never reopens the device between songs
const engine = await openEngine({ output: {} });

const mixer = await engine.createMixer({
  slots: [{ kind: "rtp" }, { kind: "l16" }, { kind: "push" }],
  backing: {
    stems: [
      { kind: "instrumental", path: "song/instrumental.flac" },
      { kind: "guideVocals", path: "song/vocals.flac" },
    ],
    key: -2,
    speed: 1,
    stemGains: { instrumental: 1, guideVocals: 0.3 },
  },
  rtpReturn: { host: "192.168.1.20", port: 40000, payloadType: 111, ssrc: 0x1234_5678 },
});

mixer.slots[0]?.port; // the UDP port this phone sends Opus over RTP to
mixer.on("level", (slot, rmsDb, peakDb) => {});

const { startedAt, backingOffsetMs } = await mixer.start();
await mixer.destroy();
await engine.close();
```

- **Slots** are the mics: 8 per engine by default, up to 16 with `openEngine({ slots })`. `rtp`
  takes Opus over UDP from a phone or mediasoup, `l16` takes raw PCM over loopback from a house
  mic, and `push` takes PCM the host hands to `mixer.push`. Each slot has gain, pan, mute, solo,
  reverb, a pitch reading, a melody guide and pitch correction toward it, whose delay `stats()`
  reports.
- **The backing** is any format Symphonia decodes, resampled to 48 kHz. The armed key and speed
  are rendered before `start` resolves. Live, it supports seek, pause, resume, a key change and
  stem gains, and `start`, `seek` and `resume` report the position the lyric clock follows.
- **`rtpReturn`** sends the mix to mediasoup's PlainTransport as 10 ms Opus with in-band FEC.
  `host` is an IP address, never a name.
- **Reactions** play through `engine.reactions.play` between songs and during them.
- **The timer engine**, `openEngine({ output: { timer: true } })`, is for a box with no sound
  card. It opens no device; its own thread renders a block at each deadline on the OS's
  monotonic clock, and the mix leaves on `rtpReturn`. `measure-clock` prints its timing:
  `cargo run --release -p devices --bin measure-clock` in `native/`.
- **The offline engine**, `openEngine({ output: { offline: true } })`, opens no device and
  renders only when its caller runs `engine.pump(blocks)`. The tests run on it.

[docs/design.md](docs/design.md) section 7.2 has the full API. The reasons are in
[docs/decisions](docs/decisions).

## Development

You install Node 24, pnpm, a Rust toolchain, cmake and a C++17 compiler yourself (GCC, Clang or
the Visual Studio C++ build tools). cmake builds libopus from source, and the compiler builds the
DSP kernels in `native/kernels`.

```bash
pnpm install                     # dependencies, including Vite+
pnpm exec vp run build:native    # the addon the TypeScript tests load
pnpm exec vp check               # format, lint, type-check
pnpm exec vp test                # the TypeScript suite, on the offline engine
pnpm exec vp pack                # build dist/
```

In `native/`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`
do the same for Rust. A pre-commit hook runs `vp check --fix` on staged files;
`pnpm exec vp hooks disable` turns it off for your clone.

Agents start at [AGENTS.md](AGENTS.md). Contributors start at [CONTRIBUTING.md](CONTRIBUTING.md).
Security reports go through [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE)
