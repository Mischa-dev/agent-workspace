#!/usr/bin/env bash
# Decide which CI jobs a pull request needs, from the files it changes.
#
#   tools/ci-changed-areas.sh <base-sha> <head-sha>
#
# Prints `rust=true|false` and `web=true|false` for $GITHUB_OUTPUT. Anything
# it can't classify, and any failure to diff, runs both jobs: a skipped job
# counts as passing, so this must only ever skip work it is sure about.
set -uo pipefail

rust=false
web=false

if ! files="$(git diff --name-only "${1:?base sha}...${2:?head sha}")" || [ -z "${files}" ]; then
  echo "rust=true"
  echo "web=true"
  exit 0
fi

while IFS= read -r file; do
  case "${file}" in
    apps/web/* | scripts/*)
      web=true
      ;;
    apps/server/* | tools/* | integrations/* | deploy/* | docs/* | \
      Cargo.toml | Cargo.lock | rust-toolchain.toml | deny.toml)
      rust=true
      ;;
    *)
      rust=true
      web=true
      ;;
  esac
done <<< "${files}"

echo "rust=${rust}"
echo "web=${web}"
