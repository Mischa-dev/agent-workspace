#!/bin/sh
# Writes the third-party notices for the server image: every Rust crate
# and web package built into it, with its version and licence. Run from
# the repository root after a build.
set -eu
echo "Third-party software in this image"
echo "=================================="
echo
echo "Rust crates (licence as each crate declares it):"
echo
cargo metadata --locked --format-version 1 \
  | tr '{' '\n' \
  | sed -n 's/.*"name":"\([^"]*\)","version":"\([^"]*\)".*"license":\("[^"]*"\|null\).*/\1 \2 \3/p' \
  | sort -u
echo
echo "The web app's packages, with theirs, are in THIRD-PARTY-NOTICES-WEB"
echo "beside this file (tools/licenses/web-licenses.mjs --notices)."
