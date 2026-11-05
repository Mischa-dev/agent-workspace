#!/usr/bin/env bash
# Tests for tools/ci/admission.sh, how checks.sh, run.sh and probe.sh use it,
# and (when run inside a build-run job) the build-run candidate's nested branch.
# Local only: no CI host, no root, nothing outside a temporary directory.
#
# The lock is a private file in that directory, so separate runs of this suite
# never wait on each other. admission.sh only accepts a root-owned lock, so it
# runs inside `unshare -r`: in that user namespace our own files count as
# root's, and everyone else's as nobody's. The kernel's flock is the same
# inside and out, so holders on either side exclude each other.
set -uo pipefail
unset BUILD_ADMISSION_HELD BUILD_ADMISSION_FD BUILD_ADMISSION_WAIT CI_BUILD_LOCK CI_MIN_FREE_GB

here="$(cd "$(dirname "$0")" && pwd)"
admit_src="${here}/admission.sh"
tmp="$(mktemp -d)"
trap 'kill $(jobs -p) 2>/dev/null; rm -rf "${tmp}"' EXIT
failures=0

pass() { echo "ok   $1"; }
fail() { echo "FAIL $1${2:+: $2}"; failures=$((failures + 1)); }

if ! unshare -r true 2>/dev/null; then
  echo "SKIP admission tests: user namespaces aren't available here (unshare -r failed)"
  exit 0
fi

lock="${tmp}/admission.lock"
: > "${lock}"
chmod 0644 "${lock}"
: > "${tmp}/other.lock"
chmod 0644 "${tmp}/other.lock"
# admission.sh as it runs for real, with our files counting as root's.
admit() { unshare -r "${admit_src}" "$@"; }

# Is the lock free right now? Tried on a descriptor of our own.
lock_free() (exec {fd}<"${lock}" && flock -n -x "${fd}")
# Wait up to three seconds for the lock to come free.
comes_free() {
  local i
  for i in $(seq 30); do lock_free && return 0; sleep 0.1; done
  return 1
}
# Hold the lock for $1 seconds in the background, and wait until it's held.
hold() {
  admit "${lock}" sleep "$1" &
  holder=$!
  local i
  for i in $(seq 30); do lock_free || return 0; sleep 0.1; done
  fail "hold" "the holder never got the lock"
}
# expect <name> <status> <wanted status> <output> <wanted text>: and nothing ran.
expect() {
  local name="$1" status="$2" want="$3" out="$4" text="$5"
  if [ "${status}" -eq "${want}" ] && [[ "${out}" == *"${text}"* ]] && [ ! -e "${tmp}/ran" ]; then
    pass "${name}"
  else
    fail "${name}" "status ${status} (wanted ${want}), ran=$([ -e "${tmp}/ran" ] && echo yes || echo no), output '${out}'"
  fi
  rm -f "${tmp}/ran"
}

# 1. It refuses (125) instead of running unlocked when the lock isn't sound.
out="$(admit "${tmp}/missing.lock" touch "${tmp}/ran" 2>&1)"; expect "refuses: missing file" $? 125 "${out}" "can't open"
ln -s "${lock}" "${tmp}/link.lock"
out="$(admit "${tmp}/link.lock" touch "${tmp}/ran" 2>&1)"; expect "refuses: symlink, even to a sound lock" $? 125 "${out}" "is a symlink"
# A file owned by someone else (root, outside the namespace) counts as nobody's in it.
others="$(readlink -f "$(command -v flock)")"
out="$(admit "${others}" touch "${tmp}/ran" 2>&1)"; expect "refuses: not owned by root" $? 125 "${out}" "isn't owned by root"
mkdir "${tmp}/dir.lock"
out="$(admit "${tmp}/dir.lock" touch "${tmp}/ran" 2>&1)"; expect "refuses: directory" $? 125 "${out}" "isn't a regular file"
: > "${tmp}/writable.lock"; chmod 0666 "${tmp}/writable.lock"
out="$(admit "${tmp}/writable.lock" touch "${tmp}/ran" 2>&1)"; expect "refuses: writable by others" $? 125 "${out}" "writable by group or others"
: > "${tmp}/unreadable.lock"; chmod 000 "${tmp}/unreadable.lock"
if [ "$(id -u)" -ne 0 ]; then # outside the namespace, where we really can't read it
  out="$("${admit_src}" "${tmp}/unreadable.lock" touch "${tmp}/ran" 2>&1)"; expect "refuses: unreadable" $? 125 "${out}" "can't open"
