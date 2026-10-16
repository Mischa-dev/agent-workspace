#!/usr/bin/env bash
# Get a fresh cloud machine ready to build and test this repository: a
# Claude Code cloud session (run from the SessionStart hook in
# .claude/settings.json) or a Codex Cloud task (paste
# `./tools/cloud/setup.sh` into the environment's setup script).
#
#   tools/cloud/setup.sh [--warm wait|background|skip] [--db-only]
#
# It starts PostgreSQL, creates a test database, exports
# AGENT_WORKSPACE_DATABASE_URL, installs the web dependencies with the pinned
# pnpm and warms the cargo build. Every step is safe to run again; run it again
# whenever integration tests say the database is unreachable (a resumed
# container doesn't always keep PostgreSQL running).
#
# The database role and password are local to this throwaway machine and
# listen on 127.0.0.1 only. Never point this script at a shared server.
set -euo pipefail

warm="wait"
db_only=""
while [ $# -gt 0 ]; do
  case "$1" in
    --warm) warm="${2:?--warm needs wait, background or skip}"; shift 2 ;;
    --warm=*) warm="${1#--warm=}"; shift ;;
    --db-only) db_only=1; shift ;;
    -h | --help) sed -n '2,16p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1" >&2; exit 2 ;;
  esac
done
case "${warm}" in wait | background | skip) ;; *) echo "--warm: wait, background or skip" >&2; exit 2 ;; esac

repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "${repo}"

db_user="agent_workspace"
db_password="agent_workspace"
db_name="agent_workspace_test"
db_port="${AGENT_WORKSPACE_CLOUD_PG_PORT:-5432}"
database_url="postgres://${db_user}:${db_password}@127.0.0.1:${db_port}/${db_name}"

log() { printf '[cloud-setup] %s\n' "$*" >&2; }

# Run a command as the postgres superuser, whoever we are.
as_postgres() {
  if [ "$(id -un)" = "postgres" ]; then
    "$@"
  elif [ "$(id -u)" -eq 0 ]; then
    if command -v runuser >/dev/null; then runuser -u postgres -- "$@"; else su postgres -s /bin/sh -c "$(printf '%q ' "$@")"; fi
  else
    sudo -n -u postgres "$@"
  fi
}

pg_bin() {
  local found
  found="$(ls -d /usr/lib/postgresql/*/bin 2>/dev/null | sort -V | tail -n 1 || true)"
  if [ -n "${found}" ]; then echo "${found}"; else dirname "$(command -v initdb 2>/dev/null || echo /usr/bin/initdb)"; fi
}

ready() { pg_isready -q -h 127.0.0.1 -p "${db_port}" 2>/dev/null; }

start_postgres() {
  if ready; then return 0; fi
  if ! command -v pg_isready >/dev/null && [ ! -x "$(pg_bin)/pg_ctl" ]; then
    if [ "$(id -u)" -eq 0 ] && command -v apt-get >/dev/null; then
      log "installing PostgreSQL"
      DEBIAN_FRONTEND=noninteractive apt-get install -y -qq postgresql >/dev/null ||
        { apt-get update -qq >/dev/null && DEBIAN_FRONTEND=noninteractive apt-get install -y -qq postgresql >/dev/null; }
    else
      log "PostgreSQL is not installed and can't be installed here; integration tests will skip"
      return 1
    fi
  fi
  export PATH="$(pg_bin):${PATH}"
  if command -v pg_lsclusters >/dev/null && [ -n "$(pg_lsclusters -h 2>/dev/null)" ]; then
    # Debian and Ubuntu: start the packaged cluster.
    read -r version cluster _ < <(pg_lsclusters -h | sort -V | tail -n 1)
    log "starting PostgreSQL ${version}/${cluster}"
    if [ "$(id -u)" -eq 0 ]; then
      pg_ctlcluster "${version}" "${cluster}" start || true
    else
      as_postgres pg_ctlcluster "${version}" "${cluster}" start || true
    fi
  else
    # Anything else: a private cluster owned by the postgres user, or by us.
    local data="${AGENT_WORKSPACE_CLOUD_PG_DATA:-/var/tmp/agent-workspace-postgres}"
    if [ "$(id -u)" -eq 0 ] && ! id postgres >/dev/null 2>&1; then
      useradd --system --home-dir "${data}" --shell /bin/sh postgres
    fi
    if [ ! -s "${data}/PG_VERSION" ]; then
      log "creating a PostgreSQL cluster in ${data}"
      mkdir -p "${data}"
      [ "$(id -u)" -eq 0 ] && chown postgres "${data}"
      as_postgres initdb -D "${data}" -U postgres --auth-local=trust --auth-host=scram-sha-256 >/dev/null
    fi
    log "starting PostgreSQL in ${data}"
    as_postgres pg_ctl -D "${data}" -l "${data}/server.log" -w \
      -o "-c listen_addresses=127.0.0.1 -p ${db_port} -k /tmp" start >/dev/null || true
  fi
  for _ in $(seq 1 30); do ready && return 0; sleep 1; done
  log "PostgreSQL did not become ready on 127.0.0.1:${db_port}"
  return 1
}

