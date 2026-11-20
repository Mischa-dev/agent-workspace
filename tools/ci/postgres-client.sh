#!/usr/bin/env bash
# GitHub CI: a PostgreSQL 18 client and age. backup_restore.rs (H1.16)
# restores a real backup set: it needs a pg_dump and pg_restore at least as
# new as the service's PostgreSQL, and age for an encrypted set, and skips
# without them.
set -euo pipefail

[ -x /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh ] ||
  { sudo apt-get update && sudo apt-get install -y postgresql-common; }
sudo /usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y
sudo apt-get install -y --no-install-recommends postgresql-client-18 age
pg_dump --version
