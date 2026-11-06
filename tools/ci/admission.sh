#!/usr/bin/env bash
# Build admission: one heavy build at a time on Mischa's laptop, across users.
#
#   tools/ci/admission.sh <lock-file> <command> [args...]
#   . tools/ci/admission.sh && admission_acquire <lock-file> <what>
#
# Agents' builds (build-run, as mischa) and CI (checks.sh, as aw-ci) take an
# exclusive flock on the same root-owned file, /run/build-admission.lock, so
# they don't compile side by side. It's cooperative: anything that doesn't go
# through here isn't held back.
#
# The lock belongs to the open file description, not to a process or a scope:
# it lasts while any process still has that descriptor open, and ends when the
# last one closes it or exits. So the command must stay in the foreground and
# wait for its heavy children; a child that closes the descriptor and detaches
# runs on without the lock, and one that keeps it holds the lock until it exits.
#
# It fails closed (exit 125) instead of running unlocked when the lock file is
# missing, unreadable, a symlink, not a regular file, not owned by root, or
# writable by group or others. The file only needs to be readable: flock works
# on a read-only descriptor.
#
# One owner per chain: the command runs with BUILD_ADMISSION_HELD (the lock's
# path) and BUILD_ADMISSION_FD (the descriptor holding it). A nested acquire of
# the same lock checks that descriptor (open, the same file, still sound) and
# confirms the exclusive lock on it, instead of opening a new one that would
# wait on its own parent. A marker without that descriptor refuses.

admission_refuse() {
  echo "build admission: not running '$1': $2" >&2
  return 125
}

# admission_check_fd <lock> <what> <fd>: is descriptor <fd> the file <lock>
# names right now, and is that file sound?
admission_check_fd() {
  local lock="$1" what="$2" fd="$3" path_id fd_id mode
  if [ -L "${lock}" ]; then
    admission_refuse "${what}" "${lock} is a symlink"
    return
  fi
  fd_id="$(stat -L -c '%d:%i:%u:%f' -- "/dev/fd/${fd}" 2>/dev/null)" || fd_id=""
  path_id="$(stat -c '%d:%i:%u:%f' -- "${lock}" 2>/dev/null)" || path_id=""
  if [ -z "${fd_id}" ] || [ -z "${path_id}" ] || [ "${path_id}" != "${fd_id}" ]; then
    admission_refuse "${what}" "descriptor ${fd} isn't ${lock} (it changed, or is another file)"
    return
  fi
  mode=$((16#${fd_id##*:}))
  if [ $((mode & 0170000)) -ne $((0100000)) ]; then
    admission_refuse "${what}" "${lock} isn't a regular file"
    return
  fi
  if [ "$(cut -d: -f3 <<<"${fd_id}")" != 0 ]; then
    admission_refuse "${what}" "${lock} isn't owned by root"
    return
  fi
  if [ $((mode & 0022)) -ne 0 ]; then
    admission_refuse "${what}" "${lock} is writable by group or others"
    return
  fi
}

# admission_lock_fd <lock> <what> <fd> <wait>: hold the exclusive lock on <fd>.
# On a descriptor whose open file already holds it, this returns at once.
admission_lock_fd() {
  local lock="$1" what="$2" fd="$3" wait="$4"
  if flock -n -x "${fd}"; then
    return 0
  fi
  echo "build admission: '${what}' is waiting for another build to finish" >&2
  if ! flock -x -w "${wait}" "${fd}"; then
    admission_refuse "${what}" "still waiting for ${lock} after ${wait}s"
    return
  fi
}

# admission_acquire <lock> <what>: hold the lock on a checked descriptor (a new
# one, or the one this chain was given), waiting up to BUILD_ADMISSION_WAIT
# seconds (default 2400). The descriptor stays open in the calling shell and
# its children: run it in a subshell, or exec the command, so the lock ends
# with the command.
admission_acquire() {
  local lock="$1" what="$2" fd wait="${BUILD_ADMISSION_WAIT:-2400}"
  case "${wait}" in '' | *[!0-9]*) admission_refuse "${what}" "BUILD_ADMISSION_WAIT=${wait} isn't a number of seconds"; return ;; esac

  if [ -n "${BUILD_ADMISSION_HELD:-}" ] && [ "${BUILD_ADMISSION_HELD}" = "${lock}" ]; then
    fd="${BUILD_ADMISSION_FD:-}"
    case "${fd}" in
      '' | *[!0-9]*)
        admission_refuse "${what}" "BUILD_ADMISSION_HELD is set but no descriptor came with it (BUILD_ADMISSION_FD='${fd}')"
        return
        ;;
    esac
    if [ ! -e "/dev/fd/${fd}" ]; then
      admission_refuse "${what}" "BUILD_ADMISSION_HELD is set but descriptor ${fd} isn't open here"
      return
    fi
    admission_check_fd "${lock}" "${what}" "${fd}" || return
    admission_lock_fd "${lock}" "${what}" "${fd}" "${wait}" || return
    return 0
  fi

  if [ -L "${lock}" ]; then
    admission_refuse "${what}" "${lock} is a symlink"
    return
  fi
  if ! { exec {fd}<"${lock}"; } 2>/dev/null; then
    admission_refuse "${what}" "can't open ${lock} for reading (missing or not readable)"
    return
  fi
  if ! admission_check_fd "${lock}" "${what}" "${fd}" ||
    ! admission_lock_fd "${lock}" "${what}" "${fd}" "${wait}"; then
    exec {fd}<&-
    return 125
  fi
  export BUILD_ADMISSION_HELD="${lock}" BUILD_ADMISSION_FD="${fd}"
}

