#!/usr/bin/env bash
# Build and install a release from the bot's dev checkout (~bot/dev/agent-workspace).
#
# Run as cloud-user on the Red Hat host:
#   deploy/redhat/deploy-dev-checkout.sh <label>
#
# 1. Backs up the production database (custom-format pg_dump).
# 2. Builds the server and web app as bot, memory-capped.
# 3. Installs into /opt/agent-workspace/releases/<label>, switches `current`,
#    restarts the service (migrations run on start), and checks /health.
# The previous release stays in place; roll back by pointing `current` at it
# and restoring the backup if a migration ran.
set -euo pipefail

label="${1:?usage: deploy-dev-checkout.sh <label>}"
readonly checkout=/home/bot/dev/agent-workspace
readonly target=/home/bot/dev/target
readonly install_root=/opt/agent-workspace
readonly release_root="${install_root}/releases/${label}"
readonly backups=/var/backups/agent-workspace
previous="$(readlink -f "${install_root}/current")"

cd /tmp
sudo install -d -m 0700 "${backups}"
backup="${backups}/pre-${label}.dump"
echo "Backing up the database to ${backup}"
sudo -n -u bot env XDG_RUNTIME_DIR=/run/user/1002 PGDATABASE=agent_workspace \
  "${previous}/bin/agent-workspace-pg-dump" --format=custom \
  | sudo tee "${backup}" >/dev/null
sudo test -s "${backup}"

echo "Building ${label}"
sudo -n systemd-run --quiet --scope -p MemoryMax=12G -p MemorySwapMax=0 \
  --uid=bot --gid=bot --setenv=HOME=/home/bot \
  bash -c "source /home/bot/dev/env.sh && cd ${checkout} \
    && pnpm install --frozen-lockfile >/dev/null && pnpm build >/dev/null \
    && cargo build --release --locked -p agent-workspace-server -p agent-workspace-host-maintenance"

echo "Installing into ${release_root}"
sudo install -d -m 0755 "${release_root}/bin" "${release_root}/web"
sudo install -m 0755 "${target}/release/agent-workspace-server" "${release_root}/bin/"
sudo install -m 0755 "${target}/release/agent-workspace-host-maintenance" "${release_root}/bin/"
sudo install -m 0755 \
  "${checkout}/deploy/redhat/agent-workspace-service-command" \
  "${checkout}/deploy/redhat/agent-workspace-pg-dump" \
  "${checkout}/deploy/redhat/agent-workspace-pg-restore" \
  "${checkout}/deploy/redhat/agent-workspace-psql" \
  "${release_root}/bin/"
sudo cp -R "${checkout}/apps/web/dist/." "${release_root}/web/"
sudo chown -R root:root "${release_root}"

sudo ln -sfn "${release_root}" "${install_root}/current.next"
sudo mv -Tf "${install_root}/current.next" "${install_root}/current"
sudo systemctl restart agent-workspace.service

for _ in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:3100/health >/dev/null 2>&1; then
    echo "Healthy: ${label} (previous: ${previous})"
    exit 0
  fi
  sleep 2
done
echo "Health check failed. Roll back with:" >&2
echo "  sudo ln -sfn ${previous} ${install_root}/current && sudo systemctl restart agent-workspace.service" >&2
sudo journalctl -u agent-workspace.service -n 30 --no-pager >&2
exit 1
