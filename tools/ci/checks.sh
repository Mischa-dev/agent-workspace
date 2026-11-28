#!/usr/bin/env bash
# Run the CI checks in this checkout, on the machine tools/ci/run.sh picked.
#
#   tools/ci/checks.sh [rust,web|rust|web]
#
# Quick checks run first so a typo fails in seconds; the web checks then run
# alongside clippy and the Rust tests, or after them on a small or
# memory-capped machine.
#
# GitHub runs the checks as parallel jobs (.github/workflows/ci.yml), each
# asking for one part with CI_PART; unset, every part runs here in turn:
#   lint        the quick checks, cargo fmt, clippy and cargo deny
#   rust-build  the Rust tests built once into a nextest archive (with the
#               server binary the browser tests use), doc tests, schema snapshot
#   rust-tests  one slice of that archive's tests (CI_SHARD=k/n)
#   web         web licences, lint, types, tests, build and budgets
#   e2e         one shard of the browser smoke tests (CI_SHARD=k/n), against
#               a server binary already built
# Expects from run.sh: CI_RUN_ID,
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
part="${CI_PART:-all}"
case "${part}" in
  all|lint|rust-build|rust-tests|web|e2e) ;;
  *) echo "ci: invalid CI_PART '${part}' (expected lint, rust-build, rust-tests, web or e2e)" >&2; exit 2 ;;
esac
# `runs <part>...`: whether this run includes any of those parts.
runs() { [ "${part}" = all ] || [[ " $* " == *" ${part} "* ]]; }
shard="${CI_SHARD:-}"
if [ -n "${shard}" ] && ! [[ "${shard}" =~ ^[1-9][0-9]*/[1-9][0-9]*$ ]]; then
  echo "ci: invalid CI_SHARD '${shard}' (expected k/n)" >&2
  exit 2
fi
# Where the rust-build part leaves the tests for the rust-tests part.
nextest_archive="${CI_NEXTEST_ARCHIVE:-}"
# CI_UI_TOUR=channel (run.sh --host redhat --ui-tour channel, never the
# default): after the browser smoke tests, the UI tour's seed and its channel
# screens, in the same step (run_ui_tour).
case "${CI_UI_TOUR:-}" in
  '' | channel) ;;
  *) echo "ci: invalid CI_UI_TOUR '${CI_UI_TOUR}' (expected channel)" >&2; exit 2 ;;
esac

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
  # So is the opt-in UI tour's, which never runs uncapped.
  if [ -n "${CI_UI_TOUR:-}" ]; then
    echo "ci: not running checks: the UI tour needs a capped scope" \
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

# The Node pinned in .node-version, for CI: the host's own Node when it is
# exactly that version, otherwise the official build kept in the CI user's
# cache (no root, nothing the app's service uses), so CI follows the pin
# instead of waiting for each host's Node to be upgraded.
#
# pinned_node_dir: sets pin_version and pin_dir (where the official build is
# kept) for the caller.
pinned_node_dir() {
  local arch
  pin_version=$(tr -d '[:space:]' < .node-version)
  case "$(uname -m)" in
    x86_64) arch=x64 ;;
    aarch64 | arm64) arch=arm64 ;;
    *) echo "No official Node build for $(uname -m)." >&2; return 1 ;;
  esac
  pin_dir="${CI_NODE_CACHE:-${HOME}/.cache/aw-ci/node}/node-v${pin_version}-linux-${arch}"
}

# is_pinned_node <node>: whether that node binary reports exactly the pin.
is_pinned_node() {
  [ "$("$1" --version 2> /dev/null || true)" = "v${pin_version}" ]
}

