#!/usr/bin/env bash
# Is this machine free to run CI right now? Runs on the candidate host
# (`ssh host bash -s < probe.sh`). Prints `ok`, or `busy: <reasons>`.
#
# A personal machine is only used when nobody is using it hard: on mains
# power, no game running, GPU quiet, CPU mostly idle and enough free memory.
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

if pgrep -f 'SteamLaunch' >/dev/null || pgrep -x gamescope >/dev/null ||
  pgrep -x wineserver >/dev/null; then
  reasons+=("a game is running")
fi

if command -v nvidia-smi >/dev/null; then
  gpu="$(nvidia-smi --query-gpu=utilization.gpu --format=csv,noheader,nounits 2>/dev/null |
    sort -n | tail -1 | tr -d ' ')"
  if [ -n "${gpu}" ] && [ "${gpu}" -ge 20 ]; then
    reasons+=("GPU at ${gpu}%")
  fi
fi

cores="$(nproc)"
load="$(cut -d' ' -f1 /proc/loadavg)"
if awk -v l="${load}" -v c="${cores}" 'BEGIN { exit !(l > c * 0.4) }'; then
  reasons+=("CPU busy (load ${load} on ${cores} threads)")
fi

free_gb="$(awk '/^MemAvailable:/ { printf "%d", $2 / 1048576 }' /proc/meminfo)"
if [ "${free_gb}" -lt "${min_free_gb}" ]; then
  reasons+=("only ${free_gb} GB memory free")
fi

if [ "${#reasons[@]}" -eq 0 ]; then
  echo "ok"
else
  (IFS=','; echo "busy: ${reasons[*]}" | sed 's/,/, /g')
fi
