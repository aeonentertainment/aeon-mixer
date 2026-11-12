---
name: bug-review
description: Hunt a pull request for bugs it introduces
disable-model-invocation: true
argument-hint: <pr-number>
allowed-tools: Read, Grep, Glob, Bash(gh pr view:*), Bash(gh pr diff:*), Bash(gh issue view:*), Bash(git rev-parse HEAD)
---

Hunt pull request #$ARGUMENTS for bugs it introduces or leaves half-fixed. `/pr-review` judges
the PR against AGENTS.md; you judge whether the code works. A finding is a **scenario**: a
concrete sequence of calls, packets or samples that ends in a wrong result for a host that
embeds the mixer, a listener in the room, or a person reading a tool's numbers. Style, taste,
scope and missing tests belong to `/pr-review`.

1. Read the PR: `gh pr view $ARGUMENTS` for the title and body, then `gh pr diff $ARGUMENTS`.
   A diff that touches only prose docs gets the one-line summary "Docs only, nothing to hunt"
   and the review ends there. The tree you read must be the PR's head. CI checks it out for
   you. In a local run, `git rev-parse HEAD` must equal
   `gh pr view $ARGUMENTS --json headRefOid`; when it differs, end with "Check out the PR's
   head first".
2. Walk the diff once per lens. For each changed function, read its callers and callees in the
   checked-out tree until the lens is settled; the diff alone rarely shows the bug.
   - **Interleaving.** For every `await`, event, command ring and thread hand-off the diff adds
     or moves: what was checked before it, and is that still true after it? A `destroy`, a
     second `start`, a fault or a listener calling back in can land while a mixer is `starting`
     or being swapped. Name the phase each event fires in and whether the documented handler
     can act there.
   - **Order of effects.** State changed before a step that can fail (a full ring, a rejected
     swap, a failed render): is it put back, or is the engine now wrong about what is live?
   - **Buffers and time.** What fills while nothing drains it (armed, paused, stalled), and
     what plays first when draining starts? After an overflow or a drop, do timestamps,
     sequence numbers and counters still describe the audio? Check units at every boundary:
     frames, samples, channels, milliseconds. Follow a NaN or an infinity through any state
     that outlives one block.
   - **Every backend.** A request field or contract the diff touches (channel, device, rate,
     "returns once the stream runs", reported latency): confirm each OS backend honours it,
     including the ones the diff left alone.
   - **Callers left behind.** Every exported type, binding signature, stats field, CLI flag and
     documented command the diff changes: grep for the other users, including tests, tools,
     the README and `docs/design.md`.
   - **Failure paths.** A `start` or `open` that reports success before the thing runs, an
     error nobody is told about, a cached rejected promise, a flag set once and never cleared,
     a resource the engine still holds after the host let go.
   - **Numbers people quote.** In tools and reports: a warning computed but never printed, a
     measurement that includes samples outside the clip, a verdict that reads the first row
     where the score reads all of them, a path check that two spellings of one folder get past.
3. Prove each candidate against the code before it becomes a finding: name the lines that make
   the scenario reachable, and look for the guard that already prevents it. A candidate you
   cannot tie to reachable lines is dropped. Keep at most eight findings, the most severe.
4. Grade each finding. **P1**: a host using the API as documented hits it, the room hears it,
   or a tool reports a wrong number. **P2**: real, but it needs an unusual sequence or setup.
5. In CI, post one inline comment per finding on the line where the fix belongs:
   `**P1** **<what goes wrong, in five words>**`, then the scenario as numbered events ending
   in the wrong result, then the smallest fix. Then write the summary into your Claude comment
   on the PR: one line per finding with its grade and a pointer to its inline comment, then
   one line per lens that found nothing, saying what you checked. A local run has neither
   tool: give the findings and the summary in the session.

Done when every lens in step 2 has a line in the summary and every inline comment carries a
grade, a scenario and a fix.
