#!/usr/bin/env bash
# GitHub CI: prepares the job's throwaway PostgreSQL service.
#
# The service's own role is a superuser, and row rules never bind a
# superuser: the checks connect as a plain role instead, as the server does,
# so the tests prove every query keeps to its workspace (H1.63).
#
# Nothing in it has to survive a crash, so it skips flushing to disk. Every
# test copies a migrated template database, and with fsync on each copy
# waited on the runner's disk. The tests see the same SQL either way.
set -euo pipefail

superuser="postgres://aw_ci:aw_ci@127.0.0.1:5432/postgres"
psql -d "${superuser}" -v ON_ERROR_STOP=1 \
  -c "CREATE ROLE aw_app LOGIN PASSWORD 'aw_app' CREATEDB NOSUPERUSER NOBYPASSRLS" \
  -c "GRANT pg_signal_backend TO aw_app" \
  -c "ALTER SYSTEM SET fsync = off" \
  -c "ALTER SYSTEM SET synchronous_commit = off" \
  -c "ALTER SYSTEM SET full_page_writes = off" \
  -c "SELECT pg_reload_conf()"
psql -d "${superuser}" -Atc "SELECT name || '=' || setting FROM pg_settings WHERE name IN ('fsync', 'synchronous_commit', 'full_page_writes')"
