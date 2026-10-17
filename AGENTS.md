# Aeon Mixer

A low-latency audio mixer for [Aeon Karaoke](https://github.com/aeonentertainment/aeon-karaoke).
Aeon Karaoke can use both wired and wireless microphones, but introduces a new concept of being
able to use a mobile device on the LAN as microphones. The mixer takes those mic streams, house
mics and a backing track, and plays one mix to the room. Aeon Karaoke's device service will depend
on this package in place of its GStreamer engine, so the public API here is a contract, not an
experiment.

## Glossary

- **Mixer**: one mixing graph for one performance. Channels in, one mix out.
- **Channel**: one audio source in the mix with its own gain and pan.
- **Ingest kind**: how a channel's audio arrives. `rtp` is a phone sending Opus over LAN UDP,
  `local` is a wired or USB mic on the device, `webrtc` is the fallback relayed path.
- **Backing track**: the instrumental the singers perform over. It is a channel like any other.
- **Latency budget**: mouth-to-speaker time. Target 40 to 60 ms; under 30 ms the singer's own
  voice fuses with the room and the echo disappears.
- **Device**: the LAN box or desktop app that runs a karaoke session and hosts the mixer.

## Things we never compromise

1. **The latency budget.** Every buffer, queue and resample in the audio path is a cost against
   40 to 60 ms. Add one only with a measurement that shows the budget still holds.
2. **The mixer runs with no network beyond the LAN.** No cloud calls, no credentials, no
   telemetry. Anything the cloud needs to know is Aeon Karaoke's job.
3. **The public API is versioned.** A breaking change to an exported type or behaviour is a
   deliberate major bump with a note in the decision log, never a side effect of a refactor.
4. **No `any`.** Inferred types first, annotations where inference cannot reach.

## Ways to hurt yourself

1. **Killing by pattern.** Never `pkill -f` or kill a PID found by name. Kill only a PID you
   captured at spawn.
2. **Grabbing the developer's audio device.** Tests and tools never open the default input or
   output device. Audio in tests comes from fixtures and deterministic buffers.

## Commands

- `vp install` after cloning or pulling.
- `vp check` formats, lints and type-checks. `vp test` runs the suite; `vp test <file>` runs one.
- Both are cheap. Run both before calling work done, and quote the output in the PR.
- `vp <name>` is a built-in; `vp run <name>` is a `package.json` script. They can differ.

## Verifying

- Test behaviour, not wiring. A DSP test feeds a known buffer and asserts the output within a
  tolerance. No tests that assert a callback was called or mirror the implementation.
- Timing claims need a number. A latency change ships with the measurement that justifies it.
- Never wait on sleeps or polling in tests. Await the event or promise that marks the milestone.

## Pull requests

- Never open a PR unless the developer asks.
- One concern per PR. If the description needs "also", split it.
- Titles are conventional commits in plain language: `fix(rtp): jitter buffer no longer drops
  late frames`.
- Body: the problem in a sentence or two, how it was fixed, then a "Verified" paragraph naming
  what ran. End with the model and harness that did the work.
- Rebase onto `main` before opening. Open a real PR, not a draft.
- Branch commits are working notes, two sentences maximum. Every PR squash-merges, so the PR
  title and body become the one commit on `main`.
- Work is tracked in GitHub issues. The merged PR is the record. Do not commit plans, notes or
  checklists.
- When babysitting a PR: poll checks and comments newer than the last push, verify each bot
  finding against the source, fix real ones, dismiss false positives with a written reason.
  Stay quiet when nothing is new. Stop when the bots are green on the latest commit.

## Documentation

- Most code changes need no doc change. Agents can read the code.
- `docs/decisions/` holds the reasons behind choices that span files: the DSP engine, the
  transport, the API shape. One numbered file per decision. When a decision changes, rewrite
  its file; do not append a second account.
- A comment above a function says how it is used when that is not obvious. Comments say why,
  never narrate control flow.
- README.md describes the shipped package, never planned work.

## Taste

- Keep things simple. Build the smallest model that makes the correct behaviour unsurprising.
- Domain-driven design with hexagonal boundaries where it earns its keep: the mixing graph is
  pure, and ingest kinds and audio devices are adapters at the edge. One class per file.
- If a rule here fights the task, say so and get a human sign-off before breaking it.
