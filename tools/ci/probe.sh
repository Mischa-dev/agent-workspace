#!/usr/bin/env bash
# Is this machine free to run CI right now? Runs on the candidate host
# (`ssh host bash -s < probe.sh`). Prints `ok`, or `busy: <reasons>`.
#
# A personal machine is only used when it's on mains power and nobody is
# using it hard: GPU quiet, CPU mostly idle and enough free memory. It goes by
# measured use, not by which apps are open: a game that leaves the machine
# idle doesn't keep CI away.
#
# With CI_PROBE_RESOURCES=0 (the laptop, where CI takes the build admission
# lock) the CPU and memory checks are left to checks.sh, which makes them once
# it holds the lock: here an agent's build would count against them and send CI
# away instead of letting it wait its turn. Power and GPU are still checked.
set -uo pipefail

min_free_gb="${CI_MIN_FREE_GB:-7}"
reasons=()

for supply in /sys/class/power_supply/*; do
  if [ "$(cat "${supply}/type" 2>/dev/null)" = "Mains" ] &&
    [ "$(cat "${supply}/online" 2>/dev/null)" = "0" ]; then
    reasons+=("on battery")
    break
  fi
done

if command -v nvidia-smi >/dev/null; then
  gpu="$(nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>/dev/null |
    sort -n | tail -1 | tr -d ' ')"
  if [ -n "${gpu}" ] && [ "${gpu}" -ge 20 ]; then
    reasons+=("GPU at ${gpu}%")
  fi
fi

if [ "${CI_PROBE_RESOURCES:-1}" != 0 ]; then
  cores="$(nproc)"
  load="$(cut -d' ' -f1 /proc/loadavg)"
  # The load average still counts a CI run that just finished here (run.sh sets
  # CI_RECENT_RUN), so it only says the owner is busy when no run was recent.
  if [ -z "${CI_RECENT_RUN:-}" ] && awk -v l="${load}" -v c="${cores}" 'BEGIN { exit !(l > c * 0.4) }'; then
    reasons+=("CPU busy (load ${load} on ${cores} threads)")
  fi

  free_gb="$(awk '/^MemAvailable:/ { printf "%d", $2 / 1048576 }' /proc/meminfo)"
  if [ "${free_gb}" -lt "${min_free_gb}" ]; then
    reasons+=("only ${free_gb} GB memory free")
  fi
fi

if [ "${#reasons[@]}" -eq 0 ]; then
  echo "ok"
else
  (IFS=','; echo "busy: ${reasons[*]}" | sed 's/,/, /g')
fi
