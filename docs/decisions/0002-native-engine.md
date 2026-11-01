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
- napi-rs because Aeon Karaoke installs this package from npm into Node 24 and Electron, and
  napi-rs publishes a prebuilt addon per platform as an optional dependency and generates the
  TypeScript types from the Rust.
- The device opens once because every reopen is a pop and, on Windows in exclusive mode, a
  fight with every other app. A room-lifetime engine also puts the reaction sounds and the echo
  canceller's reference where the audio already is.
- A fixed topology because the karaoke graph is known. A general graph engine is machinery with
  no customer.

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
