#!/usr/bin/env bash
set -euo pipefail

readonly guide="docs/self-hosted-red-hat.md"

test -f "${guide}"
bash -n deploy/redhat/*.sh

for required in \
  'Red Hat Enterprise Linux 10.1 ARM64' \
  '127.0.0.1:3100' \
  '127.0.0.1:3101' \
  '127.0.0.1:55432' \
  'agent-workspace' \
  'podman-compose' \
  'git bundle' \
  'deploy-release.sh' \
  'health/ready' \
  'retention-worktrees' \
  'migration' \
  'Rollback boundary'
do
  rg --fixed-strings --quiet "${required}" "${guide}"
done

if rg --quiet 'curl[^\n]*\|[[:space:]]*(sh|bash)' "${guide}"; then
  echo "The self-hosted guide must not pipe network responses into a shell." >&2
  exit 1
fi

if rg --quiet '(password|secret|token)[[:space:]]*=[[:space:]]*[^[:space:]`]*(example|test|real)[^[:space:]`]*' "${guide}"; then
  echo "The self-hosted guide contains a credential-like literal." >&2
  exit 1
fi
