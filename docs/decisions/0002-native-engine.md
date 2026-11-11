# 0002: A native engine owns the output callback

**Status:** accepted, 2026-10-31

## Decision

The mixer is a native engine behind the TypeScript package. Rust, bound with napi-rs, owns the
audio device's output callback and everything that runs on the audio thread. The DSP leaves
that already exist in C++, the pitch corrector from Aeon Karaoke's `lib/dsp` and the
signalsmith stretcher, are compiled into the same addon behind small C shims. TypeScript
carries the contract and the control plane and never touches a sample.

The engine lives as long as the process, opens the device once and renders without pause. A
mixer is the graph for one song and is swapped in and out under the running callback. The
graph is a fixed topology: slots, a backing path, a master.

[docs/design.md](../design.md) is the design this decision commits to.

## Why

- A JavaScript mix cannot promise a 128-frame callback. Garbage collection and event-loop
  jitter are longer than the 2.67 ms block, and Node has no realtime thread.
- GStreamer spent the budget where we cannot reach it from a launch string: a 20 ms platform
  sink, a 40 ms jitter buffer, two convert stages per slot, a 10 ms output block. The contract
  document puts the device-held total at 70 ms today and 20 to 30 ms with an engine that owns
  the sink.
- Rust for the engine because the engine is mostly concurrency: a control thread and an audio
  thread sharing parameters, commands, events and graphs. The compiler refuses a data race
  there. The DSP leaves are buffer-in, buffer-out functions; each costs about a hundred lines
  of shim, not a rewrite, and the corrector's measured behaviour comes with it unchanged.
- napi-rs because Aeon Karaoke installs this package into Node and Electron with no toolchain,
  and napi-rs builds a prebuilt addon for every platform, loads the one that matches, and
  generates the TypeScript types from the Rust. A release ships them all in one tarball
  (design.md 8.0).
- The device opens once because every reopen is a pop and, on Windows in exclusive mode, a
  fight with every other app. A room-lifetime engine also puts the reaction sounds and the echo
  canceller's reference where the audio already is.
- A fixed topology because the karaoke graph is known. A general graph engine is machinery with
  no customer.

## The invariants this changes

AGENTS.md's never-compromise list changes in two places with this decision.

- **The latency budget binds the mic path, not the whole audio path.** The backing path runs on
  its own playhead and the engine reports its offset, so latency there is compensated by the
  lyric clock rather than heard. Key shift, time stretch and fades may cost latency on that
  path. Nothing on a slot's path may.
- **The audio thread allocates nothing, locks nothing, calls nothing in the OS and never
  panics.** Owning the callback only delivers the budget if the callback always returns in
  time. An allocation or a lock in it is a glitch waiting for a busy moment.

## Alternatives

- **Keep GStreamer and tune it.** Its sink and jitter buffer have floors a launch string cannot
  move, and the lesson list in the contract document is the cost of living with it.
- **C++ throughout, with miniaudio or PortAudio for devices.** One language, a mature device
  layer, and the team has shipped disciplined realtime C++ in `lib/dsp`. Weaker on thread
  safety, which is where this engine's bugs will live. We flip to this if the kernels
  workstream finds the FFI costing more than it saves.
- **Rust with cpal for devices.** cpal opens WASAPI in shared mode, a 10 ms engine period on
  Windows, and has no exclusive path. Per-OS backends instead, chosen after the sink
  measurement in 0003.

## What this does not decide

- The device backend per OS, and shared versus exclusive on Windows: 0003, from measurements.
- Whether echo cancellation ships: the calibration probe decides.
- The offline stretcher: signalsmith first, an A/B against Rubber Band before a licence.
- Stem storage on a LAN box with little memory.