# fetch_pinned_node: makes sure the pinned Node is there, downloading it
# (checked against nodejs.org's SHA-256 list) only when neither the host's
# Node nor a kept copy is exactly that version. Fails unless a copy reporting
# exactly that version ends up kept.
fetch_pinned_node() {
  [ -f .node-version ] || return 0
  local pin_version pin_dir name cache tmp
  pinned_node_dir || return 1
  if is_pinned_node node || is_pinned_node "${pin_dir}/bin/node"; then
    return 0
  fi
  name=$(basename "${pin_dir}")
  cache=$(dirname "${pin_dir}")
  echo "Fetching ${name} (host has $(node --version 2> /dev/null || echo 'no Node'))."
  mkdir -p "${cache}"
  tmp=$(mktemp -d "${cache}/.fetch.XXXXXX")
  if ! (
    cd "${tmp}" &&
      curl -fsSL --retry 3 -o SHASUMS256.txt "https://nodejs.org/dist/v${pin_version}/SHASUMS256.txt" &&
      curl -fsSL --retry 3 -o "${name}.tar.xz" "https://nodejs.org/dist/v${pin_version}/${name}.tar.xz" &&
      grep -E "^[0-9a-f]{64}  ${name}\.tar\.xz\$" SHASUMS256.txt | sha256sum -c --strict - &&
      tar -xJf "${name}.tar.xz" &&
      is_pinned_node "./${name}/bin/node"
  ); then
    rm -rf "${tmp}"
    return 1
  fi
  # A broken copy left there is replaced; another run may have kept a good
  # one meanwhile, and either will do. Either way one must be there now.
  if [ -e "${pin_dir}" ] && ! is_pinned_node "${pin_dir}/bin/node"; then
    rm -rf "${pin_dir}"
  fi
  mv -T "${tmp}/${name}" "${pin_dir}" 2> /dev/null || true
  rm -rf "${tmp}"
  if ! is_pinned_node "${pin_dir}/bin/node"; then
    echo "Couldn't keep ${name} in ${cache}." >&2
    return 1
  fi
}

# activate_pinned_node: puts the pinned Node first on PATH in this shell. It
# never downloads: it uses the host's Node when that is exactly the pin, or
# the copy fetch_pinned_node kept, and refuses when that copy is missing or
# isn't exactly the pinned version, so a host Node never stands in for it.
activate_pinned_node() {
  [ -f .node-version ] || return 0
  local pin_version pin_dir
  pinned_node_dir || return 1
  if is_pinned_node node; then
    return 0
  fi
  if ! is_pinned_node "${pin_dir}/bin/node"; then
    echo "Node v${pin_version} isn't kept in ${pin_dir}; the Node toolchain step must fetch it first." >&2
    return 1
  fi
  export PATH="${pin_dir}/bin:${PATH}"
  # This Node's own corepack provides the pinned pnpm.
  corepack enable --install-directory "${pin_dir}/bin" pnpm
  node --version
}

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

# `cargo fmt` formats what it reaches from each crate's root, which leaves
# out a module that generated code declares (`modules/core/*/mod.rs`). This
# formats every Rust file by its path, as the commit hook does.
every_rust_file_formatted() {
  local edition
  edition="$(sed -n 's/^edition = "\(.*\)"$/\1/p' Cargo.toml 2>/dev/null | head -n 1)"
  find . -name '*.rs' -not -path '*/target/*' -not -path '*/node_modules/*' -print0 |
    xargs -0 rustfmt --check --edition "${edition}"
}

# 1. Quick checks: formatting, lint and types, fixtures.
if has rust && runs lint; then
  rustup toolchain install >/dev/null 2>&1 || true
  step "CI script tests" python3 -m unittest discover -s tools/ci -p 'test_*.py'
  step "admission lock tests" tools/ci/test_admission.sh
  step "pinned Node tests" tools/ci/test_pinned_node.sh
  step "cargo fmt" cargo fmt --check
  step "rustfmt, every file" every_rust_file_formatted
  step "self-hosted guide" ./tools/verify-self-hosted-guide.sh
  step "hermes fixtures" python3 tools/hermes-connector/verify-fixtures.py
  step "agentos-nyx tests" python3 -m unittest discover -s integrations/agentos-nyx -p 'test_*.py'
  step "runner SDK tests (Python)" python3 -m unittest discover -s sdks/runner-python/tests -t sdks/runner-python
  step "Hermes plugin tests" python3 -m unittest discover -s sdks/adapters/hermes/tests -p 'test_*.py'
