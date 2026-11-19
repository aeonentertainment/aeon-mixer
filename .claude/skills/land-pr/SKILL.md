---
name: land-pr
description: Land a PR in aeon-mixer: babysit it, prove it on the mixer's own bench, then merge on green. Use whenever you babysit, watch or finish a PR in this repo.
---

# Land a PR

Agent PRs here merge on green. The developer answers questions, not merge buttons, so a PR that
is proven and green lands without waiting for anyone. Landing is three steps, in order. Every
new push sends you back to step 1. The stop list at the end applies from the first step: the
moment one of its conditions holds, stop and ask instead of waiting for green.

## 1. Babysit to green

Follow the `babysit-pr` skill for watching, rate limits and review findings. **Green** means,
on the latest head: required checks pass, the review bots have reviewed that head, every review
thread is resolved, and the PR is neither BEHIND nor DIRTY.

Answer a finding at its **cause**. Before changing code, write the cause in your reply in one
sentence: the rule the code failed to keep. Fix the rule where it is kept, which is often not
the line the comment sits on, and prefer the fix that removes code. A fix that adds more lines
than it removes says in the reply why no existing code could change instead. A **second patch**
is a stop sign: when a review finds a bug in code you added to answer an earlier finding, patch
nothing more. Rewrite the approach so neither finding can occur, or reply with why the approach
is right and the test that proves it. A finding larger than the PR goes as a comment on the
area's `pattern:` issue (open one titled `pattern: <the shape>` only when the area has none) and
makes no change here.

**Two rounds** is the budget. Before a push that changes code, go through the three review skills
on the fix, as before the PR opened. A third round of findings is the same stop sign as a second
patch: the slice is too large or the approach is wrong, so split it or rethink it.

`babysit-pr` and `leaving-pr-comments` are user-level skills on the developer's machines. Without
them: watch through the harness instead of polling, check every bot finding against the source,
fix the real ones, reply to a false positive with a written reason and resolve it, and rebase
when `main` moves.

## 2. Prove it on the mixer's own bench

The mixer is proven by its own suites and tools, not inside a host application. A host's suite
proves the host, so it runs only where the host is what could break: a public API change. What
runs depends on what the PR touches. When it changes nothing the engine renders, reports or
exposes (docs, CI, a tool's own code), the PR comment says so and why, and CI's green in step 1
is the whole proof.

- **Every engine change: the offline suites, the scorecard and the contract suite.** CI runs all three on
  every push, so a head that is green in step 1 has passed them, and this step reads their lines
  in the head's log. `cargo test` holds the offline suites and fails when a scorecard number
  moves past its tolerance. `vp test` holds the contract suite, `tests/contract.test.ts`, which
  runs `@aeonentertainment/mixer/contract` against the package: a pass is that test passing with
  nothing in `broken`. The PR whose code changes a promise the suite checks changes that check
  in `src/contract/`. CI's scorecard step prints every number that moved, as this does in
  `native/`:

  ```
  cargo run --release -p scorecard -- tools/scorecard/fixtures/bundles --backing tools/scorecard/fixtures/stems --baseline tools/scorecard/fixtures/baseline.json
  ```

  A pass exits 0, and the PR comment names each number it prints as moved, with its cause. A
  move the PR means to make past a tolerance rewrites the baseline: the same command with
  `--write tools/scorecard/fixtures/baseline.json` added keeps every tolerance, and the PR body
  says why each changed line of `baseline.json` moved.

- **A device, a clock or the mic path's delay: the bench.** That is `native/devices/`, the timer
  or offline clock, `native/engine/src/{driver,audio_thread}.rs`, `native/engine/src/ingest/`,
  `native/transport/`, or a stage that holds a voice's samples, such as the corrector. The bench
  is the mixer alone on a real clock, and it says whether the mix kept up. Run it in `native/` on
  the head, then in a worktree of `origin/main` (`git worktree add --detach <scratch>/main
  origin/main`) on the same machine, and remove that worktree after:

  ```
  cargo run --release -p bench -- play tools/scorecard/fixtures/bundles/duet --backing tools/scorecard/fixtures/stems
  ```

  For an `l16` or `capture` slot or the echo canceller, run `bundles/echo` the same way. It runs
  on the timer clock, takes the song's length and opens no device. A pass is the head's run
  exiting 0 on a verdict that starts `kept up`. The comment gives `underruns`, `render p99` and
  `render max` for `main` and the head, and a head slower than `main` says why. A bench figure
  belongs to its machine, so compare runs from one machine only. The same run through a real
  output, built with `--features hardware` and run with `--open-device`, is a person's.

- **A public API change: the host's suite as well.** That is a PR that changes an exported type
  or a documented behaviour a host may already rely on: anything decision 0024 would give a row,
  whether it breaks or only moves a documented figure. An addition that changes nothing already
  there, such as a new export or an optional field, is not one, since no host calls it yet. The
  host's suite stays for this case because the contract suite checks the promises as the PR
  states them: a PR that changes a promise rewrites its check and passes by construction, and
  only the host's own suite shows whether the host relied on the old one. The steps:
  - Pack the PR head: `pnpm install`, `pnpm exec vp run build:native`, `pnpm exec vp pack`, then
    `pnpm pack --pack-destination <scratch>`. The lockfile needs the pnpm that `devEngines` in
    `package.json` pins. An older global pnpm fails with `ERR_PNPM_BROKEN_LOCKFILE`, so run each
    pnpm command as `pnpm dlx pnpm@<that version>`.
  - Install the tarball into a temporary Aeon Karaoke worktree. In the host's checkout:
    `git fetch origin main`, `git worktree add --detach <scratch>/host origin/main`. In that
    worktree: `npm ci`, `npm rebuild mediasoup --ignore-scripts=false`, then
    `npm install --no-save <tarball>`, which replaces the version the host pins. The host's
    `.npmrc` turns install scripts off, so mediasoup has no worker until the rebuild.
  - Run the device service's protocol suite there:
    `npx nx run @aeon-karaoke/device-service:e2e:protocol`. It builds the service first and takes
    about 20 minutes. A pass is every test passing and the command exiting 0. The suite opens the
    engine on its timer clock, so it never opens the developer's audio device. A run that goes
    silent writes `STALL.txt`, with stacks, beside the device logs in its run folder under the
    suite's `.result/`; a failed or stalled run keeps that folder, so quote it in the PR comment.
    Then `git worktree remove --force <scratch>/host` from the host's checkout.
- **Then hunt edge cases** the change is most likely to break, on the offline driver: a slot
  added or removed mid-song, a mixer swap during playback, packet loss, a DTX pause and late
  packets, every parameter at its limits, a ramp interrupted by another. Run the five most
  relevant ones under the allocation guard.
- A latency or audio-quality claim needs its number from `tools/`, taken on real hardware. Only
  a person runs those tools, so a PR whose claim rests on one stops for a person at step 3.

Done when a PR comment (per `leaving-pr-comments`) holds one table: each proof above that
applied, pass or fail, with its numbers (each scorecard number that moved and why, the bench's
three for `main` and the head, the host suite's result); each that did not apply, with why; and
each edge case, pass or fail. A failing row is a bug: fix it, push, return to step 1.

## 3. Merge

Check the stop list below. If nothing on it applies: `git fetch`, confirm the head is still the
one you proved and green, then `gh pr merge <n> --squash --match-head-commit <sha>` with that
head, so a push that slipped in fails the merge and sends you back to step 1. Then close the
issue the PR finishes, with a comment naming the PRs and where any leftover went. In the same
turn, remove the PR's worktree and mark the agent that built it as finished in the harness, so
nothing is left open.

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
