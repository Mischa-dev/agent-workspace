#!/usr/bin/bash
set -euo pipefail

readonly service="agent-workspace.service"
readonly readiness_url="http://127.0.0.1:3100/health/ready"
readonly stop_budget_ms=20000
readonly start_budget_ms=10000

if [[ "$(/usr/bin/id -u)" -ne 0 ]]; then
  echo "Run this host-local lifecycle measurement through sudo." >&2
  exit 1
fi
if [[ $# -ne 0 ]]; then
  echo "usage: measure-service-lifecycle.sh" >&2
  exit 64
fi
if ! /usr/bin/systemctl is-active --quiet "${service}"; then
  echo "The supported service must be active before measurement." >&2
  exit 1
fi

release_path="$(/usr/bin/readlink -e /opt/agent-workspace/current)"
release_commit="$(/usr/bin/basename -- "${release_path}")"
if [[ ! "${release_commit}" =~ ^[0-9a-f]{40}$ ]]; then
  echo "The active release path is not named for an exact commit." >&2
  exit 1
fi

ensure_running() {
  /usr/bin/systemctl start "${service}" >/dev/null 2>&1 || true
}
trap ensure_running EXIT

stop_started_ns="$(/usr/bin/date +%s%N)"
/usr/bin/systemctl stop "${service}"
stop_finished_ns="$(/usr/bin/date +%s%N)"
stop_ms="$(( (stop_finished_ns - stop_started_ns) / 1000000 ))"

start_started_ns="$(/usr/bin/date +%s%N)"
/usr/bin/systemctl start "${service}"
ready=false
for _ in {1..100}; do
  if /usr/bin/curl --fail --silent --show-error --max-time 1 "${readiness_url}" >/dev/null 2>&1; then
    ready=true
    break
  fi
  /usr/bin/sleep 0.1
done
start_finished_ns="$(/usr/bin/date +%s%N)"
start_ms="$(( (start_finished_ns - start_started_ns) / 1000000 ))"

passed=true
if [[ "${ready}" != true || "${stop_ms}" -ge "${stop_budget_ms}" || "${start_ms}" -ge "${start_budget_ms}" ]]; then
  passed=false
fi
printf '{"schemaVersion":1,"commit":"%s","stopMs":%d,"stopBudgetMs":%d,"readyMs":%d,"readyBudgetMs":%d,"ready":%s,"passed":%s}\n' \
  "${release_commit}" "${stop_ms}" "${stop_budget_ms}" "${start_ms}" "${start_budget_ms}" "${ready}" "${passed}"

[[ "${passed}" == true ]]
