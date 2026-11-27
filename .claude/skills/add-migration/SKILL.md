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
- A migration that's on `main` can't change (CI refuses the pull request with
  the fix); add another one.
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
- Every new table belongs to a workspace: give it `workspace_id` (and
  `space_id` where it lives in a space) with a foreign key, list it in
  `TENANT_TABLES` in `src/tenancy/tables.rs`, and protect it with row
  security in the same migration: `SELECT tenancy_protect('<table>',
  'workspace_id');` (add the row's conversation as a third argument when the
  table can hold a DM's rows). `tenancy_row_rules` fails on a tenant table
  without its policy. A server-wide table goes on `GLOBAL_TABLES` with its
  reason instead. No single-process state.
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
- Tests run a migration on empty tables. One that changes or checks rows
  already there (a backfill, a new constraint, a trigger firing on the rows
  it updates) is only tried for real by the server's `preflight` command,
  which before a deploy rehearses the migrations still to run on the live
  database's own data, in a transaction it rolls back, and stops the deploy
  on the one the data refuses. When it names yours, repair the rows in a new
  migration that sorts before it. List a new migration under the pull
  request's deploy notes.
- Don't `COMMIT` inside a migration or mark one to run outside a transaction:
  the rehearsal can't roll such a migration back, so it tries none of them.
