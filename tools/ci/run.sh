#!/usr/bin/env bash
# Run CI for this checkout on the best machine that's free.
#
#   tools/ci/run.sh [--areas rust,web|rust|web] [--host nixos|redhat]
#
# Used by .github/workflows/ci.yml and by agents before they push (it checks
# the working tree as it is, uncommitted changes included).
#
# Machines, best first:
#   nixos   Mischa's laptop (16 threads). Used only when tools/ci/probe.sh says
#           it's idle: on mains power, no game, GPU and CPU quiet, memory free.
#           Jobs run as the unprivileged aw-ci user (modules/nixos/ci.nix in
#           /etc/nixos) at low priority with capped memory.
#   redhat  The always-on Red Hat host (4 threads), as aw-runner.
# Each machine keeps its checkout and build cache between runs, so a run only
# rebuilds what changed, and runs one CI job at a time (a lock in its work
# directory). While the laptop is up and idle, a job waits for it rather than
# falling back to the Red Hat host; if all are busy, run.sh waits.
set -euo pipefail

areas="rust,web"
only_host=""
while [ $# -gt 0 ]; do
  case "$1" in
    --areas) areas="$2"; shift 2 ;;
    --host) only_host="$2"; shift 2 ;;
    *) echo "usage: tools/ci/run.sh [--areas rust,web|rust|web] [--host nixos|redhat]" >&2; exit 2 ;;
  esac
done

case "${areas}" in
  rust,web|rust|web) ;;
  *) echo "ci: invalid areas '${areas}' (expected rust,web, rust or web)" >&2; exit 2 ;;
esac

root="$(git rev-parse --show-toplevel)"
cd "${root}"
here="${root}/tools/ci"
run_id="${GITHUB_RUN_ID:+gh${GITHUB_RUN_ID}_${GITHUB_RUN_ATTEMPT:-1}}"
run_id="${run_id:-local$(date +%s)_$$}"
ssh_opts=(-o BatchMode=yes -o ConnectTimeout=8 -o ServerAliveInterval=30 -o StrictHostKeyChecking=accept-new)

# Host settings: transport, ssh target, work directory, test database, limits.
load_host() {
  case "$1" in
    nixos)
      transport=ssh target=aw-ci@100.84.251.51 work=/var/lib/aw-ci/agent-workspace
      database_base=postgres://aw_test@127.0.0.1:55433
      # Compiling the server's library or its unit tests takes about 5 GB
      # (checks.sh builds them one after the other under a cap), so more than
      # 4 jobs overrun the memory cap.
      memory_max=8G cpu_weight=20 jobs=4 probe=yes
      # Playwright's own Chromium builds don't run on NixOS; the system Chrome does.
      browser=/run/current-system/sw/bin/google-chrome-stable
      ;;
    redhat)
      if [ "$(id -un)" = aw-runner ]; then
        transport=local target=""
      else
        transport=sudo target=cloud-user@100.94.141.127
      fi
      work=/opt/aw-runner/ci
      database_base=postgres://aw_test:aw_test@127.0.0.1:55433
      memory_max=10G cpu_weight=100 jobs=4 probe=no
      browser=""  # aw-runner has `playwright install chromium` in ~/.cache
      ;;
    *) echo "ci: unknown host $1" >&2; return 1 ;;
  esac
}

# `remote <command>`: run a bash command on the current host; stdin passes through.
remote() {
  case "${transport}" in
    local) bash -c "$1" ;;
    ssh) ssh "${ssh_opts[@]}" "${target}" "bash -lc $(printf %q "$1")" ;;
    # The runner's own environment (Node 24, rustup homes) rather than a login shell's.
    sudo) ssh "${ssh_opts[@]}" "${target}" "sudo -n -u aw-runner bash -c $(printf %q "set -a; . /opt/aw-runner/actions-runner/.env; set +a; $1")" ;;
  esac
}

# Files that changed get the time of this sync (--no-times), not the time they
# were checked out. Cargo and `tsc -b` rebuild only sources newer than their
# last build, and jobs check out long before they get a machine, so keeping
# checkout times let a run reuse another PR's newer build.
sync_source() (
  local exclude_dir path
  exclude_dir="$(mktemp -d)"
  trap 'rm -rf "${exclude_dir}"' EXIT
  # Let Git handle negation and nested ignore rules. Tracked files are never
  # ignored; keep syncing their working-tree contents, including local edits.
  git ls-files --others --ignored --exclude-standard --directory -z > "${exclude_dir}/paths"
  while IFS= read -r -d '' path; do
    # These are literal, root-relative paths, not rsync glob patterns.
    # Rsync only interprets backslash escapes in patterns containing wildcards.
    if [[ "${path}" == *'*'* || "${path}" == *'?'* || "${path}" == *'['* ]]; then
      path="${path//\\/\\\\}"
    fi
    path="${path//\*/\\*}"
    path="${path//\?/\\?}"
    path="${path//\[/\\[}"
    printf '/%s\0' "${path}"
  done < "${exclude_dir}/paths" > "${exclude_dir}/rules"
  local rsync_args=(-a --no-times --checksum --delete --exclude .git --exclude node_modules
    --exclude target --exclude dist --exclude .claude
    --from0 --exclude-from="${exclude_dir}/rules")
  remote "mkdir -p ${work}/src"
  case "${transport}" in
    local) rsync "${rsync_args[@]}" ./ "${work}/src/" ;;
    ssh) rsync "${rsync_args[@]}" -e "ssh ${ssh_opts[*]}" ./ "${target}:${work}/src/" ;;
    sudo)
      rsync "${rsync_args[@]}" -e "ssh ${ssh_opts[*]}" \
        --rsync-path="sudo -n -u aw-runner rsync" ./ "${target}:${work}/src/"
      ;;
  esac
  # Builds from before --no-times may not match the source beside them: touch
  # it all once per machine so the next build redoes everything.
  remote "cd ${work} && if [ ! -e synced-without-times ]; then
      find src \\( -name node_modules -o -name dist -o -name target \\) -prune -o -type f -exec touch {} + &&
      touch synced-without-times; fi"
)