fi
out="$(BUILD_ADMISSION_WAIT=soon admit "${lock}" touch "${tmp}/ran" 2>&1)"; expect "refuses: bad wait setting" $? 125 "${out}" "isn't a number"

# 2. A sound lock runs the command with its status, and hands it the marker:
#    the lock's path and an open descriptor for it.
out="$(admit "${lock}" bash -c 'echo "${BUILD_ADMISSION_HELD} $([ -e "/dev/fd/${BUILD_ADMISSION_FD}" ] && echo open)"; exit 7')"
status=$?
[ "${status}" -eq 7 ] && [ "${out}" = "${lock} open" ] && pass "runs the command, keeps its status, passes the descriptor" ||
  fail "runs the command, keeps its status, passes the descriptor" "status ${status}, output '${out}'"
lock_free && pass "the lock ends with the command" || fail "the lock ends with the command"

# 3. While a command holds the lock a second waits; with no time to wait it
#    refuses rather than running unlocked; given time, it starts after.
hold 2
out="$(BUILD_ADMISSION_WAIT=0 admit "${lock}" touch "${tmp}/ran" 2>&1)"; expect "a second build never runs unlocked" $? 125 "${out}" "still waiting"
start=$(date +%s.%N)
BUILD_ADMISSION_WAIT=10 admit "${lock}" true 2>/dev/null
status=$?
waited="$(awk -v a="${start}" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }')"
wait "${holder}"
[ "${status}" -eq 0 ] && awk -v w="${waited}" 'BEGIN { exit !(w >= 1) }' && pass "the second build starts when the first ends (${waited}s)" ||
  fail "the second build starts when the first ends" "status ${status} after ${waited}s"

# 4. Markers: only an inherited, open descriptor for the same sound file that
#    holds (or can take) the lock counts. Anything else refuses.
out="$(BUILD_ADMISSION_HELD="${lock}" admit "${lock}" touch "${tmp}/ran" 2>&1)"
expect "stale marker with no descriptor refuses" $? 125 "${out}" "no descriptor came with it"
out="$(BUILD_ADMISSION_HELD="${lock}" BUILD_ADMISSION_FD=nine admit "${lock}" touch "${tmp}/ran" 2>&1)"
expect "marker with a non-numeric descriptor refuses" $? 125 "${out}" "no descriptor came with it"
out="$(BUILD_ADMISSION_HELD="${lock}" BUILD_ADMISSION_FD=63 admit "${lock}" touch "${tmp}/ran" 63<&- 2>&1)"
expect "forwarded marker whose descriptor is closed refuses" $? 125 "${out}" "isn't open here"
out="$(BUILD_ADMISSION_HELD="${lock}" BUILD_ADMISSION_FD=63 admit "${lock}" touch "${tmp}/ran" 63<"${tmp}/other.lock" 2>&1)"
expect "marker whose descriptor is another file refuses" $? 125 "${out}" "isn't ${lock}"
hold 2
out="$(BUILD_ADMISSION_WAIT=0 BUILD_ADMISSION_HELD="${lock}" BUILD_ADMISSION_FD=63 admit "${lock}" touch "${tmp}/ran" 63<"${lock}" 2>&1)"
expect "marker with a fresh descriptor while someone else holds the lock refuses" $? 125 "${out}" "still waiting"
wait "${holder}"
# A fresh descriptor for the right file, lock free: it takes the lock on that descriptor.
BUILD_ADMISSION_HELD="${lock}" BUILD_ADMISSION_FD=63 admit "${lock}" sleep 1 63<"${lock}" &
taker=$!
sleep 0.5
lock_free && fail "marker with a fresh descriptor takes the lock on it" "lock wasn't held" ||
  pass "marker with a fresh descriptor takes the lock on it"
wait "${taker}"
# The genuine chain: a nested acquire confirms the parent's descriptor at once.
out="$(BUILD_ADMISSION_WAIT=0 admit "${lock}" bash -c '
  outer="${BUILD_ADMISSION_FD}"
  "$0" "$1" bash -c "echo \"nested ${outer} \${BUILD_ADMISSION_FD}\""' "${admit_src}" "${lock}" 2>&1)"
[[ "${out}" =~ ^nested\ ([0-9]+)\ ([0-9]+)$ ]] && [ "${BASH_REMATCH[1]}" = "${BASH_REMATCH[2]}" ] &&
  pass "nested acquire confirms the parent's descriptor in place" ||
  fail "nested acquire confirms the parent's descriptor in place" "${out}"

