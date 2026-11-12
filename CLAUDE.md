@AGENTS.md

# Claude Code specifics

- `/pr-review <number>` reviews a PR against AGENTS.md and `/bug-review <number>` hunts it for
  bugs. CI runs both skills on every PR.
- Shared permissions and hooks live in `.claude/settings.json`. Personal overrides go in
  `.claude/settings.local.json`, which is gitignored.
