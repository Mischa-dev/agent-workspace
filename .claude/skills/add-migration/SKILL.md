---
name: add-migration
description: Use when a change needs a database change in Agent Workspace (a new table, column, index or constraint under apps/server/migrations).
---

# Add a database migration

## Name it by time

New migrations are named by a UTC timestamp: `apps/server/migrations/
YYYYMMDDhhmmss_what_it_does.sql`. The numbered ones stop at 0054, and
`apps/server/tests/integration/migrations_guard.rs` refuses new numbered ones
and duplicate timestamps. Several branches add migrations at once, so:

- Use the range your workstream was given (the brief or the roadmap says), or
  the current UTC time if none was.
- Never edit a migration that's on `main`; add another one.
- A migration must apply on top of any other branch's migration, in either
  order. Don't depend on something a branch that hasn't merged adds.

## Read today's schema first

`tools/db/schema-snapshot.sh` applies every migration to a scratch database
and writes the result, as `pg_dump --schema-only` prints it, to
`target/schema.sql` (it needs `AGENT_WORKSPACE_DATABASE_URL` or
`CI_DATABASE_BASE`; each CI run keeps one with its reports). Read a table,
function or trigger there instead of hunting through the migrations for its
last `CREATE OR REPLACE`. It isn't checked in, so run it again after a merge.

## Write it like the others

- Start with a comment saying which workstream it's for and what each part is.
- Every new table belongs to a workspace: give it `project_id` (or
  `organization_id`) with a foreign key, so it's scoped and isolation tests
  can reach it. No single-process state.
- Put limits in the database as well as in code: `CHECK (char_length(...)
  BETWEEN 1 AND n)`, `!~ '[[:cntrl:]]'` for one-line text, enums as
  `CHECK (x IN (...))`.
- Index for the query you'll run, with partial indexes (`WHERE archived_at IS
  NULL`) where most rows don't matter.
- Secrets never go in a column: store them through `crate::secrets` and keep
  the secret's ID.
- Adding a column to a table a query selects with `SELECT ... FROM`: update
  each `FromRow` struct that reads it (`grep -rn "FROM table_name"`).

## Check it

- `cargo test -p agent-workspace-server --test integration -- migrations_guard
  m0_database` checks the name and applies every migration to a fresh
  schema.
- Any integration test applies them too (`sqlx::migrate!` in the test setup).
- The server's `preflight` command checks a release's migrations against a
  live database before a deploy; nothing to do here, but list the new
  migration under the pull request's deploy notes.
