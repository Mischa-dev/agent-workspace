#!/usr/bin/env bash
# Run CI for this checkout on the best machine that's free.
#
#   tools/ci/run.sh [--areas rust,web|rust|web] [--host nixos|redhat]
#   tools/ci/run.sh --host redhat [--areas ...] --ui-tour channel
#
# Used by .github/workflows/ci.yml and by agents before they push (it checks
# the working tree as it is, uncommitted changes included).
#
# Machines, best first:
#   nixos   A NixOS laptop (16 threads). Used when tools/ci/probe.sh says it's
#           on mains power with the GPU quiet; each step then waits its turn on
#           the build admission lock and checks CPU and memory once it has it
#           (see checks.sh). Jobs run as the unprivileged aw-ci user
#           (modules/services/ci.nix in /etc/nixos) at low priority with capped memory.
#   redhat  The always-on Red Hat host (4 threads), as aw-runner.
# Each machine keeps its checkout and build cache between runs, so a run only
# rebuilds what changed, and runs one CI job at a time (a lock in its work
# directory). While the laptop is up and idle, a job waits for it rather than
# falling back to the Red Hat host; if all are busy, run.sh waits.
#
# --ui-tour channel (never the default, and only with --host redhat): after
# the browser smoke tests, the UI tour's whole seed and its channel screens, in
# the same capped step (checks.sh run_ui_tour). The four screenshots, their
# contact sheet and a summary are copied back to apps/web/e2e-output/ci-tour/;
# nothing else from the run is.
set -euo pipefail

areas="rust,web"
only_host=""
ui_tour="" ui_tour_given=""
while [ $# -gt 0 ]; do
  case "$1" in
    --areas) areas="$2"; shift 2 ;;
    --host) only_host="$2"; shift 2 ;;
    --ui-tour) ui_tour_given=1 ui_tour="${2-}"; shift $(($# > 1 ? 2 : 1)) ;;
    *) echo "usage: tools/ci/run.sh [--areas rust,web|rust|web] [--host nixos|redhat] [--ui-tour channel]" >&2; exit 2 ;;
  esac
done

if [ -n "${ui_tour_given}" ] && [ "${ui_tour}" != channel ]; then
  echo "ci: invalid --ui-tour '${ui_tour}' (expected channel)" >&2
  exit 2
fi
# The tour runs on Red Hat only, asked for by name, never on the laptop.
if [ -n "${ui_tour}" ] && [ "${only_host}" != redhat ]; then
  echo "ci: --ui-tour runs only with --host redhat" >&2
  exit 2
fi

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

# The machines' addresses aren't in the repository. They come from
# AW_CI_NIXOS_TARGET and AW_CI_REDHAT_TARGET in the environment, or from the
# untracked file ${AW_CI_HOSTS:-~/.config/aw-ci/hosts}, whose lines set the
# same names (AW_CI_REDHAT_TARGET=<user>@<address>).
hosts_file="${AW_CI_HOSTS:-${HOME}/.config/aw-ci/hosts}"
read_hosts() {
  local key value
  [ -f "${hosts_file}" ] || return 0
  while IFS='=' read -r key value; do
    case "${key}" in
      AW_CI_NIXOS_TARGET|AW_CI_REDHAT_TARGET)
        [ -n "${!key:-}" ] || printf -v "${key}" '%s' "${value}" ;;
    esac
  done < "${hosts_file}"
}
read_hosts

# `host_target <host> <variable>`: the ssh target for a host, or a clear error.
host_target() {
  local value="${!2:-}"
  if [ -z "${value}" ]; then
    echo "ci: no address for $1: set $2 in ${hosts_file} (not committed) or in the environment" >&2
    return 1
  fi
  printf '%s' "${value}"
}

