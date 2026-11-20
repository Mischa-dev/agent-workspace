#!/usr/bin/env bash
# GitHub CI: extra swap on the runner's scratch disk, so the server library
# and its unit tests (about 5 GB each) can compile side by side on a 16 GB
# runner instead of one after the other. Without swap, building them together
# got the runner shut down mid-build. Where swap can't be added, checks.sh is
# told to build them in order instead (CI_BUILD_IN_ORDER).
set -uo pipefail

size="${1:-12G}"
swap=/mnt/ci-swap
if sudo fallocate -l "${size}" "${swap}" && sudo chmod 600 "${swap}" &&
  sudo mkswap "${swap}" > /dev/null && sudo swapon "${swap}"; then
  swapon --show
  free -h
else
  echo "swap: couldn't add ${size} at ${swap}; the test build will follow the library build" >&2
  echo "CI_BUILD_IN_ORDER=1" >> "${GITHUB_ENV:-/dev/null}"
fi
