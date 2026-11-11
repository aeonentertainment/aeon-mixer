# Contributing

Aeon Mixer is built by a small team and by coding agents working from [AGENTS.md](AGENTS.md).
The rules there apply to people too.

## Setup

```bash
npm i -g vite-plus
vp install
vp check && vp test
```

## Before you open a PR

- **One problem per PR.** Count underlying problems, not files. An adjacent cleanup is its own PR.
- **Establish the problem first.** Bugs go in an issue. Features and behaviour changes need a
  maintainer's agreement on direction before the code, so the work is not wasted.
- **Show evidence.** The PR body says what you ran and what you observed. A latency claim carries
  its measurement. "Tests pass" alone is not evidence.
- **Keep the latency budget.** See AGENTS.md. A change that adds buffering without a measurement
  will be sent back.

Titles are conventional commits in plain language. If an agent did the work, say which model and
harness at the end of the body.

## Releasing

Set `version` in `package.json` in a pull request. Once it merges, push the tag `v<version>` on
that commit. The Release workflow builds the addon on every platform, installs the tarball into
an empty project on each OS, and attaches it to a GitHub release. Nothing is built or published
from a laptop.

## Security

Do not open a public issue for a vulnerability. See [SECURITY.md](SECURITY.md).
