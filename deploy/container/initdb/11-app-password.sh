#!/bin/sh
# Sets aw_app's password from its secret file, without putting it on a
# command line or in a log.
set -eu
password=$(cat "${AW_APP_PASSWORD_FILE:?}")
psql -v ON_ERROR_STOP=1 --username "$POSTGRES_USER" --dbname "$POSTGRES_DB" \
  -v app_password="$password" <<'SQL'
ALTER ROLE aw_app PASSWORD :'app_password';
SQL