fi
# The pinned Node and the web dependencies, for the web checks and the browser
# smoke tests.
web_toolchain() {
  step "Node toolchain" fetch_pinned_node
  # In this shell, not a step: a step under the build admission lock runs in
  # a subshell, and the PATH has to hold for the steps after it. It never
  # downloads, and stops CI here if the kept copy is missing or wrong.
  if ! activate_pinned_node > /dev/null; then
    echo "✗ Node toolchain: the pinned Node isn't usable here." >&2
    exit 1
  fi
  step "Node version" node -e '
    const expected = require("node:fs").readFileSync(".node-version", "utf8").trim();
    if (process.version !== `v${expected}`) {
      throw new Error(`CI needs Node ${expected}; selected host has ${process.version}.`);
    }
    console.log(`Node ${process.version}`);
  '
  step "pnpm install" pnpm install --frozen-lockfile --reporter=silent
}
if has web && runs web; then
  web_toolchain
  step "web licences" node tools/licenses/web-licenses.mjs --check
  step "web lint" pnpm lint
  step "web typecheck" pnpm typecheck
  step "runner SDK tests (TypeScript)" node --test 'sdks/runner-typescript/test/*.test.ts'
  step "OpenClaw plugin tests" node --import ./sdks/adapters/openclaw/test/resolve.mjs --test 'sdks/adapters/openclaw/test/*.test.ts'
fi

# 2. Web tests and build: in the background next to the Rust checks on a big
# machine; on a small one they'd starve each other, and under a memory cap the
# server's test build alone comes close to it, so there they wait their turn.
run_web_checks() {
  step "web tests" pnpm test &&
    step "web build" pnpm build &&
    web_built=1 &&
    web_budget_base &&
    step "web asset budgets" pnpm check:assets &&
    step "performance tools" pnpm check:performance-tools &&
    { [ "${part}" != all ] || step "web browser smoke" run_browser_smoke; }
}
# The base commit's asset sizes, so the budget step can print what this change
# adds (pull requests, where CI sets BUDGET_BASE_SHA). Best effort: a base that
# won't build only drops the change column.
web_budget_base() {
  [ -n "${BUDGET_BASE_SHA:-}" ] || return 0
  local base_dir
  base_dir="$(mktemp -d)"
  if git worktree add --detach "${base_dir}/tree" "${BUDGET_BASE_SHA}" > /dev/null 2>&1 &&
    (cd "${base_dir}/tree" && pnpm install --frozen-lockfile --reporter=silent > /dev/null 2>&1 &&
      pnpm build > /dev/null 2>&1); then
    node scripts/check-web-budgets.mjs "${base_dir}/tree/apps/web/dist" > "${base_dir}/base.json" 2> /dev/null || true
    export WEB_BUDGET_BASE="${base_dir}/base.json"
  else
    echo "web asset budgets: the base commit didn't build; no change column" >&2
  fi
  git worktree remove --force "${base_dir}/tree" > /dev/null 2>&1 || true
  return 0
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
  # Build the server from this tree (incremental; waits for any cargo run in
  # progress). The e2e part uses the binary the rust-build job made instead.
  if [ "${part}" != e2e ] && type -P cargo >/dev/null; then
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
    python3 tools/ci/free_ports.py "${server_port}" "$((server_port + 1))" "${E2E_WEB_PORT:-5190}" "$((${E2E_WEB_PORT:-5190} + 1))"
  fi
  local database_name
  database_name="aw_e2e_$(printf '%s' "${CI_RUN_ID:-$$}" | tr -c 'a-zA-Z0-9' '_' | tr 'A-Z' 'a-z')"
  # The static-host spec serves this run's web build when there is one, and
  # builds its own otherwise (apps/web/e2e/static-host.spec.ts).
  # One shard of the specs (CI_SHARD=k/n) when GitHub splits them across jobs;
  # each shard seeds its own server and database in the global setup. The
  # tests are dealt out in turn (tools/ci/e2e_shard.py), not split by file or
  # into runs of neighbours, which left one shard with all the slow sweeps.
  if [ -n "${shard}" ]; then
    local listing test_list
    listing="$(mktemp)" test_list="$(mktemp)"
    PLAYWRIGHT_JSON_OUTPUT_NAME="${listing}" \
      pnpm --filter @agent-workspace/web exec playwright test --list --reporter=json || return
    python3 -I tools/ci/e2e_shard.py "${shard}" <"${listing}" >"${test_list}" || return
    head -n 1 "${test_list}"
    E2E_SERVER_BINARY="${binary}" E2E_DATABASE_BASE="${CI_DATABASE_BASE}" CI=1 \
      E2E_DATABASE_NAME="${database_name}" E2E_STATIC_DIST="${web_built:+dist}" \
      pnpm --filter @agent-workspace/web exec playwright test --test-list "${test_list}" || return
  else
    E2E_SERVER_BINARY="${binary}" E2E_DATABASE_BASE="${CI_DATABASE_BASE}" CI=1 \
      E2E_DATABASE_NAME="${database_name}" E2E_STATIC_DIST="${web_built:+dist}" \
      pnpm test:e2e || return
  fi
  [ "${CI_UI_TOUR:-}" = channel ] || return 0
  run_ui_tour "${binary}" "${database_name}"
}

