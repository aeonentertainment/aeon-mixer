@AGENTS.md

# Claude Code specifics

- `/pr-review <number>` reviews a PR against AGENTS.md. CI runs the same skill on every PR.
- Shared permissions and hooks live in `.claude/settings.json`. Personal overrides go in
  `.claude/settings.local.json`, which is gitignored.
