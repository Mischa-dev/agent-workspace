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

cd "${checkout_root}"
corepack prepare pnpm@11.18.0 --activate
pnpm install --frozen-lockfile
pnpm build
cargo build --release --locked \
  -p agent-workspace-server \
  -p agent-workspace-host-maintenance

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

sudo ln -sfn "${release_root}" "${install_root}/current.next"
sudo mv -Tf "${install_root}/current.next" "${install_root}/current"
sudo systemctl restart agent-workspace.service
sudo systemctl --no-pager --full status agent-workspace.service
