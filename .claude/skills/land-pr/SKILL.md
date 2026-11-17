---
name: land-pr
description: Land a PR in aeon-mixer: babysit it, prove it end to end in Aeon Karaoke, then merge on green. Use whenever you babysit, watch or finish a PR in this repo.
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
is right and the test that proves it. A finding larger than the PR becomes an issue titled
`pattern: <the shape>` and no change here.

`babysit-pr` and `leaving-pr-comments` are user-level skills on the developer's machines. Without
them: watch through the harness instead of polling, check every bot finding against the source,
fix the real ones, reply to a false positive with a written reason and resolve it, and rebase
when `main` moves.

## 2. Prove it end to end

The engine's own suites ran in step 1. This step runs the PR's engine where it ships: inside Aeon
Karaoke's device service. When the PR changes nothing the engine does (docs, CI), the PR comment
says so and why, and step 2 is done.

- Pack the PR head: `pnpm install`, `pnpm exec vp run build:native`, `pnpm exec vp pack`, then
  `pnpm pack --pack-destination <scratch>`. The lockfile needs the pnpm that `devEngines` in
  `package.json` pins. An older global pnpm fails with `ERR_PNPM_BROKEN_LOCKFILE`, so run each
  pnpm command as `pnpm dlx pnpm@<that version>`. The build rewrites `native/binding/index.js`;
  `git checkout` it before you commit.
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

Done when a PR comment (per `leaving-pr-comments`) holds one table: each case, pass or fail,
and the numbers. A failing row is a bug: fix it, push, return to step 1.

## 3. Merge

Check the stop list below. If nothing on it applies: `git fetch`, confirm the head is still the
one you proved and green, then `gh pr merge <n> --squash --match-head-commit <sha>` with that
head, so a push that slipped in fails the merge and sends you back to step 1. Then close the
issue the PR finishes, with a comment naming the PRs and where any leftover went.

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