# 5. Lifetime. The lock lasts while any process with the descriptor runs; a
#    failing command frees it; a child that closes the descriptor and detaches
#    runs on without the lock (the documented limit); cancelling the process
#    group, as run.sh's release() does, frees it and leaves nothing running.
admit "${lock}" false
[ $? -eq 1 ] && lock_free && pass "a failing command keeps its status and frees the lock" ||
  fail "a failing command keeps its status and frees the lock"

setsid unshare -r "${admit_src}" "${lock}" bash -c 'sleep 30 & echo $! > "$1"; wait' _ "${tmp}/child" &
top=$!
for _ in $(seq 30); do [ -s "${tmp}/child" ] && break; sleep 0.1; done
child="$(cat "${tmp}/child" 2>/dev/null)"
kill -KILL "${top}" 2>/dev/null; wait "${top}" 2>/dev/null
[ -n "${child}" ] && kill -0 "${child}" 2>/dev/null && ! lock_free &&
  pass "a child that keeps the descriptor holds the lock after its parent dies" ||
  fail "a child that keeps the descriptor holds the lock after its parent dies" "child '${child}'"
kill -TERM "${child}" 2>/dev/null
comes_free && pass "the lock ends with the last process that has it" || fail "the lock ends with the last process that has it"

rm -f "${tmp}/child"
admit "${lock}" bash -c '( eval "exec ${BUILD_ADMISSION_FD}<&-"; sleep 5 ) & echo $! > "$1"' _ "${tmp}/child"
child="$(cat "${tmp}/child" 2>/dev/null)"
[ -n "${child}" ] && kill -0 "${child}" 2>/dev/null && lock_free &&
  pass "limit: a child that closes the descriptor and detaches outlives the lock" ||
  fail "limit: a child that closes the descriptor and detaches outlives the lock" "child '${child}'"
kill -TERM "${child}" 2>/dev/null

rm -f "${tmp}/child"
setsid unshare -r "${admit_src}" "${lock}" bash -c 'sleep 30 & echo $! > "$1"; wait' _ "${tmp}/child" &
top=$!
for _ in $(seq 30); do [ -s "${tmp}/child" ] && break; sleep 0.1; done
child="$(cat "${tmp}/child" 2>/dev/null)"
lock_free && fail "cancelling the group" "the lock wasn't held before cancelling"
kill -TERM -- "-${top}" 2>/dev/null; wait "${top}" 2>/dev/null
comes_free && ! kill -0 "${child}" 2>/dev/null && pass "cancelling the group frees the lock and stops the children" ||
  fail "cancelling the group frees the lock and stops the children"

# 6. checks.sh. Its admitted(), step() and ci_exit_status() run here as they do
#    there, in a script that exits the way checks.sh's finish trap does.
defs="$(sed -n '/^admitted() {/,/^}/p; /^step() {/,/^}/p; /^ci_exit_status() {/,/^}/p' "${here}/checks.sh")"
[ "$(grep -c '() {' <<<"${defs}")" -eq 3 ] || fail "checks.sh" "couldn't find admitted(), step() and ci_exit_status()"
in_checks() { # in_checks <script>: run <script> with checks.sh's functions, inside the namespace
  unshare -r bash -c '
    . "$1"
    admission_verdict_file="$(mktemp -p "$3")"; ci_work_started=""; admission_refused_before_work=""
    eval "$2"
    trap '"'"'exit "$(ci_exit_status "$?")"'"'"' EXIT
    set -e
    eval "$4"
  ' _ "${admit_src}" "${defs}" "${tmp}" "$1"
}
rm -f "${tmp}/steps"
CI_BUILD_LOCK="${lock}" in_checks '
  record() { echo "start $(date +%s.%N)" >> "$1"; sleep 1; echo "end $(date +%s.%N)" >> "$1"; }
  admitted one record "'"${tmp}/steps"'" & admitted two record "'"${tmp}/steps"'" & wait' 2>/dev/null
[ "$(cut -d' ' -f1 "${tmp}/steps" | tr '\n' ' ')" = "start end start end " ] && pass "independent steps side by side don't overlap" ||
  fail "independent steps side by side don't overlap" "$(tr '\n' ' ' < "${tmp}/steps")"
