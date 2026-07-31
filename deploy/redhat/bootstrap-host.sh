#!/usr/bin/env bash
set -euo pipefail

readonly service_user="agent-workspace"
readonly data_root="/var/lib/agent-workspace"
readonly install_root="/opt/agent-workspace"
readonly config_root="/etc/agent-workspace"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run this script through sudo on the Red Hat host." >&2
  exit 1
fi

if ! getent passwd "${service_user}" >/dev/null; then
  useradd \
    --system \
    --create-home \
    --home-dir "${data_root}" \
    --shell /usr/sbin/nologin \
    "${service_user}"
fi

install -d -m 0750 -o "${service_user}" -g "${service_user}" \
  "${data_root}" \
  "${data_root}/attachments" \
  "${data_root}/codex/profiles" \
  "${data_root}/logs" \
  "${data_root}/repositories" \
  "${data_root}/temporary" \
  "${data_root}/worktrees"
install -d -m 0755 -o root -g root "${install_root}" "${install_root}/releases"
install -d -m 0750 -o root -g "${service_user}" "${config_root}"

install -m 0644 \
  deploy/redhat/agent-workspace.service \
  /etc/systemd/system/agent-workspace.service

systemctl daemon-reload

echo "Host directories and service account are ready."
echo "Create ${config_root}/agent-workspace.env with mode 0640 before starting the service."