# `run_ui_tour <server> <database name>`: the opt-in UI tour (CI_UI_TOUR=channel)
# after the smoke tests, in the same step: the tour's whole seed and its channel
# screens (apps/web/e2e/tour, UI_TOUR_ONLY=channel), against the server and
# throwaway database name the smoke tests just used. Not the tour's launcher,
# which would build the server again. check_ui_tour.py then fails the step
# unless the seed finished with a run and no warnings and the four channel
# screenshots and contact sheet are this run's, and writes a summary.
run_ui_tour() {
  local output=apps/web/e2e-output started
  rm -rf "${output}/tour" "${output}/state/tour.json"
  if [ -n "${CI_LOCK_DIR:-}" ]; then
    local server_port="${E2E_SERVER_PORT:-3190}"
    python3 tools/ci/free_ports.py "${server_port}" "$((server_port + 1))" "${E2E_WEB_PORT:-5190}" "$((${E2E_WEB_PORT:-5190} + 1))"
  fi
  started=$(date +%s)
  E2E_SERVER_BINARY="$1" E2E_DATABASE_BASE="${CI_DATABASE_BASE}" CI=1 \
    E2E_DATABASE_NAME="$2" UI_TOUR_ONLY=channel \
    pnpm --filter @agent-workspace/web exec playwright test --config playwright.tour.config.ts || return
  python3 tools/ci/check_ui_tour.py "${output}" "${started}"
}

# Under an admission lock nothing heavy runs side by side: the web checks wait.
# A part runs alone, in order.
if [ "${part}" != all ]; then
  if has web && runs web; then web_later=1; fi
elif has web && has rust && { [ "$(nproc)" -lt 8 ] || [ -n "${CI_MEMORY_MAX:-}" ] || [ -n "${CI_BUILD_LOCK:-}" ]; }; then
  web_later=1
elif has web; then
  (
    status=0
    run_web_checks > "${web_log}" 2>&1 || status=$?
    echo "${status}" > "${web_status_file}"
  ) &
  web_pid=$!
fi

