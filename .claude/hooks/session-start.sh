#!/usr/bin/env bash
# SessionStart hook: get a Claude Code cloud session ready to build and test.
# Does nothing outside the cloud. See tools/cloud/setup.sh.
set -euo pipefail

root="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "$0")/../.." && pwd)}"
# Commits are checked for formatting before they are made, on any machine.
(cd "${root}" && tools/git-hooks/turn-on) >&2 || true

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

# The cargo build warms in the background so the session starts sooner; the
# first cargo command waits for it on cargo's own lock.
exec "${root}/tools/cloud/setup.sh" --warm background >&2
