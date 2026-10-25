#!/usr/bin/env bash
set -euo pipefail

# Existing hosts can stage Node without changing the shared Codex installation.
node_only=false
case "${1:-}" in
  --node-only) node_only=true; shift ;;
  "") ;;
  *) echo "usage: install-toolchains.sh [--node-only]" >&2; exit 2 ;;
esac
if [[ "$#" -ne 0 ]]; then
  echo "usage: install-toolchains.sh [--node-only]" >&2
  exit 2
fi

readonly node_version="24.22.1"
readonly codex_version="0.146.0"
readonly architecture="linux-arm64"
readonly install_root="/opt/agent-workspace"
readonly node_dir="${install_root}/toolchains/node-v${node_version}-${architecture}"
readonly archive="node-v${node_version}-${architecture}.tar.xz"
readonly download_root="https://nodejs.org/dist/v${node_version}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "Run this script through sudo on the Red Hat host." >&2
  exit 1
fi

temporary_dir="$(mktemp -d)"
trap 'rm -rf "${temporary_dir}"' EXIT

curl --fail --location --proto '=https' --tlsv1.2 \
  "${download_root}/${archive}" \
  --output "${temporary_dir}/${archive}"
curl --fail --location --proto '=https' --tlsv1.2 \
  "${download_root}/SHASUMS256.txt" \
  --output "${temporary_dir}/SHASUMS256.txt"

(
  cd "${temporary_dir}"
  grep " ${archive}\$" SHASUMS256.txt | sha256sum --check -
)

install -d -m 0755 "${install_root}/toolchains"
tar -xJf "${temporary_dir}/${archive}" -C "${install_root}/toolchains"

# npm and Codex use /usr/bin/env node; use this toolchain for both.
export PATH="${node_dir}/bin:${PATH}"
"${node_dir}/bin/node" --version
if ${node_only}; then
  exit 0
fi

install -d -m 0755 "${install_root}/codex/${codex_version}"
"${node_dir}/bin/npm" install \
  --global \
  --prefix "${install_root}/codex/${codex_version}" \
  "@openai/codex@${codex_version}"

install -d -m 0755 "${install_root}/bin"
ln -sfn \
  "${install_root}/codex/${codex_version}/bin/codex" \
  "${install_root}/bin/codex"

"${node_dir}/bin/node" --version
"${install_root}/bin/codex" --version
