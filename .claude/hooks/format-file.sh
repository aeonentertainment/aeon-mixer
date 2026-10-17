#!/bin/bash
# PostToolUse hook: format the file Claude just edited. Never blocks; a
# formatter failure shows up in vp check, not here.
file=$(jq -r '.tool_input.file_path // empty')
[ -z "$file" ] && exit 0
case "$file" in
  *.ts|*.tsx|*.js|*.mjs|*.json|*.jsonc|*.md|*.yml|*.yaml)
    pnpm exec vp fmt --no-error-on-unmatched-pattern "$file" >/dev/null 2>&1 || true
    ;;
esac
exit 0