# `install_tool <cargo-nextest|cargo-deny>`: the pinned version in
# ~/.cargo/bin. On GitHub's runners, from its release build (checked against
# the SHA-256 below): seconds, where building it took four minutes. Elsewhere
# (a NixOS laptop can't run a prebuilt glibc binary), built with cargo install.
install_tool() {
  local name="$1" version url sha256 bin tmp
  case "${name}" in
    cargo-nextest)
      version=0.9.136
      url="https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-${version}/cargo-nextest-${version}-x86_64-unknown-linux-gnu.tar.gz"
      sha256=a098eed56f2dd88c7fdca1e554a6b99fa1ffbd2a7a1c41b865700112981f6f52
      ;;
    cargo-deny)
      version=0.20.2
      url="https://github.com/EmbarkStudios/cargo-deny/releases/download/${version}/cargo-deny-${version}-x86_64-unknown-linux-musl.tar.gz"
      sha256=9f12ed4c49936e09b48bf862b595cde2fe64fcbd9d74dfacac6131ca824c8d5f
      ;;
    *) echo "install_tool: unknown tool ${name}" >&2; return 2 ;;
  esac
  bin="${CARGO_HOME:-${HOME}/.cargo}/bin"
  if "${bin}/${name}" "${name#cargo-}" --version 2> /dev/null | grep -qF " ${version}"; then
    return 0
  fi
  if [ "${GITHUB_ACTIONS:-}" = true ] && [ "$(uname -s)-$(uname -m)" = Linux-x86_64 ]; then
    tmp="$(mktemp -d)"
    if curl -fsSL --retry 3 -o "${tmp}/tool.tar.gz" "${url}" &&
      echo "${sha256}  ${tmp}/tool.tar.gz" | sha256sum -c --strict --quiet - &&
      tar -xzf "${tmp}/tool.tar.gz" -C "${tmp}"; then
      mkdir -p "${bin}"
      find "${tmp}" -type f -name "${name}" -exec install -m 0755 {} "${bin}/${name}" \;
      rm -rf "${tmp}"
      "${bin}/${name}" "${name#cargo-}" --version | grep -qF " ${version}" && return 0
    fi
    rm -rf "${tmp}" "${bin:?}/${name}"
    echo "install_tool: the ${name} ${version} release build didn't install; building it" >&2
  fi
  cargo install "${name}" --version "${version}" --locked --quiet
}

# `test_database`: a throwaway database for the Rust tests, after clearing
# what a killed run left; sets AGENT_WORKSPACE_DATABASE_URL.
test_database() {
  # Databases left by a run that was killed before it could clean up.
  psql -d "${CI_DATABASE_BASE}/postgres" -Atc \
    "SELECT datname FROM pg_database WHERE datname ~ '^aw_ci_run_'" 2>/dev/null |
    while read -r stale; do
      dropdb --force --if-exists --maintenance-db="${CI_DATABASE_BASE}/postgres" "${stale}" || true
    done
  # Test databases (aw_t_<uuid v7>) a killed run left, older than any test
  # still running; and test templates (aw_tpl_<migrations>), unless one is
  # being copied, since the first test of a run makes the one it needs.
  psql -d "${CI_DATABASE_BASE}/postgres" -Atc \
    "SELECT datname FROM pg_database WHERE datname ~ '^aw_t_[0-9a-f]{32}$'
       AND to_timestamp(('x' || substr(datname, 6, 12))::bit(48)::bigint / 1000.0)
           < now() - interval '6 hours'" 2>/dev/null |
    while read -r stale; do
      dropdb --force --if-exists --maintenance-db="${CI_DATABASE_BASE}/postgres" "${stale}" || true
    done
  psql -d "${CI_DATABASE_BASE}/postgres" -Atc \
    "SELECT datname FROM pg_database WHERE datname ~ '^aw_tpl_'" 2>/dev/null |
    while read -r stale; do
      dropdb --if-exists --maintenance-db="${CI_DATABASE_BASE}/postgres" "${stale}" 2>/dev/null || true
    done
  database="aw_ci_run_$(printf '%s' "${CI_RUN_ID:-$$}" | tr -c 'a-zA-Z0-9' '_')"
  createdb --maintenance-db="${CI_DATABASE_BASE}/postgres" "${database}"
  export AGENT_WORKSPACE_DATABASE_URL="${CI_DATABASE_BASE}/${database}"
}

