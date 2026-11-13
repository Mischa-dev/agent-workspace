#!/usr/bin/env bash
# The UI tour (AWD-84): about 15 key screens at 1440 and 390 px, in light and
# dark, against a throwaway server seeded with a busy workspace.
#
#   tools/ui-tour/run.sh [--only <screen,...>] [--direction workroom|ledger|paper]
#
# From a clean checkout it installs the web dependencies, builds the server,
# makes a throwaway database, seeds it and takes the screenshots. The PNGs and
# a contact sheet (index.html) land in apps/web/e2e-output/tour/.
#
# Needs a PostgreSQL you can create databases on (E2E_DATABASE_BASE, a URL
# without a database name; on Mischa's laptop the CI one is used by default)
# and a Chromium (PLAYWRIGHT_CHROMIUM_EXECUTABLE, or Playwright's own).
# Where cargo, node or python3 are missing and Nix is there (NixOS), it runs
# itself again inside a Nix shell that has them. On Mischa's laptop it runs as
# one build-run job, or not at all. E2E_SERVER_BINARY names a server built
# already, which skips the build.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${root}"

only=""
direction=""
while [ $# -gt 0 ]; do
  case "$1" in
    --only) only="$2"; shift 2 ;;
    --direction) direction="$2"; shift 2 ;;
    *) echo "usage: tools/ui-tour/run.sh [--only <screen,...>] [--direction <name>]" >&2; exit 2 ;;
  esac
done

# On Mischa's laptop heavy work goes through build-run (AGENTS.md): one job at
# a time in the agents' capped slice, taking turns with laptop CI on the build
# admission lock. The whole tour (build, seed, screenshots) runs as one such
# job, and it doesn't run at all when that protection isn't there.
build_run="${HOME}/.local/bin/build-run"
guarded=""
case "$(cut -d: -f3- /proc/self/cgroup 2>/dev/null)" in
  */app-t3code-agents.slice/app-t3code-agents-buildrun.slice/*) guarded=1 ;;
esac
if [ -z "${guarded}" ] && { [ -e /run/build-admission.lock ] || [ -e "${build_run}" ]; }; then
  if [ ! -x "${build_run}" ]; then
    echo "tour: this machine builds through ${build_run}, which isn't there; not running unguarded" >&2
    exit 125
  fi
  # build-run refuses (125) itself when its cap, queue or lock isn't set up.
  exec "${build_run}" "${root}/tools/ui-tour/run.sh" ${only:+--only "${only}"}
fi

if { ! type -P cargo >/dev/null || ! type -P node >/dev/null || ! type -P python3 >/dev/null; } &&
  [ -z "${UI_TOUR_IN_NIX:-}" ] && type -P nix >/dev/null; then
  echo "tour: cargo, node or python3 missing; running inside a Nix shell"
  UI_TOUR_IN_NIX=1 exec nix shell nixpkgs#rustup nixpkgs#nodejs_24 nixpkgs#python3 nixpkgs#gcc \
    nixpkgs#cmake nixpkgs#pkg-config nixpkgs#perl -c "$0" ${only:+--only "${only}"} ${direction:+--direction "${direction}"}
fi
for tool in cargo node python3; do
  type -P "${tool}" >/dev/null || { echo "tour: ${tool} is missing" >&2; exit 1; }
done

if [ -z "${E2E_DATABASE_BASE:-}" ]; then
  if psql postgres://aw_test@127.0.0.1:55433/postgres -tAc 'select 1' >/dev/null 2>&1; then
    export E2E_DATABASE_BASE=postgres://aw_test@127.0.0.1:55433
  else
    echo "tour: set E2E_DATABASE_BASE to a PostgreSQL URL without a database, e.g. postgres://user:pass@127.0.0.1:5432" >&2
    exit 1
  fi
fi
if [ -z "${PLAYWRIGHT_CHROMIUM_EXECUTABLE:-}" ]; then
  for candidate in /run/current-system/sw/bin/google-chrome-stable /opt/pw-browsers/chromium; do
    if [ -x "${candidate}" ]; then export PLAYWRIGHT_CHROMIUM_EXECUTABLE="${candidate}"; break; fi
  done
fi

export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
if type -P pnpm >/dev/null; then pnpm=(pnpm); else pnpm=(corepack pnpm); fi
echo "tour: installing web dependencies"
"${pnpm[@]}" install --frozen-lockfile --reporter=silent

if [ -n "${E2E_SERVER_BINARY:-}" ]; then
  # A server built already (from this tree) is used as it is.
  [ -x "${E2E_SERVER_BINARY}" ] || { echo "tour: E2E_SERVER_BINARY (${E2E_SERVER_BINARY}) isn't a program" >&2; exit 1; }
  echo "tour: using the server at ${E2E_SERVER_BINARY}"
else
  echo "tour: building the server"
  build=(cargo build --quiet -j "${CARGO_BUILD_JOBS:-4}" -p agent-workspace-server --bin agent-workspace-server)
  if [ -n "${guarded}" ]; then
    # Inside build-run's job: its cap and lock cover the build, with one
    # Cargo job unless CARGO_BUILD_JOBS says otherwise. A scope of its own
    # would escape that cap.
    "${build[@]}"
  elif systemd-run --user --scope --quiet -p MemoryMax=1G true 2>/dev/null; then
    # Elsewhere: capped, so the build can't push other services into the OOM killer.
    systemd-run --user --scope --quiet -p MemoryMax="${UI_TOUR_BUILD_MEMORY:-7G}" -p MemorySwapMax=0 "${build[@]}"
  else
    "${build[@]}"
  fi
fi

echo "tour: seeding and taking screenshots"
cd apps/web
UI_TOUR_ONLY="${only}" UI_TOUR_DIRECTION="${direction}" \
  E2E_DATABASE_NAME="${E2E_DATABASE_NAME:-aw_e2e_tour}" \
  E2E_SERVER_PORT="${E2E_SERVER_PORT:-3290}" E2E_WEB_PORT="${E2E_WEB_PORT:-5290}" \
  node ./node_modules/@playwright/test/cli.js test --config playwright.tour.config.ts
echo "tour: done. Open apps/web/e2e-output/tour/index.html"
