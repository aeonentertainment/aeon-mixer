# 0001: TypeScript package on Vite+

**Status:** accepted, 2026-10-16

## Decision

Aeon Mixer is a TypeScript package. One tool, Vite+ (`vp`), owns formatting (Oxfmt), linting
(Oxlint), type checking (tsgo), tests (Vitest) and the build (tsdown). CI runs `vp check`,
`vp test` and `vp pack`, and the pre-commit hook runs `vp check --fix` on staged files.

## Why

- Aeon Karaoke, the only consumer, is TypeScript. A TypeScript boundary means the device service
  imports the mixer like any other package, with types flowing through.
- One tool with one config is less for an agent to hold in context than five. The same choice
  t3code made for the same reason.
- Oxlint and Oxfmt are fast enough to run on every edit, so a hook can format and lint without
  slowing a session.

## What this does not decide

The DSP engine. Today's mixer in Aeon Karaoke is GStreamer driven from TypeScript. Whether this
package drives a native engine (GStreamer, a Rust core over napi, or similar) or mixes in process
is the next decision, and it is bounded by the latency budget in AGENTS.md, not by this one.
