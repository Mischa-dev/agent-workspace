#!/usr/bin/env bash
# GitHub CI: the Rust toolchain rust-toolchain.toml pins, and no other.
#
# The runner image ships its own stable Rust, and Swatinem/rust-cache folds
# every installed toolchain's version into its cache key. Each runner image
# update (a new stable) changed the key, so pull requests found no cache and
# built everything from scratch. Removing the image's toolchains keeps the
# key on the pinned version alone.
set -euo pipefail

cd "$(dirname "$0")/../.."
pinned="$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)"
[ -n "${pinned}" ] || { echo "rust-toolchain: no channel in rust-toolchain.toml" >&2; exit 1; }
rustup show active-toolchain || rustup toolchain install
rustup toolchain list | awk '{print $1}' | while read -r toolchain; do
  case "${toolchain}" in
    "${pinned}" | "${pinned}"-*) ;;
    *) rustup toolchain uninstall "${toolchain}" ;;
  esac
done
rustup toolchain list
