#!/usr/bin/env bash
# Decide which checks a pull request needs, from the files it changes.
#
#   tools/ci/changed-areas.sh <base-sha> <head-sha>
#
# Prints `areas=rust`, `areas=web` or `areas=rust,web` for $GITHUB_OUTPUT.
# Anything it can't classify, and any failure to diff, runs everything.
set -uo pipefail

rust=false
web=false

if ! files="$(git diff --name-only "${1:?base sha}...${2:?head sha}")" || [ -z "${files}" ]; then
  echo "areas=rust,web"
  exit 0
fi

while IFS= read -r file; do
  case "${file}" in
    # Web files the Rust tests read (include_str! and parity fixtures): a
    # change to one can break a Rust test, so it runs both sides. And the
    # files generated from the server's operations: a hand edit must fail
    # the server's drift check, and a regenerated file must pass the web's.
    apps/web/src/experience/registry.json | \
      apps/web/src/ui/markdown/*.json | \
      apps/web/src/memory/live/*.json | \
      apps/web/src/settings/workspace/roles.ts | \
      apps/web/src/api/generated/*)
      rust=true
      web=true
      ;;
    apps/web/* | scripts/*)
      web=true
      ;;
    apps/server/* | tools/* | integrations/* | deploy/* | docs/* | \
      Cargo.toml | Cargo.lock | rust-toolchain.toml | deny.toml | .config/nextest.toml)
      rust=true
      ;;
    *)
      rust=true
      web=true
      ;;
  esac
done <<< "${files}"

case "${rust},${web}" in
  true,false) echo "areas=rust" ;;
  false,true) echo "areas=web" ;;
  *) echo "areas=rust,web" ;;
esac
