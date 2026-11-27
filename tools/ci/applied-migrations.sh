#!/usr/bin/env bash
# Refuse a change to a migration the base branch already has.
#
#   tools/ci/applied-migrations.sh <base-sha> <head-sha>
#
# sqlx records each applied migration's checksum and the server won't start
# on a database whose applied migration no longer matches its file. A
# migration this branch adds may still change; one the base has may not.
set -euo pipefail

base="${1:?base sha}"
head="${2:?head sha}"
migrations="apps/server/migrations"

changed="$(git diff --no-renames --diff-filter=MD --name-only "${base}...${head}" -- "${migrations}")"
if [ -z "${changed}" ]; then
  exit 0
fi

{
  echo "These migrations are already on the base branch, so databases have applied them:"
  echo "${changed}" | sed 's/^/  /'
  echo
  echo "Put each one back as the base has it:"
  echo "  git checkout $(git merge-base "${base}" "${head}") -- $(echo "${changed}" | tr '\n' ' ')"
  echo "then make the change in a new migration named by UTC timestamp,"
  echo "such as ${migrations}/$(date -u +%Y%m%d%H%M%S)_what_it_does.sql"
  echo "(.claude/skills/add-migration/SKILL.md)."
} >&2
exit 1
