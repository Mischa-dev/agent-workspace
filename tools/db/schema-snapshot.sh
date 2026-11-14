#!/usr/bin/env bash
# Writes the schema every migration adds up to, as pg_dump prints it, to
# target/schema.sql (or the path given). Read it to see a table, function or
# trigger as it stands today instead of hunting for its last CREATE OR
# REPLACE across the migrations. Not checked in: every migration pull request
# would rewrite it and conflict. CI keeps it with each run's reports.
#
# Needs a PostgreSQL server it may create a database on: CI_DATABASE_BASE (a
# URL without a database), or else the server in AGENT_WORKSPACE_DATABASE_URL.
# It creates a scratch database, applies apps/server/migrations in version
# order, each in one transaction as the server does, dumps it and drops it.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
output="${1:-${root}/target/schema.sql}"

base="${CI_DATABASE_BASE:-}"
if [ -z "${base}" ] && [ -n "${AGENT_WORKSPACE_DATABASE_URL:-}" ]; then
  base="${AGENT_WORKSPACE_DATABASE_URL%%\?*}"
  base="${base%/*}"
fi
if [ -z "${base}" ]; then
  echo "schema-snapshot: set CI_DATABASE_BASE or AGENT_WORKSPACE_DATABASE_URL." >&2
  exit 1
fi

database="aw_schema_$$_$(date +%s)"
createdb --maintenance-db="${base}/postgres" "${database}"
trap 'dropdb --force --if-exists --maintenance-db="${base}/postgres" "${database}" 2>/dev/null || true' EXIT

# sqlx orders migrations by their numeric version, the prefix before "_".
while IFS= read -r name; do
  psql -d "${base}/${database}" -X -q -v ON_ERROR_STOP=1 --single-transaction \
    -f "${root}/apps/server/migrations/${name}" >/dev/null
done < <(ls "${root}/apps/server/migrations" | grep -E '^[0-9]+_.*\.sql$' | sort -n)

mkdir -p "$(dirname "${output}")"
# Newer pg_dump wraps the dump in \restrict lines with a random key; drop
# them so two snapshots of the same schema compare equal.
pg_dump --schema-only --no-owner -d "${base}/${database}" |
  grep -Ev '^\\(un)?restrict ' > "${output}"
echo "schema-snapshot: wrote ${output}"