# Host settings: transport, ssh target, work directory, test database, limits.
load_host() {
  case "$1" in
    nixos)
      transport=ssh work=/var/lib/aw-ci/agent-workspace
      target="$(host_target nixos AW_CI_NIXOS_TARGET)" || return 1
      database_base=postgres://aw_test@127.0.0.1:55433
      # Compiling the server's library or its unit tests takes about 5 GB
      # (checks.sh builds them one after the other under a cap). Agents build
      # here too, so CI starts with one Cargo job and takes turns with their
      # build-run jobs on one admission lock (tools/ci/admission.sh).
      memory_max=5G cpu_weight=20 jobs=1 probe=yes
      build_lock=/run/build-admission.lock
      # Each checks.sh step makes this memory check (and the old CPU one) once
      # it holds that lock; the probe below no longer does. 6 GB, still
      # provisional, is a little above the 5G cap. The first guarded run's
      # heaviest compile peaked at 4,991 MiB, and the old 7 GB stopped healthy
      # runs at about 6 GB free. It's headroom for one job, not a promise
      # about the whole machine.
      # memory_max=5G is a provisional per-job cap for the first guarded runs
      # (down from 8G), not a measured server requirement: a step that needs
      # more fails inside its own scope. It caps CI; it doesn't reserve
      # anything for the desktop.
      min_free_gb=6
      # Playwright's own Chromium builds don't run on NixOS; the system Chrome does.
      browser=/run/current-system/sw/bin/google-chrome-stable
      ;;
    redhat)
      if [ "$(id -un)" = aw-runner ]; then
        transport=local target=""
      else
        transport=sudo
        target="$(host_target redhat AW_CI_REDHAT_TARGET)" || return 1
      fi
      work=/opt/aw-runner/ci
      database_base=postgres://aw_test:aw_test@127.0.0.1:55433
      memory_max=10G cpu_weight=100 jobs=4 probe=no
      build_lock=""  # nothing else builds there
      min_free_gb=""
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

# `checks_command`: what runs checks.sh on the current host, with this run's
# settings. CI_RECENT_RUN as the probe sets it: a CI run finished here in the
# last 3 minutes, so the load average still counts it. CI_UI_TOUR only when
# --ui-tour asked for it.
checks_command() {
  echo "cd ${work}/src || exit 1
    if [ \$((\$(date +%s) - \$(cat ${work}/last_done 2>/dev/null || echo 0))) -lt 180 ]; then export CI_RECENT_RUN=1; fi
    unset CI_SCOPED  # never inherited: checks.sh sets it itself inside its capped scope
    CI_RUN_ID='${run_id}' CI_DATABASE_BASE='${database_base}' \
    CI_TARGET_DIR='${work}/target' CI_LOCK_DIR='${work}/lock' \
    CI_MEMORY_MAX='${memory_max}' CI_CPU_WEIGHT='${cpu_weight}' CARGO_BUILD_JOBS='${jobs}' \
    ${build_lock:+CI_BUILD_LOCK='${build_lock}' CI_MIN_FREE_GB='${min_free_gb}'} \
    ${browser:+PLAYWRIGHT_CHROMIUM_EXECUTABLE='${browser}'} \
    ${ui_tour:+CI_UI_TOUR='${ui_tour}'} \
    exec setsid -w tools/ci/checks.sh '${areas}'"
}

# The opt-in tour's outputs that come back here: the four channel screenshots,
# their contact sheet and the summary. Nothing else does (not the signed-in
# browser state next to them).
ui_tour_files=(02-channel-desktop-light.png 02-channel-desktop-dark.png
  02-channel-phone-light.png 02-channel-phone-dark.png index.html summary.txt)

# `ui_tour_after <checks status>`: after an opt-in tour run, copy its outputs
# back, only when every check passed, and only all of them, from this run (the
# summary names this run's ID, so an earlier run's files left on the host never
# pass for this one). Anything else fails, saying why. Returns the status to
# exit with.
ui_tour_after() {
  local status="$1" into="${root}/apps/web/e2e-output/ci-tour" name missing=()
  rm -rf "${into}"
  if [ "${status}" -ne 0 ]; then
    echo "ci: the checks failed, so no UI tour output was copied back" >&2
    return "${status}"
  fi
  mkdir -p "${into}"
  if ! remote "cd ${work}/src/apps/web/e2e-output/tour && tar -cf - ${ui_tour_files[*]}" |
    tar -xf - -C "${into}"; then
    echo "ci: couldn't copy the UI tour's output back from ${held_host:-the host}" >&2
    return 1
  fi
  for name in "${ui_tour_files[@]}"; do
    [ -s "${into}/${name}" ] || missing+=("${name}")
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    echo "ci: the UI tour's output is missing ${missing[*]}" >&2
    return 1
  fi
  if ! grep -qxF "run: ${run_id}" "${into}/summary.txt"; then
    echo "ci: the UI tour's summary isn't from this run (${run_id})" >&2
    return 1
  fi
  echo "ci: UI tour output copied to ${into}: ${ui_tour_files[*]}"
}