create_database() {
  local psql=(psql -X -q -v ON_ERROR_STOP=1 -h /tmp -p "${db_port}" -U postgres -d postgres)
  # Packaged clusters keep their socket in /var/run/postgresql.
  if [ -S "/var/run/postgresql/.s.PGSQL.${db_port}" ]; then
    psql=(psql -X -q -v ON_ERROR_STOP=1 -h /var/run/postgresql -p "${db_port}" -d postgres)
  fi
  as_postgres "${psql[@]}" <<SQL
SELECT format('CREATE ROLE %I LOGIN CREATEDB PASSWORD %L', '${db_user}', '${db_password}')
 WHERE NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = '${db_user}') \gexec
SELECT format('CREATE DATABASE %I OWNER %I', '${db_name}', '${db_user}')
 WHERE NOT EXISTS (SELECT 1 FROM pg_database WHERE datname = '${db_name}') \gexec
SQL
}

export_database_url() {
  export AGENT_WORKSPACE_DATABASE_URL="${database_url}"
  local line="export AGENT_WORKSPACE_DATABASE_URL='${database_url}'"
  # Claude Code: the SessionStart hook's environment file.
  if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
    grep -qxF "${line}" "${CLAUDE_ENV_FILE}" 2>/dev/null || echo "${line}" >> "${CLAUDE_ENV_FILE}"
  fi
  # Everything else (Codex Cloud, a later login shell).
  for rc in "${HOME}/.bashrc" "${HOME}/.profile"; do
    grep -qxF "${line}" "${rc}" 2>/dev/null || echo "${line}" >> "${rc}"
  done
}

database_ok=""
if [ -n "${AGENT_WORKSPACE_DATABASE_URL:-}" ] && [ "${AGENT_WORKSPACE_DATABASE_URL}" != "${database_url}" ]; then
  log "AGENT_WORKSPACE_DATABASE_URL is already set; leaving it as it is"
  database_ok=1
elif start_postgres && create_database; then
  export_database_url
  database_ok=1
  log "test database ready: AGENT_WORKSPACE_DATABASE_URL=${database_url}"
fi
[ -n "${db_only}" ] && { [ -n "${database_ok}" ]; exit $?; }

# Web dependencies with the pnpm that package.json pins.
export COREPACK_ENABLE_DOWNLOAD_PROMPT=0
if ! type -P pnpm >/dev/null || [ "$(pnpm --version 2>/dev/null)" != "$(sed -n 's/.*"packageManager": "pnpm@\([^"]*\)".*/\1/p' package.json)" ]; then
  mkdir -p "${HOME}/.local/bin"
  corepack enable --install-directory "${HOME}/.local/bin" pnpm
  export PATH="${HOME}/.local/bin:${PATH}"
  if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
    echo "export PATH=\"${HOME}/.local/bin:\${PATH}\"" >> "${CLAUDE_ENV_FILE}"
  fi
fi
log "installing web dependencies"
pnpm install --frozen-lockfile --reporter=silent

# Warm the cargo build so the first check or test doesn't compile everything.
rustup toolchain install >/dev/null 2>&1 || true
cargo fetch --locked --quiet
case "${warm}" in
  wait)
    log "warming the cargo build (cargo test --workspace --no-run)"
    cargo test --workspace --no-run --locked --quiet
    ;;
  background)
    mkdir -p target
    log "warming the cargo build in the background (target/cloud-setup-warm.log)"
    nohup nice -n 10 cargo test --workspace --no-run --locked >target/cloud-setup-warm.log 2>&1 &
    ;;
  skip) ;;
esac

[ -n "${database_ok}" ] || log "set up without a database; PostgreSQL integration tests will skip"
log "done"
