# Interface change log

A slice that changes a name, shape or behaviour another plan relies on (a
function, table, column, event, operation, error or wire format) adds a row
here, updates the owning plan's text, and says so under "Interfaces changed"
in its pull request. A rename keeps the old name working until the relying
slices move.

| Date | Slice | Interface | Old → new | Plans that rely on it |
| --- | --- | --- | --- | --- |
| 2026-11-14 | P0.8 | `roles::role_allows_sql`, `search::projects_allowing` | a custom role's permission alone → the permission and a base role that holds it | K1 |
| 2026-11-14 | P0.9 | MCP refusal body | always `retryCategory` → spelled as the tool's output schema spells it (`retry_category` where the schema says so) | K2 |
| 2026-11-14 | P0.9 | Public API status per outcome | unknown outcomes 200 → `rejected` 422, `busy` 429, `cursor_expired` 410, unknown 500; `ok`, `applied`, `found`, `events`, `requested` 200 | K2 |
| 2026-11-14 | P0.10 | Reply wake-up delivery | posted inside the claim transaction → claimed in a short transaction, posted outside, each result recorded in its own | A2 |
| 2026-11-14 | P0.11 | GitHub pull request automation | its own status SQL → `update_locked_work_item` as an integration actor; audit action `github.work_item_automation_transition` | T1, K3 |
| 2026-11-14 | P0.12 | Board drag (`move_work_item`), `task_mark_blocked`, `task_mark_ready_for_review` | own update paths → `update_locked_work_item`; a drag that changes nothing no longer bumps the version | T1 |
| 2026-11-14 | P0.13 | Rate limits | global login and MCP auth budgets count every attempt → only failures; client address from `AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER` (set `cf-connecting-ip` behind Cloudflare) | K5, H1 |
| 2026-11-14 | P0.14 | OAuth client registration | all clients count toward the cap → only unclaimed ones (1,024), and 20 registrations a minute per address | A2 |
| 2026-11-14 | P0.15, P0.16 | `page_write` (create), `decision_record`, `whiteboard_create`, `whiteboard_duplicate`, `project_create`, `file_upload`, `message_schedule`, `task_from_template`, `milestone_write` (create) | no retry key → optional `clientRequestId`; a retry answers the first result with `idempotentReplay: true`; another call with the same id is a conflict | K2 |
| 2026-11-14 | P0.17 | `agent_workspace_server::mcp::tool_catalog` | crate-private → public (doc hidden) for the contract tests | K2 |
| 2026-11-14 | P0.18 | `POST /api/v1/projects/{id}/external-agents/{id}/webhook` | saving switched to typed wake-ups → keeps the format unless `wakeupFormat: "typed"` | A2 |
| 2026-11-14 | P0.20 | Integration test helpers | spread over `agent_interface.rs`, `mcp_admin.rs`, `workspace_isolation.rs` → `tests/integration/support/` (old paths re-export) | every workstream's tests |
| 2026-11-14 | P0.21 | `support::world::setup` | a migrated schema per test, never dropped → a copy of the run's template database, dropped when the test ends | every workstream's tests |
| 2026-11-14 | P0.22 | `workspace_isolation::{http_cases, mcp_cases, get_text}` | private to the file → `pub(super)`, walked again by `privacy_inside_workspace.rs` | C2, K1 (each new read adds its case once, both nets walk it) |
| 2026-11-14 | P0.24 | `agent_workspace_server::test_support::source` | each guard's own scanner → one scanner (`rust_files`, `strip_tests`, `routes`) for the policy inventory, the module test, both isolation nets and the structure guards | K1, K2, K4 guards |
| 2026-11-14 | P0.26 | `apps/server/src/structure_guards/allowances.json` | one rule → fourteen rules with per-file allowances and ceilings | every workstream (lower a number when you remove an offender) |
| 2026-11-14 | P0.28 | SQL `conversation_readable_by(conversation_id, actor_id)` | new: one call to `conversation_visible_to` (ruling R32) | C2.1, K1, K4.4 |
| 2026-11-14 | P0.35 | `tools/db/schema-snapshot.sh` → `target/schema.sql` | new: today's schema as `pg_dump --schema-only` prints it; CI keeps it in the `ci-reports` artifact | every workstream that adds a migration |
| 2026-11-14 | P0.36 | `page_comment`, `page_suggestion` notification `detail` | `title` and `preview` copied in → ids only (`pageCommentId`, `threadId`, `pageSuggestionId`, `href`); the inbox reads the page title and the comment live, an archived page shows "(archived)" and no preview, a page moved out of the project hides its notifications | C1, K3 |
| 2026-11-14 | P0.28, P0.36 | migration names | P0.28's migration is `20261114070500_conversation_readable_by.sql` (main took `20261114070000`); the scrub is `20261114071000_notifications_page_text.sql` | none |
| 2026-11-14 | P0.37 | `mail::transport_for_actor`; `POST /api/v1/workspace-invitations/accept-existing` | a stored default workspace carried account mail → only a workspace the person owns, or their only one; accepting an invitation set the default → sets it only when the person had none and belongs nowhere else | H1 (H1.17 replaces the transport), K1 |
| 2026-11-14 | T1.5, T1.7, T1.10 | `views::RecordSource`; `views::query::{page, total, group_counts}` | `from_sql()` → `tables()`; take an executor → take `&mut PgConnection` and run in their own transaction with `SET LOCAL jit = off` (ruling R24: no pool-wide settings) | T1, K4 |
| 2026-11-14 | T1.10 | `fields::FieldType`; `fields::group_sql`; view config, records route and `record_query`; field options | nine types → eighteen (`long_text`, `email`, `phone`, `status`, and the system types `created_time`, `edited_time`, `created_by`, `edited_by`, `auto_id`, which read the record and can't be written); `group_sql` takes a `DateBucket`; new `groupBucket` (`day`, `week`, `month`) on the view config, the records route and `record_query`; an option carries an optional status `category` | T1, W1, M1 |
| 2026-11-14 | T1.3 | Writes of a task's title, field values and archive state | written on `work_items` and copied to its record → written on the record (`task_records::write_shared`; a new task's record by `task_records::create`, before the task); a Tasks record may be updated in SQL but not deleted, moved or renumbered except through its task, and a record of a task-like collection without its task at commit is refused | T1, K3, C1 |
| 2026-11-14 | T1.4a | SQL view `task_rows` | new: a task's core joined to its record under the old `work_items` column names, plus `record_version`, `record_updated_at`, `record_search_vector` and (from T1.4d) `description_vector` | every workstream that reads tasks |
| 2026-11-14 | T1.4d | `work_items.title`, `field_values`, `archived_at`, `archived_by_actor_id`, `search_vector` | dropped → read through `task_rows`; `work_items.description_vector` holds the description's words; a task inserted without its record is refused; task references (`thing_references_from_task`) read `task_rows`, and archiving a task's record refreshes them | K3, K4, C1, H1 (export), every workstream that reads tasks |