# One CI job per machine: mkdir is atomic. The holder touches lock/alive every
# minute (heartbeat below); a lock with no heartbeat for 5 minutes is from a
# run that died without cleaning up, e.g. a cancelled job the runner killed
# before release() reached the machine. Its checks may still be running, so
# stop them as release() would before taking the lock over.
acquire() {
  remote "mkdir -p ${work} && cd ${work} && now=\$(date +%s) &&
    if mkdir lock 2>/dev/null; then echo '${run_id}' \$now > lock/owner; touch lock/alive; echo acquired;
    elif [ \$((now - \$(stat -c %Y lock/alive 2>/dev/null || cut -d' ' -f2 lock/owner 2>/dev/null || echo 0))) -gt 300 ]; then
      pg=\$(cat lock/pgid 2>/dev/null); [ -n \"\$pg\" ] && kill -TERM -- -\$pg 2>/dev/null;
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
# Only the machines this checkout knows the address of.
known=()
for host in "${hosts[@]}"; do
  (load_host "${host}") && known+=("${host}")
done
if [ "${#known[@]}" -eq 0 ]; then
  echo "ci: no CI machine has an address; see ${hosts_file}" >&2
  exit 1
fi
hosts=("${known[@]}")
# A build-run job holds the laptop's admission lock until it ends, and CI on
# the laptop waits for that lock: started from inside one, it would wait on
# itself. Red Hat has no lock, so --host redhat still works from there.
if [ -n "${BUILD_ADMISSION_HELD:-}" ] && [[ " ${hosts[*]} " == *" nixos "* ]]; then
  echo "ci: run tools/ci/run.sh outside build-run (it holds the laptop's build lock" \
    "that CI there waits for), or pass --host redhat" >&2
  exit 2
fi
deadline=$(($(date +%s) + ${CI_WAIT_MINUTES:-45} * 60))

# ci_moves_on <status>: should a finished remote run move to the next machine?
# Only for 75 from a host with a build lock: checks.sh's refusal, once it held
# the lock, before any step ran. checks.sh reports any other 75 as 1, and a
# refusal after work began as 76, which ends the job here.
ci_moves_on() {
  [ "$1" -eq 75 ] && [ -n "${build_lock}" ]
}

# Pick a machine, run the checks there. On the laptop each step waits for the
# build admission lock and only then looks at CPU and memory. Too busy at the
# first step, before anything ran: checks.sh exits 75 and the job moves to the
# next machine, saying so. Too busy later: it stops with 76 and isn't moved.
while :; do
  waited=""
  while [ -z "${held_host}" ]; do
    for host in "${hosts[@]}"; do
      load_host "${host}"
      # An unreachable host makes ssh fail; that means "try the next one", so
      # it must not end the run under set -e.
      lock="$(acquire)" || lock=""
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
        status="$(remote "export CI_MIN_FREE_GB=7${build_lock:+ CI_PROBE_RESOURCES=0}; [ \$((\$(date +%s) - \$(cat ${work}/last_done 2>/dev/null || echo 0))) -lt 180 ] && export CI_RECENT_RUN=1; bash -s" < "${here}/probe.sh" 2>/dev/null)" &&
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
  remote "$(checks_command)" < /dev/null &
  # Wait in the background so a cancelled job runs the traps (and frees the
  # lock) straight away instead of after the checks finish.
  wait $!
  status=$?
  set -e
  if [ -n "${ui_tour}" ]; then ui_tour_after "${status}" || status=$?; fi
  if ci_moves_on "${status}"; then
    busy_host="${held_host}"
    release
    remaining=()
    for host in "${hosts[@]}"; do
      [ "${host}" = "${busy_host}" ] || remaining+=("${host}")
    done
    hosts=("${remaining[@]}")
    if [ "${#hosts[@]}" -eq 0 ]; then
      echo "ci: ${busy_host} was too busy once it had the build lock, and there's no other machine to try" >&2
      exit 75
    fi
    echo "ci: ${busy_host} was too busy once it had the build lock; moving to ${hosts[*]}"
    continue
  fi
  exit "${status}"
done
