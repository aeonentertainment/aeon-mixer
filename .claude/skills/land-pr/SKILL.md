---
name: land-pr
description: Land a PR in aeon-mixer: babysit it, prove it end to end in Aeon Karaoke, then merge on green. Use whenever you babysit, watch or finish a PR in this repo.
---

# Land a PR

Agent PRs here merge on green. The developer answers questions, not merge buttons, so a PR that
is proven and green lands without waiting for anyone. Landing is three steps, in order. Every
new push sends you back to step 1.

## 1. Babysit to green

Follow the `babysit-pr` skill for watching, rate limits and review findings. **Green** means,
on the latest head: required checks pass, the review bots have reviewed that head, every review
thread is resolved, and the PR is neither BEHIND nor DIRTY.

## 2. Prove it end to end

The engine's own suites ran in step 1. This step runs the PR's engine where it ships: inside Aeon
Karaoke's device service.

- Build the PR head (`pnpm install`, `vp run build:native`) and install that build into an Aeon
  Karaoke worktree off its `origin/main`.
- Run the device service's protocol suite on it: `MIXER_ENGINE=aeon npx vitest run` in
  `app/device/service/test/e2e/protocol`. Its README names the six tests that assert
  GStreamer's own behaviour; every other test passes. The suite keeps the engine on its offline
  clock, so it never opens the developer's audio device.
- **Then hunt edge cases** the change is most likely to break, on the offline driver: a slot
  added or removed mid-song, a mixer swap during playback, packet loss, a DTX pause and late
  packets, every parameter at its limits, a ramp interrupted by another. Run the five most
  relevant ones under the allocation guard.
- A latency or audio-quality claim needs its number from `tools/`, taken on real hardware. Only
  a person runs those tools, so a PR whose claim rests on one stops for a person at step 3.

Done when a PR comment (per `leaving-pr-comments`) holds one table: each case, pass or fail,
and the numbers. A failing row is a bug: fix it, push, return to step 1.

## 3. Merge

Check the stop list below. If nothing on it applies: `git fetch`, confirm the head is still the
one you proved and green, then `gh pr merge <n> --squash`. Then close the issue the PR finishes,
with a comment naming the PRs and where any leftover went.

### Stop and ask instead of merging

- The PR touches the never-compromise list: a new cost on the mic path, the audio thread's rules,
  a breaking change to the public API, or a decision record.
- A claim needs a hardware measurement or a listen that no person has taken yet.
- The PR touches publishing, release or `.github/`.
- A person's review comment is unresolved, the PR carries the `hold` label, or the brief said
  not to merge.

To stop, unwatch the PR and end your turn with a message that starts **Waiting on you:**, then
the PR link, one question, the options and your recommendation. The daily inbox collects these
messages, and the developer's answer arrives in this thread.