# 3. Clippy, then the Rust tests against a throwaway database.
if has rust && runs lint; then
  step "cargo clippy" cargo clippy --workspace --all-targets --all-features -- -D warnings
  if web_failed; then report_web; fi
fi

if has rust && [ "${part}" = all ]; then
  if ! cargo nextest --version >/dev/null 2>&1; then
    step "install cargo-nextest" install_tool cargo-nextest
  fi
  test_database

  # Compiling the server library and its unit tests takes about 5 GB each, so
  # side by side they overrun an 8 GB cap. Under a cap, build the library and
  # binaries first; the test build then reuses the library.
  if [ -n "${CI_MEMORY_MAX:-}" ]; then
    step "rust build" cargo build --workspace
  fi
  step "rust tests" cargo nextest run --workspace --profile ci
  step "doc tests" cargo test --workspace --doc --quiet
  step "schema snapshot" tools/db/schema-snapshot.sh
  if web_failed; then report_web; fi
fi

# The rust-build part: every test binary built once, into an archive the
# rust-tests jobs each run a slice of, and the server binary the browser
# tests use (cargo builds a package's binaries for its integration tests).
if runs rust-build && [ "${part}" != all ]; then
  : "${nextest_archive:?the rust-build part needs CI_NEXTEST_ARCHIVE}"
  mkdir -p "$(dirname "${nextest_archive}")"
  step "install cargo-nextest" install_tool cargo-nextest
  # The server library and its unit tests take about 5 GB each to compile, so
  # the library and binaries come first and the tests after: built side by
  # side they got a 16 GB runner shut down, and with swap added they built no
  # faster than in order.
  if has rust; then
    step "rust build" cargo build --workspace
    step "rust test build" cargo nextest archive --workspace --archive-file "${nextest_archive}"
  fi
  # Built with the tests already (cargo builds a package's binaries for its
  # integration tests); on its own for a web-only change.
  step "server build" cargo build -p agent-workspace-server --bin agent-workspace-server
  if has rust; then
    step "doc tests" cargo test --workspace --doc --quiet
    step "schema snapshot" tools/db/schema-snapshot.sh
  fi
fi

# The rust-tests part: one slice of the archive's tests. cargo-nextest is
# called directly, since with an archive nothing needs building.
if has rust && runs rust-tests && [ "${part}" != all ]; then
  : "${nextest_archive:?the rust-tests part needs CI_NEXTEST_ARCHIVE}"
  step "install cargo-nextest" install_tool cargo-nextest
  test_database
  extracted="$(mktemp -d)"
  step "rust tests${shard:+ (slice ${shard})}" \
    "${CARGO_HOME:-${HOME}/.cargo}/bin/cargo-nextest" nextest run \
    --archive-file "${nextest_archive}" --workspace-remap . --extract-to "${extracted}" \
    --profile ci ${shard:+--partition "slice:${shard}"}
fi

if has rust && runs lint; then
  if web_failed; then report_web; fi
  step "install cargo-deny" install_tool cargo-deny
  step "cargo deny" cargo deny check
fi

report_web
if [ -n "${web_later:-}" ]; then run_web_checks; fi
# The browser smoke tests drive the server as much as the web app, so a change
# on the Rust side alone runs them too (they need the web dependencies and
# Vite's dev server, not the web build).
if [ "${part}" = all ] && has rust && ! has web; then
  step "pnpm install" pnpm install --frozen-lockfile --reporter=silent
  step "web browser smoke" run_browser_smoke
fi
# The e2e part runs them for a change on either side, as above.
if [ "${part}" = e2e ]; then
  web_toolchain
  step "web browser smoke${shard:+ (shard ${shard})}" run_browser_smoke
fi
echo "All checks passed (${areas}${CI_PART:+, ${part}${shard:+ ${shard}}})."
