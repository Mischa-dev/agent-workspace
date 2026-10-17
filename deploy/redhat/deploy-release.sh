#!/usr/bin/env bash
set -euo pipefail

readonly checkout_root="${1:-/home/bot/deployments/agent-workspace}"
readonly commit_sha="$(git -C "${checkout_root}" rev-parse HEAD)"
readonly install_root="/opt/agent-workspace"
readonly release_root="${install_root}/releases/${commit_sha}"
readonly node_bin="${install_root}/toolchains/node-v24.18.1-linux-arm64/bin"
readonly cargo_bin="${HOME}/.local/share/agent-workspace/cargo/bin"
readonly rustup_home="${HOME}/.local/share/agent-workspace/rustup"
readonly cargo_target_root="${checkout_root}/target"

if [[ -n "$(git -C "${checkout_root}" status --porcelain)" ]]; then
  echo "Refusing to deploy a dirty checkout." >&2
  exit 1
fi

export PATH="${cargo_bin}:${node_bin}:${PATH}"
export COREPACK_HOME="${checkout_root}/.corepack"
export CARGO_HOME="${HOME}/.local/share/agent-workspace/cargo"
export RUSTUP_HOME="${rustup_home}"
# Keep Cargo's output path identical to the path installed below. Inheriting a
# caller-provided CARGO_TARGET_DIR can otherwise build the new commit elsewhere
# while copying stale binaries from this checkout's target directory.
export CARGO_TARGET_DIR="${cargo_target_root}"

# Stamped into the server for /health/version and Settings > General.
export AGENT_WORKSPACE_BUILD_COMMIT="${commit_sha}"
# The host has no swap and shares its memory with the live app and other
# services, so the build runs capped (as CI does) and is what gets killed if it
# grows too large. Override with AGENT_WORKSPACE_BUILD_MEMORY_MAX.
readonly build_memory_max="${AGENT_WORKSPACE_BUILD_MEMORY_MAX:-10G}"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"

build() {
  corepack prepare pnpm@11.18.0 --activate
  pnpm install --frozen-lockfile
  pnpm build
  cargo build --release --locked \
    -p agent-workspace-server \
    -p agent-workspace-host-maintenance
}

cd "${checkout_root}"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
if systemd-run --user --scope --quiet -p MemoryMax=1G true 2>/dev/null; then
  systemd-run --user --scope --quiet \
    -p MemoryMax="${build_memory_max}" -p MemorySwapMax=0 \
    nice -n 10 bash -euo pipefail -c "$(declare -f build); build"
else
  echo "No user systemd here: building without a memory cap." >&2
  build
fi

sudo install -d -m 0755 "${release_root}/bin" "${release_root}/web"
sudo install -m 0755 \
  "${cargo_target_root}/release/agent-workspace-server" \
  "${release_root}/bin/agent-workspace-server"
sudo install -m 0755 \
  "${cargo_target_root}/release/agent-workspace-host-maintenance" \
  "${release_root}/bin/agent-workspace-host-maintenance"
sudo install -m 0755 \
  deploy/redhat/agent-workspace-service-command \
  "${release_root}/bin/agent-workspace-service-command"
sudo install -m 0755 \
  deploy/redhat/agent-workspace-pg-dump \
  deploy/redhat/agent-workspace-backup \
  deploy/redhat/agent-workspace-backup-key \
  deploy/redhat/agent-workspace-backup-lib.sh \
  deploy/redhat/agent-workspace-daily-backup \
  deploy/redhat/agent-workspace-restore-drill \
  deploy/redhat/agent-workspace-pg-restore \
  deploy/redhat/agent-workspace-psql \
  deploy/redhat/install-file-secret.sh \
  deploy/redhat/measure-service-lifecycle.sh \
  "${release_root}/bin/"
sudo cp -R apps/web/dist/. "${release_root}/web/"
sudo chown -R root:root "${release_root}"

# The new build checks the live configuration and database before it takes
# over: keyring, harness CLIs, disk, and which migrations it would run. It
# changes nothing; a failure stops here with the old release still serving.
sudo "${release_root}/bin/agent-workspace-service-command" --release "${release_root}" preflight

sudo ln -sfn "${release_root}" "${install_root}/current.next"
sudo mv -Tf "${install_root}/current.next" "${install_root}/current"
sudo systemctl restart agent-workspace.service

# Migrations run before the server listens, so a big upgrade takes a while.
for _ in $(seq 1 120); do
  curl --fail --silent --output /dev/null http://127.0.0.1:3100/health/ready && break
  sleep 2
done
running="$(curl --fail --silent http://127.0.0.1:3100/health/version || true)"
if [[ "${running}" != *"\"commit\":\"${commit_sha}\""* ]]; then
  sudo systemctl --no-pager --full status agent-workspace.service || true
  echo "The service didn't come back on ${commit_sha}; see journalctl -u agent-workspace." >&2
  exit 1
fi
echo "Serving ${commit_sha}."

# Nightly encrypted backups (docs/operations/backups.md) start once their
# settings exist; until then, say so on every deploy.
if sudo test -f /etc/agent-workspace/backup.env; then
  sudo install -m 0644 \
    deploy/redhat/agent-workspace-daily-backup.service \
    deploy/redhat/agent-workspace-daily-backup.timer \
    /etc/systemd/system/
  sudo systemctl daemon-reload
  sudo systemctl enable --now agent-workspace-daily-backup.timer
else
  echo "Nightly backups are off: /etc/agent-workspace/backup.env doesn't exist (docs/operations/backups.md)." >&2
fi
