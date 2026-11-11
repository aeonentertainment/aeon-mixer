# Aeon Mixer

A low-latency audio mixer for [Aeon Karaoke](https://github.com/aeonentertainment/aeon-karaoke).
Phones on the LAN, wired mics and wireless mics are the microphones. The mixer takes those mic
streams and a backing track and plays one mix to the room. Aeon Karaoke's device service will
depend on this package in place of its GStreamer engine, so the public API here is a contract,
not an experiment.

This repository is public and the package is released from it. Write every doc, comment, issue
and commit as public text, with nothing about Aeon Karaoke's cloud, customers or keys in it.

The engine is a native core that owns the audio device's output callback, behind a TypeScript
API. [docs/design.md](docs/design.md) is the design: read its shared sections once, then the
section for the module you are changing. [docs/decisions](docs/decisions) holds the reasons.

Below the never-compromise list, every rule here and every number and mechanism in
docs/design.md is a default with its reason. When a change makes the mixer better, now or in a
year, make it and rewrite the default in the same PR, saying why. A contract other workstreams
build against lands first, in its own PR, so they follow. The never-compromise list moves with
a decision record and the developer's agreement.

## Glossary

- **Engine**: the process-lifetime owner of the audio device and the audio thread. It hosts one
  mixer at a time and plays the room's reaction sounds between songs.
- **Mixer**: one mixing graph for one performance. Slots in, one mix out. It is swapped into the
  running engine with a fade, never by reopening the device.
- **Slot**: one mic input with its own decoder, effect chain, gain and pan. Aeon Karaoke binds a
  channel to a slot.
- **Ingest kind**: how a slot's audio arrives. `rtp` is Opus over LAN UDP from a phone or from
  mediasoup, `l16` is raw PCM over loopback from a house mic, `push` is PCM the host hands in.
- **Backing path**: the stems a singer performs over, on their own playhead. Key, speed, guide
  vocal, seek and fades live here and may add latency. The mic path may not, except the corrector
  on a slot whose singer asked for it.
- **Audio thread**: the OS callback that renders one block. **Control thread**: Node's event
  loop, where every API call runs.
- **Block**: 128 frames at 48 kHz, 2.67 ms. The unit of every render and every ramp.
- **Offline driver**: the test clock. It calls render in a loop with no device and no socket.
- **Kernel**: a C++ DSP leaf compiled into the addon: the pitch corrector, the stretcher.
- **Latency budget**: mouth-to-speaker time on the mic path, with the singer's effects on. Target
  20 to 30 ms, where a singer stops hearing the speakers as a late copy of their own voice.
  Decision 0008 has the evidence.
- **Sounding incredible**: what the corrector and the voice stage are for. A singer who sings
  well already does and needs neither; one who does not, or who is shy, turns correction on.
  Each is the singer's own choice, per slot.
- **Device**: the LAN box or desktop app that hosts the engine. A house mic plugged into it is
  as real a slot as a phone.

## Things we never compromise

1. **The latency budget.** Every buffer, queue, resample and effect on the mic path is a cost
   against 20 to 30 ms. Add one only with a measurement that shows the budget still holds. The one
   exception is pitch correction a singer turns on, which ships at GStreamer parity under
   decision 0019 until the measurements there bring it back inside.
2. **The audio thread allocates nothing, locks nothing, calls nothing in the OS and never
   panics.** Audio never crosses to JavaScript. How parameters, commands and events cross is
   docs/design.md section 2.
3. **The mixer runs with no network beyond the LAN.** No cloud calls, no credentials, no
   telemetry. Anything the cloud needs to know is Aeon Karaoke's job.
4. **The public API is versioned.** A breaking change to an exported type or behaviour is a
   deliberate major bump with a note in the decision log, never a side effect of a refactor.
5. **No `any`.** Inferred types first, annotations where inference cannot reach.

## Ways to hurt yourself

1. **Killing by pattern.** Never `pkill -f` or kill a PID found by name. Kill only a PID you
   captured at spawn.
2. **Grabbing the developer's audio device.** Tests run on the offline driver. Only the
   measurement tools in `tools/` open a real device, and only when a person runs them.
3. **Blocking the control thread.** A wait on the audio thread or on a render job holds Node's
   event loop and starves the device service. Await the event or promise instead.
4. **Editing another workstream's files.** Parallel work is split by directory in
   docs/design.md section 10. In a shared file, add your own line and leave the rest.
5. **Running a fork's pull request on our own runners.** It would run a stranger's code on that
   machine. Every job that can name our runner either skips a fork or sends it to a
   GitHub-hosted one, and the repository holds outside contributors' runs for approval.

## Commands

- `pnpm install` after cloning or pulling, then `pnpm exec vp run build:native`: the TypeScript
  tests load the addon it builds. With a global `vp`, `vp install` and `vp run build:native` do the
  same. pnpm, a Rust toolchain and cmake are the things to install yourself: `opusic-sys`
  builds libopus from source with cmake.
- `vp check` formats, lints and type-checks the TypeScript. `vp test` runs its suite.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` in
  `native/` do the same for Rust.
- All of them are cheap. Run them before calling work done, and quote the output in the PR.
- `vp <name>` is a built-in; `vp run <name>` is a `package.json` script. They can differ.

## Verifying

- Test behaviour, not wiring. A DSP test feeds a known buffer and asserts the output within a
  tolerance. No tests that assert a callback was called or mirror the implementation.
- The offline driver is the clock in every engine test. Packets, stems and parameter changes
  are fixtures; the test pumps blocks and asserts the output.
- Every realtime `process` and `render` runs under the allocation guard in tests. A guard
  failure is a bug in the code, never a reason to loosen the guard.
- Timing claims need a number. A latency change ships with the measurement that justifies it,
  taken with the tools in `tools/`.
- Never wait on sleeps or polling in tests. Await the event or promise that marks the milestone.

## Pull requests

- Never open a PR unless the developer asks.
- One concern per PR. If the description needs "also", split it.
- One thread is one PR. A thread starts in its own worktree from `main`, carries one issue to
  one merged PR, and ends. More work is a new thread, so a review of either stays small.
- Titles are conventional commits in plain language: `fix(rtp): jitter buffer no longer drops
  late frames`.
- Body: the problem in a sentence or two, how it was fixed, then a "Verified" paragraph naming
  what ran. End with the model and harness that did the work.
- Rebase onto `main` before opening. Open a real PR, not a draft.
- Branch commits are working notes, two sentences maximum. Every PR squash-merges, so the PR
  title and body become the one commit on `main`.
- Work is tracked in GitHub issues. The merged PR is the record. Do not commit plans, notes or
  checklists.
- Agent PRs merge on green once proven end to end in Aeon Karaoke; the `land-pr` skill has
  the steps and the changes that stop for the developer.

## Documentation

- Most code changes need no doc change. Agents can read the code.
- `docs/design.md` is the engine's shape and the contracts between its modules. A change to a
  contract is its own PR, merged before the code that needs it, so parallel workstreams build
  against one truth.
- `docs/decisions/` holds the reasons behind choices that span files: the engine, the
  transport, the API shape. One numbered file per decision. When a decision changes, rewrite
  its file; do not append a second account.
- A comment above a function says how it is used when that is not obvious. Comments say why,
  never narrate control flow.
- README.md describes the shipped package, never planned work.

## Taste

- Keep things simple. Build the smallest model that makes the correct behaviour unsurprising.
  The mixer is a fixed topology, not a graph engine.
- Domain-driven design with hexagonal boundaries where it earns its keep: the engine crate is
  pure and knows no device, socket or Node; devices, transport and the binding are adapters at
  the edge. One class per file.
- `unsafe` lives in `native/kernels` and `native/devices`, beside the foreign code it wraps.
