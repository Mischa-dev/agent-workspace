#!/usr/bin/env bash
set -euo pipefail

readonly rust_version="1.97.1"
readonly rustup_target="aarch64-unknown-linux-gnu"
readonly rustup_sha256="9732d6c5e2a098d3521fca8145d826ae0aaa067ef2385ead08e6feac88fa5792"
readonly rustup_home="${HOME}/.local/share/agent-workspace/rustup"
readonly cargo_home="${HOME}/.local/share/agent-workspace/cargo"

if [[ "$(id -u)" -eq 0 ]]; then
  echo "Run this script as the bot build operator, not through sudo." >&2
  exit 1
fi

temporary_dir="$(mktemp -d)"
trap 'rm -rf "${temporary_dir}"' EXIT

curl --fail --location --proto '=https' --tlsv1.2 \
  "https://static.rust-lang.org/rustup/dist/${rustup_target}/rustup-init" \
  --output "${temporary_dir}/rustup-init"
printf '%s  %s\n' \
  "${rustup_sha256}" \
  "${temporary_dir}/rustup-init" \
  | sha256sum --check -
chmod 0755 "${temporary_dir}/rustup-init"

export RUSTUP_HOME="${rustup_home}"
export CARGO_HOME="${cargo_home}"
"${temporary_dir}/rustup-init" \
  --no-modify-path \
  --profile minimal \
  --default-toolchain "${rust_version}" \
  -y

"${cargo_home}/bin/rustc" --version
"${cargo_home}/bin/cargo" --version
