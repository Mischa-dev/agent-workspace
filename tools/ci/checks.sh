#!/usr/bin/env bash
# Run the CI checks in this checkout, on the machine tools/ci/run.sh picked.
#
#   tools/ci/checks.sh [rust,web|rust|web]
#
# Quick checks run first so a typo fails in seconds; the web checks then run
# alongside clippy and the Rust tests, or after them on a small or
# memory-capped machine. Expects from run.sh: CI_RUN_ID,
# CI_DATABASE_BASE (a PostgreSQL URL without a database) and optionally
# CI_TARGET_DIR, CI_LOCK_DIR, CI_MEMORY_MAX, CI_CPU_WEIGHT and CI_BUILD_LOCK.
#
# With CI_BUILD_LOCK set (the laptop route), every step holds that build
# admission lock while it runs (tools/ci/admission.sh), each on its own
# descriptor, so agents' builds and CI take turns step by step. A step that
# can't get a sound lock fails rather than running unlocked.
set -euo pipefail

areas="${1-rust,web}"
case "${areas}" in
  rust,web|rust|web) ;;
  *) echo "ci: invalid areas '${areas}' (expected rust,web, rust or web)" >&2; exit 2 ;;
esac
has() { [[ ",${areas}," == *",$1,"* ]]; }

# Run at low priority, capped, so the machine's owner always comes first.
if [ -z "${CI_SCOPED:-}" ]; then
  export CI_SCOPED=1
  export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
  if [ -n "${CI_LOCK_DIR:-}" ]; then
    echo "$$" > "${CI_LOCK_DIR}/pgid"
  fi
  if [ -n "${CI_MEMORY_MAX:-}" ] &&
    systemd-run --user --scope --quiet -p MemoryMax=1G true 2>/dev/null; then
    exec systemd-run --user --scope --quiet \
      -p MemoryMax="${CI_MEMORY_MAX}" -p MemorySwapMax=0 \
      -p CPUWeight="${CI_CPU_WEIGHT:-100}" -p IOWeight="${CI_CPU_WEIGHT:-100}" \
      nice -n 10 "$0" "$@"
  fi
  # On the laptop route (CI_BUILD_LOCK set) the memory cap is required: with
  # none asked for, or no way to make the capped scope, nothing runs. Elsewhere
  # a machine without user scopes still runs the checks at low priority.
  if [ -n "${CI_BUILD_LOCK:-}" ]; then
    echo "ci: not running checks: the laptop route needs a capped scope" \
      "(CI_MEMORY_MAX='${CI_MEMORY_MAX:-}'), and systemd-run --user --scope couldn't make one" >&2
    exit 125
  fi
  exec nice -n 10 "$0" "$@"
fi

export PATH="${HOME}/.cargo/bin:${PATH}"
export CARGO_TARGET_DIR="${CI_TARGET_DIR:-${PWD}/target}"
export CARGO_TERM_COLOR=always
# Incremental caches only help repeat local edits; on CI hosts they grew to
# 12 GB and filled the Red Hat disk.
export CARGO_INCREMENTAL=0
# Line tables keep file:line in backtraces and drop the rest of the debug info,
# which costs the server's test build memory and most of the build cache.
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
# Keep the build cache from filling a shared disk: start fresh past 16 GB.
if [ -d "${CARGO_TARGET_DIR}" ] &&
  [ "$(du -s --block-size=1G "${CARGO_TARGET_DIR}" | cut -f1)" -gt 16 ]; then
  echo "Build cache is over 16 GB; clearing it."
  rm -rf "${CARGO_TARGET_DIR}"
fi
export AGENT_WORKSPACE_ENV=test
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
started=$(date +%s)

# The repo's scripts call `pnpm` themselves; corepack provides the pinned one.
if ! type -P pnpm >/dev/null; then
  mkdir -p "${HOME}/.local/bin"
  corepack enable --install-directory "${HOME}/.local/bin" pnpm
  export PATH="${HOME}/.local/bin:${PATH}"
fi

# `admitted <name> <command...>`: run one step under the build admission lock
# when there is one. The subshell drops any inherited marker and opens its own
# descriptor, so steps running side by side wait for each other, and it closes
# the descriptor when the step ends.
#
# Every step, once it holds the lock, checks the laptop's CPU and memory
# (admission_resources_ok, with CI_MIN_FREE_GB from run.sh) before its command
# runs. run.sh's probe no longer does: an agent's build would count against it
# and send CI away instead of letting it wait its turn. What a "no" means
# depends on when it comes:
#   - at the first step, before anything has run here: checks.sh exits 75 and
#     run.sh moves the job to Red Hat, saying so;
#   - at a later step: the job stops with 76 and says why. It isn't moved, and
#     the steps that already ran aren't run again anywhere.
# 75 means only that first refusal. Any other 75 (a check that happens to
# return it) is reported as 1 (ci_exit_status, in finish below).
. "$(dirname "$0")/admission.sh"
admission_verdict_file="$(mktemp)"
ci_work_started=""
admission_refused_before_work=""
admitted() {
  local name="$1"
  shift
  if [ -z "${CI_BUILD_LOCK:-}" ]; then
    "$@"
    return
  fi
  (
    unset BUILD_ADMISSION_HELD BUILD_ADMISSION_FD
    admission_acquire "${CI_BUILD_LOCK}" "${name}" || exit
    if [ -n "${CI_MIN_FREE_GB:-}" ] && ! why="$(admission_resources_ok "${CI_MIN_FREE_GB}")"; then
      echo "${why:-resource check failed}" > "${admission_verdict_file}"
      exit 1
    fi
    "$@"
  )
}

