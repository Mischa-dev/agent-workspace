#!/usr/bin/env bash
# Builds the trailer and renders stills of moments into stills/<name>/: ./shots.sh name 1.5 3 ...
set -euo pipefail
cd "$(dirname "$0")"
name="$1"; shift
../node_modules/.bin/vite build --logLevel error
node stills.mjs "stills/${name}" "$@"