# admission_resources_ok <min-free-GiB>: the CPU and memory checks CI makes each
# time a step takes the lock, so another admitted build doesn't count against
# it. These are the probe's old rules, moved here unchanged and still
# provisional (nobody has measured what this laptop needs):
#   memory: MemAvailable at least <min-free-GiB> (run.sh passes CI_MIN_FREE_GB);
#   CPU: the 1-minute load average at most 0.4 per hardware thread, skipped
#     when CI_RECENT_RUN is set (a CI run just finished, or this one has begun,
#     and the average still counts it). That average also still counts an agent
#     build that just ended; tuning that is left for later.
# Prints why when it says no.
admission_resources_ok() {
  local min="$1" free cores load
  case "${min}" in '' | *[!0-9]*) echo "bad memory threshold '${min}'"; return 2 ;; esac
  free="$(awk '/^MemAvailable:/ { printf "%d", $2 / 1048576 }' /proc/meminfo 2>/dev/null)" || free=""
  load="$(cut -d' ' -f1 /proc/loadavg 2>/dev/null)" || load=""
  cores="$(nproc 2>/dev/null)" || cores=""
  if [ -z "${free}" ] || [ -z "${load}" ] || [ -z "${cores}" ]; then
    echo "couldn't read memory or CPU use"
    return 1
  fi
  if [ "${free}" -lt "${min}" ]; then
    echo "only ${free} GB memory free with the build lock held (wants ${min})"
    return 1
  fi
  if [ -z "${CI_RECENT_RUN:-}" ] && awk -v l="${load}" -v c="${cores}" 'BEGIN { exit !(l > c * 0.4) }'; then
    echo "CPU busy with the build lock held (load ${load} on ${cores} threads)"
    return 1
  fi
}

# Run as a command: take the lock, then become the command, so the command
# holds the descriptor (and so the lock) for as long as it and any child that
# keeps the descriptor run.
if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -euo pipefail
  if [ "$#" -lt 2 ]; then
    echo "usage: admission.sh <lock-file> <command> [args...]" >&2
    exit 2
  fi
  lock="$1"
  shift
  status=0
  admission_acquire "${lock}" "$1" || status=$?
  [ "${status}" -eq 0 ] || exit "${status}"
  exec "$@"
fi