# `step <name> <command...>`: a folded, timed section in the GitHub log.
step() {
  local name="$1" t0=$(date +%s) status=0
  shift
  echo "::group::${name}"
  : > "${admission_verdict_file}"
  admitted "${name}" "$@" || status=$?
  echo "::endgroup::"
  if [ -s "${admission_verdict_file}" ]; then
    if [ -z "${ci_work_started}" ]; then
      echo "ci: the laptop was too busy to start CI once it had the build lock: $(cat "${admission_verdict_file}")" >&2
      admission_refused_before_work=1
      exit 75
    fi
    echo "✗ ${name} not run: the laptop got too busy during CI: $(cat "${admission_verdict_file}")." \
      "Stopping here; the steps already done aren't run again elsewhere." >&2
    exit 76
  fi
  ci_work_started=1
  # From here on the load average counts this run's own work (as the probe's
  # CI_RECENT_RUN did for a run that had just finished).
  export CI_RECENT_RUN=1
  if [ "${status}" -ne 0 ]; then
    echo "✗ ${name} failed after $(($(date +%s) - t0))s"
    return "${status}"
  fi
  echo "✓ ${name} ($(($(date +%s) - t0))s)"
}

# ci_exit_status <status>: what checks.sh exits with. 75 is kept only for the
# refusal before any work; a check that returned 75 is an ordinary failure.
ci_exit_status() {
  if [ "$1" -eq 75 ] && [ -z "${admission_refused_before_work}" ]; then
    echo "ci: a check exited 75; reporting it as an ordinary failure (1)" >&2
    echo 1
  else
    echo "$1"
  fi
}

# The web checks run in the background next to the Rust ones.
web_pid=""
web_log="$(mktemp)"
web_status_file="$(mktemp)"
database=""

finish() {
  local status=$?
  if [ -n "${web_pid}" ] && kill -0 "${web_pid}" 2>/dev/null; then
    kill "${web_pid}" 2>/dev/null || true
  fi
  if [ -n "${database}" ]; then
    dropdb --force --if-exists --maintenance-db="${CI_DATABASE_BASE}/postgres" "${database}" 2>/dev/null || true
  fi
  rm -f "${web_log}" "${web_status_file}" "${admission_verdict_file}"
  echo "CI checks took $(($(date +%s) - started))s on $(hostname)"
  exit "$(ci_exit_status "${status}")"
}
trap finish EXIT

# Stop early if the web checks already failed.
web_failed() {
  [ -n "${web_pid}" ] && ! kill -0 "${web_pid}" 2>/dev/null &&
    [ "$(cat "${web_status_file}")" != "0" ]
}
report_web() {
  [ -z "${web_pid}" ] && return 0
  wait "${web_pid}" || true
  web_pid=""
  cat "${web_log}"
  [ "$(cat "${web_status_file}")" = "0" ]
}

# 1. Quick checks: formatting, lint and types, fixtures.
if has rust; then
  step "CI script tests" python3 -m unittest discover -s tools/ci -p 'test_*.py'
  step "admission lock tests" tools/ci/test_admission.sh
  rustup toolchain install >/dev/null 2>&1 || true
  step "cargo fmt" cargo fmt --check
  step "self-hosted guide" ./tools/verify-self-hosted-guide.sh
  step "hermes fixtures" python3 tools/hermes-connector/verify-fixtures.py
  step "agentos-nyx tests" python3 -m unittest discover -s integrations/agentos-nyx -p 'test_*.py'
fi
if has web; then
  step "pnpm install" pnpm install --frozen-lockfile --reporter=silent
  step "web lint" pnpm lint
  step "web typecheck" pnpm typecheck
fi