# A marker exported into checks.sh doesn't stand in for a step's own lock.
out="$(CI_BUILD_LOCK="${lock}" BUILD_ADMISSION_HELD="${lock}" BUILD_ADMISSION_FD=63 \
  in_checks 'admitted fmt bash -c "[ \"\${BUILD_ADMISSION_FD}\" != 63 ] && echo own"' 63<"${tmp}/other.lock" 2>&1)"
[ "${out}" = own ] && pass "each step takes its own lock, whatever marker it inherited" ||
  fail "each step takes its own lock, whatever marker it inherited" "${out}"
out="$(CI_BUILD_LOCK="${tmp}/missing.lock" in_checks "admitted fmt touch '${tmp}/ran'" 2>&1)"
expect "a step without a sound lock fails, named (125, not a move to Red Hat)" $? 125 "${out}" "not running 'fmt'"
out="$(in_checks 'admitted fmt echo unlocked' 2>&1)"
[ "${out}" = unlocked ] && pass "with no CI_BUILD_LOCK (Red Hat) steps run as before" ||
  fail "with no CI_BUILD_LOCK (Red Hat) steps run as before" "${out}"

# Refused at the first step, before anything ran: 75, nothing runs, lock left free.
out="$(CI_BUILD_LOCK="${lock}" CI_MIN_FREE_GB=999999 in_checks "step first touch '${tmp}/ran'; echo after" 2>&1)"
expect "refused before any work: exits 75 (may move to Red Hat)" $? 75 "${out}" "too busy to start CI"
[[ "${out}" != *after* ]] && pass "  and nothing after it runs" || fail "  and nothing after it runs" "${out}"
lock_free && pass "  and the lock is left free" || fail "  and the lock is left free"
# An ordinary check returning 75 is a failure (1), never a move.
out="$(CI_BUILD_LOCK="${lock}" CI_MIN_FREE_GB=0 CI_RECENT_RUN=1 in_checks 'step first bash -c "exit 75"' 2>&1)"
status=$?
[ "${status}" -eq 1 ] && [[ "${out}" == *"reporting it as an ordinary failure"* ]] &&
  pass "a check that exits 75 is reported as 1, not a refusal" ||
  fail "a check that exits 75 is reported as 1, not a refusal" "status ${status}, '${out}'"
out="$(in_checks 'step first bash -c "exit 75"' 2>&1)"
[ $? -eq 1 ] && pass "  also without a lock (Red Hat)" || fail "  also without a lock (Red Hat)" "${out}"
# Refused at a later step: 76, the earlier step ran once, the later didn't, nothing after.
rm -f "${tmp}/one" "${tmp}/two"
out="$(CI_BUILD_LOCK="${lock}" CI_MIN_FREE_GB=0 CI_RECENT_RUN=1 in_checks "
  step one touch '${tmp}/one'; CI_MIN_FREE_GB=999999; step two touch '${tmp}/two'; echo after" 2>&1)"
status=$?
if [ "${status}" -eq 76 ] && [ -e "${tmp}/one" ] && [ ! -e "${tmp}/two" ] && [[ "${out}" != *after* ]] &&
  [[ "${out}" == *"got too busy during CI"* ]]; then
  pass "refused after work began: stops with 76, earlier steps stay done, later ones don't run"
else
  fail "refused after work began" "status ${status}, one=$([ -e "${tmp}/one" ] && echo y), two=$([ -e "${tmp}/two" ] && echo y), '${out}'"
fi
lock_free && pass "  and the lock is left free" || fail "  and the lock is left free"
# Every step checks, not only the first: a later step passing the check runs.
out="$(CI_BUILD_LOCK="${lock}" CI_MIN_FREE_GB=0 CI_RECENT_RUN=1 in_checks 'step one true; step two echo second-ran' 2>&1)"
[ $? -eq 0 ] && [[ "${out}" == *second-ran* ]] && pass "later steps take the lock and check again, then run" ||
  fail "later steps take the lock and check again, then run" "${out}"
# After the first step the load average counts this run's own work (CI_RECENT_RUN).
out="$(CI_BUILD_LOCK="${lock}" CI_MIN_FREE_GB=0 CI_RECENT_RUN=1 in_checks 'unset CI_RECENT_RUN; step one true; echo "recent=${CI_RECENT_RUN:-}"' 2>&1)"
[[ "${out}" == *"recent=1"* ]] && pass "after the first step, CI_RECENT_RUN is set" || fail "after the first step, CI_RECENT_RUN is set" "${out}"
# Local first: another admitted build holds the lock; CI waits for it, then checks, then runs.
hold 2
start=$(date +%s.%N)
out="$(CI_BUILD_LOCK="${lock}" CI_MIN_FREE_GB=0 CI_RECENT_RUN=1 in_checks 'step first echo ran-here' 2>&1)"
status=$?
waited="$(awk -v a="${start}" -v b="$(date +%s.%N)" 'BEGIN { printf "%.1f", b - a }')"
wait "${holder}"
[ "${status}" -eq 0 ] && [[ "${out}" == *ran-here* ]] && awk -v w="${waited}" 'BEGIN { exit !(w >= 1) }' &&
  pass "local first: CI waits for an admitted build instead of leaving (${waited}s)" ||
  fail "local first: CI waits for an admitted build instead of leaving" "status ${status} after ${waited}s, '${out}'"
# The CPU rule is the probe's old one: 1-minute load over 0.4 per thread, skipped when recent.
cpu_says() { (. "${admit_src}"; unset CI_RECENT_RUN; admission_resources_ok 0 >/dev/null && echo ok || echo busy); }
load="$(cut -d' ' -f1 /proc/loadavg)"; cores="$(nproc)"
want="$(awk -v l="${load}" -v c="${cores}" 'BEGIN { print (l > c * 0.4) ? "busy" : "ok" }')"
got="$(cpu_says)"
[ "${got}" = "${want}" ] && pass "CPU check uses the old load-average rule (load ${load}, ${cores} threads: ${got})" ||
  fail "CPU check uses the old load-average rule" "load ${load} on ${cores}: wanted ${want}, got ${got}"
got="$(. "${admit_src}"; CI_RECENT_RUN=1 admission_resources_ok 0 && echo ok)"
[ "${got}" = ok ] && pass "  and skips it when CI_RECENT_RUN is set" || fail "  and skips it when CI_RECENT_RUN is set" "${got}"

# 7. probe.sh: power and measured GPU use only on the laptop; no app detection.
printf '#!/bin/sh\nwhile :; do sleep 1; done\n' > "${tmp}/gamescope"
chmod +x "${tmp}/gamescope"
"${tmp}/gamescope" &
fake_game=$!
for _ in $(seq 20); do pgrep -x gamescope >/dev/null && break; sleep 0.1; done
seen="$(pgrep -x gamescope >/dev/null && echo yes || echo no)"
out="$(CI_PROBE_RESOURCES=0 CI_MIN_FREE_GB=999999 bash "${here}/probe.sh")"
kill "${fake_game}" 2>/dev/null
[ "${seen}" = yes ] && [[ "${out}" != *game* ]] && [[ "${out}" != *memory* ]] && [[ "${out}" != *CPU* ]] &&
  pass "probe with CI_PROBE_RESOURCES=0 ignores a game process, CPU and memory ('${out}')" ||
  fail "probe with CI_PROBE_RESOURCES=0 ignores a game process, CPU and memory" "game process seen: ${seen}, '${out}'"
out="$(CI_MIN_FREE_GB=999999 bash "${here}/probe.sh")"
[[ "${out}" == *"GB memory free"* ]] && pass "probe without it still checks memory (Red Hat and old callers)" ||
  fail "probe without it still checks memory" "${out}"
grep -q 'on battery' "${here}/probe.sh" && ! grep -qE 'SteamLaunch|gamescope|wineserver' "${here}/probe.sh" &&
  pass "probe keeps the power check and has no game-name check" || fail "probe keeps the power check and has no game-name check"

# 8. run.sh. It can't run end to end here (it needs the hosts), so: its
#    refusal inside build-run, its move-on decision, and the lines that route
#    the laptop.
out="$(cd "${here}" && BUILD_ADMISSION_HELD=/run/build-admission.lock timeout 10 ./run.sh --areas web 2>&1)"
status=$?
[ "${status}" -eq 2 ] && [[ "${out}" == *"outside build-run"* ]] && pass "run.sh won't wait on its own build-run lock" ||
  fail "run.sh won't wait on its own build-run lock" "status ${status}, output '${out}'"
moves="$(sed -n '/^ci_moves_on() {/,/^}/p' "${here}/run.sh")"
[ -n "${moves}" ] || fail "run.sh" "couldn't find ci_moves_on()"
decide() { bash -c "${moves}"'; build_lock="$1"; ci_moves_on "$2" && echo move || echo stay' _ "$1" "$2"; }
for case in "/run/build-admission.lock 75 move" "/run/build-admission.lock 1 stay" "/run/build-admission.lock 76 stay" \
  "/run/build-admission.lock 125 stay" "/run/build-admission.lock 0 stay" "- 75 stay"; do
  set -- ${case}
  bl="$1"; [ "${bl}" = - ] && bl=""
  got="$(decide "${bl}" "$2")"
  [ "${got}" = "$3" ] && pass "run.sh after exit $2 ${bl:+with}${bl:-without} a lock: ${got}" ||
    fail "run.sh after exit $2 ${bl:+with}${bl:-without} a lock" "wanted $3, got ${got}"
done
set --
grep -q 'CI_PROBE_RESOURCES=0' "${here}/run.sh" && grep -q "CI_MIN_FREE_GB='\${min_free_gb}'" "${here}/run.sh" &&
  grep -q 'if ci_moves_on "${status}"; then' "${here}/run.sh" &&
  pass "run.sh: laptop probe skips CPU/memory, checks.sh gets the threshold, only ci_moves_on moves (static)" ||
  fail "run.sh routing lines"

# 9. The build-run candidate's nested branch, only inside a build-run job (it
#    must not start scopes of its own here). A copy uses this suite's lock and
#    admission.sh through the namespace.
candidate="${BUILD_RUN_CANDIDATE:-${HOME}/.local/share/build-admission/build-run.candidate}"
case "$(cut -d: -f3- /proc/self/cgroup)" in
  */app-t3code-agents.slice/app-t3code-agents-buildrun.slice/*)
    if [ -r "${candidate}" ]; then
      printf '#!/bin/sh\nexec unshare -r "%s" "$@"\n' "${admit_src}" > "${tmp}/admit-ns"
      chmod +x "${tmp}/admit-ns"
      sed -e "s|^lock=/run/build-admission.lock$|lock='${lock}'|" \
        -e "s|^admit=\"\${HOME}/.local/share/build-admission/admission.sh\"$|admit='${tmp}/admit-ns'|" \
        "${candidate}" > "${tmp}/build-run"
      chmod +x "${tmp}/build-run"
      if grep -q "^lock='${lock}'" "${tmp}/build-run" && grep -q "^admit='${tmp}/admit-ns'" "${tmp}/build-run"; then
        mine="$(cut -d: -f3- /proc/self/cgroup)"
        out="$("${tmp}/build-run" bash -c 'cut -d: -f3- /proc/self/cgroup; exit 4' 2>&1)"
        status=$?
        [ "${status}" -eq 4 ] && [ "${out}" = "${mine}" ] && pass "nested build-run, lock free: runs in place, same scope, keeps its status" ||
          fail "nested build-run, lock free: runs in place" "status ${status}, '${out}'"
        hold 3
        start=$(date +%s)
        out="$("${tmp}/build-run" touch "${tmp}/ran" 2>&1)"
        expect "nested build-run without admission while the lock is held refuses at once" $? 125 "${out}" "still waiting"
        [ $(($(date +%s) - start)) -le 2 ] && pass "  (without waiting on its own job)" || fail "  it waited"
        out="$(BUILD_ADMISSION_HELD="${lock}" "${tmp}/build-run" touch "${tmp}/ran" 2>&1)"
        expect "nested build-run with a forwarded marker and no descriptor refuses" $? 125 "${out}" "no descriptor came with it"
        wait "${holder}"
      else
        fail "build-run candidate" "couldn't point the copy at the test lock"
      fi
    else
      echo "SKIP build-run candidate: ${candidate} not found"
    fi
    ;;
  *) echo "SKIP build-run candidate nested branch: run this suite inside build-run to test it" ;;
esac

# 10. enable-shared-queue.sh, up to its one root step. A copy points at a
#     path here, and a stub sudo records the command and its input instead of
#     running it. What happens after root's step can only be seen for real.
enable="${ENABLE_SCRIPT:-${HOME}/.local/share/build-admission/enable-shared-queue.sh}"
if [ -r "${enable}" ]; then
  mkdir -p "${tmp}/stubs"
  printf '#!/bin/sh\necho "$*" > "%s/sudo-args"; cat > "%s/sudo-input"\n' "${tmp}" "${tmp}" > "${tmp}/stubs/sudo"
  printf '#!/bin/sh\n[ "$1" = -u ] && { echo 1001; exit; }; exec /run/current-system/sw/bin/id "$@"\n' > "${tmp}/stubs/id-1001"
  chmod +x "${tmp}/stubs/sudo" "${tmp}/stubs/id-1001"
  sed "s|^lock=/run/build-admission.lock$|lock='${tmp}/queue.lock'|" "${enable}" > "${tmp}/enable.sh"
  grep -q "^lock='${tmp}/queue.lock'" "${tmp}/enable.sh" || fail "enable script" "couldn't point the copy at a test path"
  run_enable() { rm -f "${tmp}/sudo-args" "${tmp}/sudo-input"; PATH="${tmp}/stubs:${PATH}" bash "${tmp}/enable.sh" 2>&1; }
  refuses_before_sudo() {
    local name="$1" text="$2" out status
    out="$(run_enable)"; status=$?
    [ "${status}" -ne 0 ] && [[ "${out}" == *"${text}"* ]] && [ ! -e "${tmp}/sudo-args" ] &&
      pass "enable script: ${name}, before sudo" || fail "enable script: ${name}" "status ${status}, sudo=$([ -e "${tmp}/sudo-args" ] && echo called), '${out}'"
  }
  mkdir -p "${tmp}/stubs-uid"
  cp "${tmp}/stubs/sudo" "${tmp}/stubs-uid/sudo"; cp "${tmp}/stubs/id-1001" "${tmp}/stubs-uid/id"
  out="$(rm -f "${tmp}/sudo-args"; PATH="${tmp}/stubs-uid:${PATH}" bash "${tmp}/enable.sh" 2>&1)"
  [ $? -ne 0 ] && [[ "${out}" == *"run this as mischa"* ]] && [ ! -e "${tmp}/sudo-args" ] &&
    pass "enable script: refuses another invoking uid, before sudo" || fail "enable script: refuses another invoking uid" "${out}"
  if [ "$(id -u)" = 1000 ]; then
    : > "${tmp}/queue.lock"
    refuses_before_sudo "refuses an existing file not owned by root" "owned by uid 1000, not root"
    rm -f "${tmp}/queue.lock"; ln -s "${lock}" "${tmp}/queue.lock"
    refuses_before_sudo "refuses a symlink" "is a symlink"
    rm -f "${tmp}/queue.lock"; mkdir "${tmp}/queue.lock"
    refuses_before_sudo "refuses a directory" "isn't a regular file"
    rmdir "${tmp}/queue.lock"
    run_enable >/dev/null
    want_input="$(printf '%s\n' 'f /run/build-admission.lock 0600 root root -' 'a /run/build-admission.lock - - - - u:mischa:r,u:aw-ci:r,u:gh-runner:r')"
    [ "$(cat "${tmp}/sudo-args" 2>/dev/null)" = "systemd-tmpfiles --create -" ] && [ "$(cat "${tmp}/sudo-input" 2>/dev/null)" = "${want_input}" ] &&
      pass "enable script: its one root step is exactly the approved tmpfiles f+a command" ||
      fail "enable script: root step" "args '$(cat "${tmp}/sudo-args" 2>/dev/null)', input '$(cat "${tmp}/sudo-input" 2>/dev/null)'"
    echo "SKIP enable script after sudo (owner, ACL, inode kept, open vs busy): needs the real root step"
  else
    echo "SKIP enable script path cases: they assume this suite runs as mischa (uid 1000)"
  fi
else
  echo "SKIP enable script: ${enable} not found"
fi

# 11. checks.sh's capped-scope bootstrap. Stubs stand in for systemd-run (one
#     that can't make a scope, one that records what it was asked) and for
#     nice, which is what would run the checks: if nice is reached, they ran.
mkdir -p "${tmp}/scope-fails" "${tmp}/scope-works"
printf '#!/bin/sh\nexit 1\n' > "${tmp}/scope-fails/systemd-run"
printf '#!/bin/sh\necho "$*" >> "%s/scope-args"\n' "${tmp}" > "${tmp}/scope-works/systemd-run"
for d in scope-fails scope-works; do
  printf '#!/bin/sh\necho "$*" > "%s/nice-ran"\n' "${tmp}" > "${tmp}/${d}/nice"
  chmod +x "${tmp}/${d}/systemd-run" "${tmp}/${d}/nice"
done
bootstrap() { # bootstrap <stub dir> [VAR=value...]: run checks.sh's bootstrap, report what happened
  local dir="$1"
  shift
  rm -f "${tmp}/nice-ran" "${tmp}/scope-args"
  (cd "${tmp}" && env -u CI_SCOPED -u CI_LOCK_DIR "$@" PATH="${tmp}/${dir}:${PATH}" bash "${here}/checks.sh" rust 2>&1)
}
out="$(bootstrap scope-fails CI_BUILD_LOCK=/run/build-admission.lock CI_MEMORY_MAX=8G)"
status=$?
[ "${status}" -eq 125 ] && [ ! -e "${tmp}/nice-ran" ] && [[ "${out}" == *"needs a capped scope"* ]] &&
  pass "laptop route: no capped scope, so no checks run (125)" ||
  fail "laptop route: no capped scope, so no checks run" "status ${status}, nice=$([ -e "${tmp}/nice-ran" ] && echo ran), '${out}'"
out="$(bootstrap scope-works CI_BUILD_LOCK=/run/build-admission.lock CI_MEMORY_MAX=)"
status=$?
[ "${status}" -eq 125 ] && [ ! -e "${tmp}/nice-ran" ] && [ ! -e "${tmp}/scope-args" ] &&
  pass "laptop route: no memory cap asked for, so no checks run (125)" ||
  fail "laptop route: no memory cap asked for, so no checks run" "status ${status}, '${out}'"
out="$(bootstrap scope-works CI_BUILD_LOCK=/run/build-admission.lock CI_MEMORY_MAX=8G)"
status=$?
if [ "${status}" -eq 0 ] && [ ! -e "${tmp}/nice-ran" ] &&
  grep -q -- '-p MemoryMax=8G -p MemorySwapMax=0' "${tmp}/scope-args" 2>/dev/null &&
  grep -q 'nice -n 10 .*checks.sh rust' "${tmp}/scope-args" 2>/dev/null; then
  pass "laptop route: with a scope, checks.sh re-runs itself inside it with MemoryMax=8G"
else
  fail "laptop route: with a scope, checks.sh re-runs itself inside it" "status ${status}, args '$(tr '\n' '|' < "${tmp}/scope-args" 2>/dev/null)'"
fi
# The probe works but the real capped scope can't be made: exec fails, nothing runs.
mkdir -p "${tmp}/scope-probe-only"
printf '#!/bin/sh\necho "$*" >> "%s/scope-args"\ncase "$*" in *"MemoryMax=1G true") exit 0 ;; esac\nexit 1\n' "${tmp}" > "${tmp}/scope-probe-only/systemd-run"
printf '#!/bin/sh\necho "$*" > "%s/nice-ran"\n' "${tmp}" > "${tmp}/scope-probe-only/nice"
chmod +x "${tmp}/scope-probe-only/systemd-run" "${tmp}/scope-probe-only/nice"
out="$(bootstrap scope-probe-only CI_BUILD_LOCK=/run/build-admission.lock CI_MEMORY_MAX=8G)"
status=$?
[ "${status}" -ne 0 ] && [ ! -e "${tmp}/nice-ran" ] && [ "$(grep -c 'MemoryMax=8G' "${tmp}/scope-args" 2>/dev/null)" = 1 ] &&
  pass "laptop route: probe works but the capped scope fails, so no checks run (${status})" ||
  fail "laptop route: probe works but the capped scope fails" "status ${status}, nice=$([ -e "${tmp}/nice-ran" ] && echo ran), '${out}'"
# An inherited CI_SCOPED=1 would skip the bootstrap: run.sh's remote command unsets
# it right before starting checks.sh. That same sequence, with no scope possible:
rm -f "${tmp}/nice-ran" "${tmp}/scope-args"
out="$(cd "${tmp}" && env -u CI_LOCK_DIR CI_SCOPED=1 PATH="${tmp}/scope-fails:${PATH}" bash -c '
  unset CI_SCOPED
  CI_BUILD_LOCK=/run/build-admission.lock CI_MEMORY_MAX=8G exec bash "$1" rust' _ "${here}/checks.sh" 2>&1)"
status=$?
[ "${status}" -eq 125 ] && [ ! -e "${tmp}/nice-ran" ] && [[ "${out}" == *"needs a capped scope"* ]] &&
  pass "inherited CI_SCOPED=1, unset as run.sh does, then no scope: no checks run (125)" ||
  fail "inherited CI_SCOPED=1, unset as run.sh does" "status ${status}, '${out}'"
# ...and run.sh's remote command does unset it, after the cd and before the checks start.
grep -A1 'then export CI_RECENT_RUN=1; fi' "${here}/run.sh" | grep -q '^    unset CI_SCOPED ' &&
  grep -A7 '^    unset CI_SCOPED ' "${here}/run.sh" | grep -q "exec setsid -w tools/ci/checks.sh" &&
  pass "run.sh's remote command unsets CI_SCOPED right before starting checks.sh (static)" ||
  fail "run.sh's remote command unsets CI_SCOPED right before starting checks.sh"
out="$(bootstrap scope-fails CI_MEMORY_MAX=10G)"
status=$?
[ "${status}" -eq 0 ] && grep -q 'checks.sh rust' "${tmp}/nice-ran" 2>/dev/null &&
  pass "no CI_BUILD_LOCK (Red Hat): no scope still runs the checks at low priority, as before" ||
  fail "no CI_BUILD_LOCK (Red Hat): falls back as before" "status ${status}, '${out}'"

lock_free && pass "no test left the lock held" || fail "no test left the lock held"
if [ "${failures}" -gt 0 ]; then
  echo "${failures} admission test(s) failed"
  exit 1
fi
echo "All admission tests passed."
