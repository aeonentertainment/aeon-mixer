---
name: pr-review
description: Review a pull request against AGENTS.md
disable-model-invocation: true
argument-hint: <pr-number>
allowed-tools: Read, Grep, Glob, Bash(gh pr view:*), Bash(gh pr diff:*), Bash(gh issue view:*)
---

Review pull request #$ARGUMENTS against AGENTS.md. Line-level bugs are a finding only when
they break a rule below; the rules are the review.

1. Read the PR: `gh pr view $ARGUMENTS` for the title, body and linked issue, then
   `gh pr diff $ARGUMENTS`. When the body links an issue, `gh issue view` it for scope. Read
   the code around a hunk whenever the diff alone cannot settle a rule.
2. Judge the PR against each rule, in this order:
   - Things we never compromise: a buffer, queue or resample added to the mic path without a
     measurement in the body; code the audio thread runs that allocates, locks, calls the OS or
     can panic, or audio crossing to JavaScript; a network call beyond the LAN, a credential or
     telemetry; a breaking change to an exported type or behaviour without a major bump and a
     decision record; `any`.
   - Verifying: a DSP change without a test that feeds a known buffer and asserts the output;
     an engine test that opens a device or socket instead of the offline driver; a realtime
     `process` or `render` tested without the allocation guard; a timing claim without a
     number; a test that sleeps or polls; a test that asserts wiring.
   - Pull requests: one concern (an "also" is two), a conventional-commit title in plain
     language, a Verified paragraph naming what ran, no committed plans or notes, the model
     and harness line at the end. Branch commits are working notes and are not reviewed.
   - Documentation: a decision that spans files with no file in `docs/decisions/`; a changed
     decision appended to instead of rewritten; a default in AGENTS.md or `docs/design.md`
     departed from without its line rewritten in the same PR; a contract in `docs/design.md`
     changed in the same PR as the code that depends on it; README describing planned work.
   - Taste: an annotation where inference already gives the type, a second class in one file,
     adapter concerns (sockets, devices, Node) leaking into the engine crate, `unsafe` outside
     `native/kernels` and `native/devices`, a workstream editing paths another owns in
     `docs/design.md` section 10. A departure with its default rewritten is not a finding.
3. In CI, post one inline comment per broken rule, on the line that breaks it, naming the rule
   in AGENTS.md's words and what satisfies it, then write the summary into your Claude comment
   on the PR: one line per rule in step 2, its verdict, and for a broken rule a pointer to its
   inline comment. A clean PR gets the summary and no inline comments. A local run has neither
   tool: give the findings and the summary in the session.

Done when every rule in step 2 has its line in the summary and every inline comment names its
rule.