# 2. Web tests and build: in the background next to the Rust checks on a big
# machine; on a small one they'd starve each other, and under a memory cap the
# server's test build alone comes close to it, so there they wait their turn.
run_web_checks() {
  step "web tests" pnpm test &&
    step "web build" pnpm build &&
    step "web asset budgets" pnpm check:assets &&
    step "performance tools" pnpm check:performance-tools &&
    step "web browser smoke" run_browser_smoke
}
# Playwright smoke tests (apps/web/e2e): sign in and the task board in a real
# browser, against the server built from this tree and a throwaway database.
# Where PostgreSQL or a Chromium for Playwright is missing they're skipped with
# a note, except in a CI run (CI_RUN_ID set), where a skip would read as a pass.
skip_browser_smoke() {
  if [ -n "${CI_RUN_ID:-}" ]; then
    echo "Browser smoke tests can't run: $*" >&2
    return 1
  fi
  echo "Skipping browser smoke tests: $*"
}
run_browser_smoke() {
  local binary="${CARGO_TARGET_DIR}/debug/agent-workspace-server"
  if [ -z "${CI_DATABASE_BASE:-}" ] ||
    ! psql -d "${CI_DATABASE_BASE}/postgres" -Atqc 'SELECT 1' >/dev/null 2>&1; then
    skip_browser_smoke "PostgreSQL isn't reachable at CI_DATABASE_BASE."
    return
  fi
  if [ -z "${PLAYWRIGHT_CHROMIUM_EXECUTABLE:-}" ] && ! (cd apps/web && node -e \
    "process.exit(require('fs').existsSync(require('@playwright/test').chromium.executablePath()) ? 0 : 1)"); then
    skip_browser_smoke "no Chromium for Playwright. Run" \
      "'pnpm --filter @agent-workspace/web exec playwright install chromium'" \
      "or set PLAYWRIGHT_CHROMIUM_EXECUTABLE."
    return
  fi
  # Build the server from this tree (incremental; waits for any cargo run in progress).
  if type -P cargo >/dev/null; then
    cargo build --quiet -p agent-workspace-server --bin agent-workspace-server || return
  fi
  if [ ! -x "${binary}" ]; then
    skip_browser_smoke "no server binary at ${binary}."
    return
  fi
  # A cancelled run can leave its server or Vite listening on the smoke ports
  # (apps/web/e2e/environment.ts). This run holds the machine's lock, so
  # anything this user still has there is left over: stop it.
  if [ -n "${CI_LOCK_DIR:-}" ]; then
    local server_port="${E2E_SERVER_PORT:-3190}"
    python3 tools/ci/free_ports.py "${server_port}" "$((server_port + 1))" "${E2E_WEB_PORT:-5190}"
  fi
  E2E_SERVER_BINARY="${binary}" E2E_DATABASE_BASE="${CI_DATABASE_BASE}" CI=1 \
    E2E_DATABASE_NAME="aw_e2e_$(printf '%s' "${CI_RUN_ID:-$$}" | tr -c 'a-zA-Z0-9' '_' | tr 'A-Z' 'a-z')" \
    pnpm test:e2e
}
# Under an admission lock nothing heavy runs side by side: the web checks wait.
if has web && has rust && { [ "$(nproc)" -lt 8 ] || [ -n "${CI_MEMORY_MAX:-}" ] || [ -n "${CI_BUILD_LOCK:-}" ]; }; then
  web_later=1
elif has web; then
  (
    status=0
    run_web_checks > "${web_log}" 2>&1 || status=$?
    echo "${status}" > "${web_status_file}"
  ) &
  web_pid=$!
fi

# 3. Clippy, then the Rust tests against a throwaway database.
if has rust; then
  step "cargo clippy" cargo clippy --workspace --all-targets --all-features -- -D warnings
  if web_failed; then report_web; fi

  if ! cargo nextest --version >/dev/null 2>&1; then
    step "install cargo-nextest" cargo install cargo-nextest --version 0.9.136 --locked
  fi
  # Databases left by a run that was killed before it could clean up.
  psql -d "${CI_DATABASE_BASE}/postgres" -Atc \
    "SELECT datname FROM pg_database WHERE datname ~ '^aw_ci_run_'" 2>/dev/null |
    while read -r stale; do
      dropdb --force --if-exists --maintenance-db="${CI_DATABASE_BASE}/postgres" "${stale}" || true
    done
  database="aw_ci_run_$(printf '%s' "${CI_RUN_ID:-$$}" | tr -c 'a-zA-Z0-9' '_')"
  createdb --maintenance-db="${CI_DATABASE_BASE}/postgres" "${database}"
  export AGENT_WORKSPACE_DATABASE_URL="${CI_DATABASE_BASE}/${database}"

  # Compiling the server library and its unit tests takes about 5 GB each, so
  # side by side they overrun an 8 GB cap. Under a cap, build the library and
  # binaries first; the test build then reuses the library.
  if [ -n "${CI_MEMORY_MAX:-}" ]; then
    step "rust build" cargo build --workspace
  fi
  step "rust tests" cargo nextest run --workspace --profile ci
  step "doc tests" cargo test --workspace --doc --quiet
  if web_failed; then report_web; fi
  step "install cargo-deny" cargo install cargo-deny --version 0.20.2 --locked --quiet
  step "cargo deny" cargo deny check
fi

report_web
if [ -n "${web_later:-}" ]; then run_web_checks; fi
# The browser smoke tests drive the server as much as the web app, so a change
# on the Rust side alone runs them too (they need the web dependencies and
# Vite's dev server, not the web build).
if has rust && ! has web; then
  step "pnpm install" pnpm install --frozen-lockfile --reporter=silent
  step "web browser smoke" run_browser_smoke
fi
echo "All checks passed (${areas})."