# One CI job per machine: mkdir is atomic. The holder touches lock/alive every
# minute (heartbeat below); a lock with no heartbeat for 5 minutes is from a
# run that died without cleaning up, e.g. a cancelled job the runner killed.
acquire() {
  remote "mkdir -p ${work} && cd ${work} && now=\$(date +%s) &&
    if mkdir lock 2>/dev/null; then echo '${run_id}' \$now > lock/owner; touch lock/alive; echo acquired;
    elif [ \$((now - \$(stat -c %Y lock/alive 2>/dev/null || cut -d' ' -f2 lock/owner 2>/dev/null || echo 0))) -gt 300 ]; then
      rm -rf lock && mkdir lock && echo '${run_id}' \$now > lock/owner && touch lock/alive && echo acquired;
    else echo busy; fi" 2>/dev/null
}

heartbeat_pid=""
heartbeat() {
  while sleep 60; do
    remote "cd ${work} && grep -qs '^${run_id} ' lock/owner && touch lock/alive" < /dev/null 2>/dev/null || true
  done
}

held_host=""
release() {
  [ -z "${held_host}" ] && return 0
  if [ -n "${heartbeat_pid}" ]; then kill "${heartbeat_pid}" 2>/dev/null || true; fi
  load_host "${held_host}"
  remote "cd ${work} && if grep -qs '^${run_id} ' lock/owner; then
      pg=\$(cat lock/pgid 2>/dev/null); [ -n \"\$pg\" ] && kill -TERM -- -\$pg 2>/dev/null;
      rm -rf lock; date +%s > last_done; fi" 2>/dev/null || true
  held_host=""
}
trap 'release' EXIT
trap 'release; exit 130' INT TERM

hosts=(nixos redhat)
[ -n "${only_host}" ] && hosts=("${only_host}")
deadline=$(($(date +%s) + ${CI_WAIT_MINUTES:-45} * 60))
waited=""

while [ -z "${held_host}" ]; do
  for host in "${hosts[@]}"; do
    load_host "${host}"
    lock="$(acquire)"
    if [ -z "${lock}" ]; then
      [ -z "${waited}" ] && echo "ci: ${host} is off or unreachable"
      continue
    fi
    if [ "${lock}" = busy ]; then
      [ -z "${waited}" ] && echo "ci: ${host} is running another CI job"
      # The laptop is so much faster that waiting for it beats the next host.
      [ "${probe}" = yes ] && break
      continue
    fi
    if [ "${probe}" = yes ] &&
      status="$(remote "export CI_MIN_FREE_GB=7; [ \$((\$(date +%s) - \$(cat ${work}/last_done 2>/dev/null || echo 0))) -lt 180 ] && export CI_RECENT_RUN=1; bash -s" < "${here}/probe.sh" 2>/dev/null)" &&
      [ "${status}" != ok ]; then
      [ -z "${waited}" ] && echo "ci: ${host} skipped (${status#busy: })"
      remote "cd ${work} && grep -qs '^${run_id} ' lock/owner && rm -rf lock" 2>/dev/null || true
      continue
    fi
    held_host="${host}"
    break
  done
  if [ -z "${held_host}" ]; then
    if [ "$(date +%s)" -gt "${deadline}" ]; then
      echo "ci: no machine free after ${CI_WAIT_MINUTES:-45} minutes" >&2
      exit 1
    fi
    [ -z "${waited}" ] && echo "ci: waiting for a machine..."
    waited=1
    sleep 20 & wait $!
  fi
done

load_host "${held_host}"
heartbeat & heartbeat_pid=$!
echo "ci: running ${areas} checks on ${held_host}"
sync_source
set +e
remote "cd ${work}/src && CI_RUN_ID='${run_id}' CI_DATABASE_BASE='${database_base}' \
  CI_TARGET_DIR='${work}/target' CI_LOCK_DIR='${work}/lock' \
  CI_MEMORY_MAX='${memory_max}' CI_CPU_WEIGHT='${cpu_weight}' CARGO_BUILD_JOBS='${jobs}' \
  ${browser:+PLAYWRIGHT_CHROMIUM_EXECUTABLE='${browser}'} \
  exec setsid -w tools/ci/checks.sh '${areas}'" < /dev/null &
# Wait in the background so a cancelled job runs the traps (and frees the
# lock) straight away instead of after the checks finish.
wait $!
status=$?
set -e
exit "${status}"
