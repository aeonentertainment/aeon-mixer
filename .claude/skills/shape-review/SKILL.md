---
name: shape-review
description: Judge whether a pull request is built the simplest way
disable-model-invocation: true
argument-hint: <pr-number>
allowed-tools: Read, Grep, Glob, Bash(gh pr view:*), Bash(gh pr diff:*), Bash(gh issue view:*), Bash(git rev-parse HEAD)
---

Judge the shape of pull request #$ARGUMENTS: whether it is the simplest way to build what it
builds, and whether the patterns it follows deserve following. `/bug-review` judges whether the
code works and `/pr-review` whether it follows AGENTS.md, so a bug or a broken rule is theirs. A
finding is a **simpler shape**: a concrete other way to build the same behaviour that removes
code, a concept or a special case.

1. Read the PR: `gh pr view $ARGUMENTS --json title,body,comments,commits` for the title, body,
   earlier review summaries and the commits pushed since, then `gh pr diff $ARGUMENTS`. A diff
   that touches only prose docs gets the one-line summary "Docs only, no shape to judge" and the
   review ends there. The tree you read must be the PR's head. CI checks it out for you. In a
   local run, `git rev-parse HEAD` must equal `gh pr view $ARGUMENTS --json headRefOid`; when it
   differs, end with "Check out the PR's head first".
2. Write the **goal**: one sentence saying what the PR makes true for a host that embeds the
   mixer, a listener in the room, or a person reading a tool's numbers. The diff is judged
   against the goal, and a simpler shape must keep it true.
3. Read past the diff. For each function, type, flag, state and module the diff adds, find the
   nearest existing code that does similar work, searching by what it does and not by its name,
   and read it. Then ask each question:
   - **Already here.** Does existing code do this, or do it after a small change?
   - **Second way.** Does the diff add a second way to do something the codebase already does
     one way: a second ring, clock, error code, stats shape or way to start a stream? Say which
     way should lose.
   - **Special case.** A flag, boolean parameter, optional field or branch that serves one
     caller, a guard stacked on a guard, a timeout tuned to hide a race, state stored that could
     be derived: what one rule makes it unnecessary?
   - **Copied pattern.** Where the diff follows a neighbour, judge the neighbour. A pattern is
     poor when each caller must remember a step, when it is repeated instead of shared, or when
     it leaks one layer into another. Count its places with Grep and name the better shape.
   - **Wrong place.** Work on the audio thread that could be done before it, a rule each backend
     repeats where the shared layer could keep it, a check every caller must remember where the
     callee could enforce it.
   - **Delete instead.** Could the goal be reached by removing or narrowing code?
   - **Patch on patch.** When the diff grew by answering review findings, are those answers
     patches around one missing rule? Name the rule.
4. Prove each candidate before it becomes a finding: name the existing code it reuses or the
   lines it removes, estimate the lines saved, and check that the goal and the AGENTS.md
   never-compromise list still hold. Drop a candidate that only moves or renames code, or that
   you prefer without being able to say what it removes. Keep at most three, the ones that
   remove the most.
5. Sort each finding. **Here**: the simpler shape is smaller than the code it replaces and stays
   inside this PR. **Later**: it reaches code this PR does not touch, so it is worth an issue
   titled `pattern: <the shape>` and no change here.
6. In CI, post one inline comment per finding on the line where the shape starts: `**Here**
   **<the simpler shape, in five words>**`, then what the code does now, the simpler shape as a
   sketch in names and sentences, and what it removes. Then write the summary into your Claude
   comment on the PR: the goal, one line per finding with a pointer to its inline comment, then
   one line per question that found nothing, saying what you compared. A PR whose shape is right
   gets "Nothing to change", and most small PRs do. A finding an earlier summary lists is not
   posted again: the summary names it as still open. A local run has neither tool: give the
   findings and the summary in the session.

Done when the summary states the goal, every question in step 3 has a line, and every finding
says what it removes.
