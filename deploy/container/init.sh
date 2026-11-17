#!/bin/sh
# First start on a new machine: writes .env and the database secrets with
# random values (never overwriting ones that exist), then starts the server
# and its database. Run it from this directory.
set -eu
cd "$(dirname "$0")"
umask 077
mkdir -p secrets
random() { od -An -N24 -tx1 /dev/urandom | tr -d ' \n'; }
[ -s secrets/db_admin_password ] || random > secrets/db_admin_password
[ -s secrets/db_app_password ] || random > secrets/db_app_password
if [ ! -s secrets/db_app_url ]; then
  printf 'postgres://aw_app:%s@db:5432/agent_workspace\n' "$(cat secrets/db_app_password)" > secrets/db_app_url
fi
if [ ! -f .env ]; then
  cat > .env <<'ENV'
# Where people open Agent Workspace. http is fine for localhost; any other
# address must be https (put a TLS proxy in front).
AW_PUBLIC_ORIGIN=http://localhost:3100
AW_PORT=3100
AW_PROFILE=standard
ENV
fi
# The containers read the secret files as their own users.
chmod 0644 secrets/*
# Where the backup profile writes its sets. The image's user (10001) must
# list them too, or retention can't find old sets to remove: it owns the
# folder when we may hand it over, otherwise anyone may add and list, and
# only a file's owner may remove it.
mkdir -p backups
if chown 10001:10001 backups 2>/dev/null; then
  chmod 0750 backups
else
  chmod 1777 backups
fi
if command -v docker >/dev/null 2>&1; then
  docker compose up -d
else
  podman-compose up -d
fi
echo "Starting. When 'docker compose ps' shows the app healthy, open the address in .env."
