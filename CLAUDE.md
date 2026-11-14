@AGENTS.md

# Claude Code specifics

- `/pr-review <number>` reviews a PR against AGENTS.md, `/bug-review <number>` hunts it for
  bugs and `/shape-review <number>` judges whether it is built the simplest way. CI runs all
  three on every PR.
- Shared permissions and hooks live in `.claude/settings.json`. Personal overrides go in
  `.claude/settings.local.json`, which is gitignored.
