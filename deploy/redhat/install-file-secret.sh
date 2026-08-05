#!/usr/bin/bash
set -euo pipefail

readonly secret_root="/etc/agent-workspace/secrets"

if [[ "$(/usr/bin/id -u)" -ne 0 ]]; then
  echo "Run this host-local secret installer through sudo." >&2
  exit 1
fi
if [[ $# -ne 2 ]]; then
  echo "usage: install-file-secret.sh <class> <absolute-candidate-path>" >&2
  exit 64
fi

readonly secret_class="$1"
readonly candidate="$2"
case "${secret_class}" in
  github-app-private-key)
    readonly destination="${secret_root}/github-app.pem"
    readonly minimum_bytes=64
    readonly maximum_bytes=65536
    readonly validation=private_key
    ;;
  github-webhook-current)
    readonly destination="${secret_root}/github-webhook-secret"
    readonly minimum_bytes=32
    readonly maximum_bytes=4096
    readonly validation=opaque
    ;;
  github-webhook-previous)
    readonly destination="${secret_root}/github-webhook-secret.previous"
    readonly minimum_bytes=32
    readonly maximum_bytes=4096
    readonly validation=opaque
    ;;
  *)
    echo "Unknown secret class." >&2
    exit 64
    ;;
esac

if [[ "${candidate}" != /* || ! -f "${candidate}" || -L "${candidate}" ]]; then
  echo "The candidate must be an absolute regular non-symlink file." >&2
  exit 1
fi
if [[ "$(/usr/bin/readlink -e -- "${candidate}")" != "${candidate}" ]]; then
  echo "The candidate path must not contain symlinks or unresolved components." >&2
  exit 1
fi
if [[ "${candidate}" == "${destination}" ]]; then
  echo "The candidate must be distinct from the active destination." >&2
  exit 1
fi

readonly candidate_mode="$(/usr/bin/stat -c '%a' -- "${candidate}")"
if (( (8#${candidate_mode} & 8#077) != 0 )); then
  echo "The candidate must not grant group or other permissions." >&2
  exit 1
fi
readonly candidate_bytes="$(/usr/bin/stat -c '%s' -- "${candidate}")"
if (( candidate_bytes < minimum_bytes || candidate_bytes > maximum_bytes )); then
  echo "The candidate size is outside the bounded class limit." >&2
  exit 1
fi
if [[ "${validation}" == private_key ]]; then
  /usr/bin/openssl pkey -in "${candidate}" -check -noout >/dev/null
fi

if [[ "$(/usr/bin/readlink -e -- "${secret_root}")" != "${secret_root}" ]]; then
  echo "The configured secret root is unavailable or contains a symlink." >&2
  exit 1
fi

temporary="$(/usr/bin/mktemp "${secret_root}/.${secret_class}.next.XXXXXX")"
cleanup() {
  /usr/bin/rm -f -- "${temporary}"
}
trap cleanup EXIT
/usr/bin/install \
  -o agent-workspace \
  -g agent-workspace \
  -m 0400 \
  -- "${candidate}" "${temporary}"
/usr/bin/mv -T -- "${temporary}" "${destination}"
trap - EXIT

printf '{"class":"%s","installed":true}\n' "${secret_class}"
