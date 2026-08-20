# Data Evolution and Migration Plan

## Migration objective

Adopt every existing Work Item as a canonical Task in place. Preserve its UUID,
Project, Organization, readable key, version, workflow, activity, comments,
criteria, links, timestamps, authorship, URLs, and audit history. Add new
contracts and features around the existing record; never copy into `tasks` and
synchronize.

## Baseline mapping

| Current storage/concept | Canonical task-platform meaning | Action |
|---|---|---|
| `work_items` | Tasks | Keep table and IDs |
| `work_item_types` | Project Task types | Keep |
| `work_item_statuses` | Project statuses | Keep |
| `work_item_status_transitions` | Workflow transition graph | Keep |
| `work_item_acceptance_criteria` | Task criteria | Keep |
| `work_item_comments` | Task comments | Keep |
| `work_item_activity` | Typed Task activity | Keep |
| `work_item_timeline_entries` | Ordered Task timeline | Keep |
| `parent_work_item_id` | Parent Task | Keep |
| blocked fields | Task blocker state | Keep |
| conversation/repository/PR links | Agent Workspace link adapters | Keep; expose conditionally |
| `outbox_events` | Durable invalidation publication | Extend typed taxonomy |

The public `Task` name does not require a table rename, view, trigger, or data
copy. Rust names may remain `WorkItem` during compatibility refactoring.

## Pre-migration inventory

Before the first implementation migration, record on a disposable restored
database:

- row counts for every table above by Project
- minimum/maximum Task number, version, board rank, and timeline sequence
- duplicate/orphan checks for parent and link relationships
- all type/status semantic categories and transition graphs
- archived/blocked/assigned counts
- comment/activity/timeline count reconciliation
- existing IDs and representative canonical URLs
- migration checksums and application release SHA

Fail the migration if unknown shapes violate the accepted schema; do not repair
ambiguous data automatically.

## Phase 0: M10 command-layer gate

M10 must first prove existing browser and MCP mutations delegate to one
application command layer with unchanged behavior. No task-platform migration
depends on unverified command names. Contract adapters are introduced only
after command transaction ownership, idempotency, optimistic conflict, event,
and audit semantics are evidenced.

## Phase 1: contract-only rollout

Add OpenAPI schema, generated client, discovery/capabilities, and a read-only
`/api/task/v1` projection over existing rows. No schema migration is required.
Compare new and compatibility projections in contract tests for the same IDs,
versions, and fields.

Then add command endpoints delegating to M10. For every command, run dual-
surface tests—not dual writes—asserting the same database effects when invoked
through either route.

## Phase 2: personal Project marker

If a stable Personal Project marker does not exist, add an additive
Organization-owned relation or constrained column. Backfill only personal
Organizations with a single unambiguous eligible Project. Otherwise leave the
marker unset and require the owner to choose/create through an explicit
idempotent command.

Never select by display name alone. Rollback ignores the additive marker; Tasks
remain ordinary Project Tasks.

## Phase 3: additive task features

Add one feature per migration/PR after command and contract design:

1. labels and Task-label links
2. canonical Task relationships
3. saved view definitions
4. import/export job metadata if required

Each uses UUIDv7 IDs, Project-scoped composite foreign keys, archive rather
than destructive delete where history matters, bounds/checks, Actor attribution,
activity/audit, and outbox facts. Backfill is empty/default unless existing
data provides an unambiguous source.

## Existing IDs, keys, and URLs

- `work_items.id` remains immutable and canonical.
- `ProjectKey-number` remains stable and only Project-scoped.
- Existing `/projects/:projectId/work-items/:id` URLs remain valid.
- New dedicated URLs use `/projects/:projectId/tasks/:id` or equivalent client
  routing and resolve the same UUID.
- API responses may include both safe client links through a link relation;
  persisted canonical URLs are not rewritten in bulk.
- Structured Message/comment references retain the Work Item ID and can render
  client-appropriate labels/URLs.

## Existing Projects and memberships

Projects remain the Task container. Organization and Project membership stay
authoritative. Do not auto-grant a task-only role or duplicate membership rows.
New task actions extend the centralized policy and are tested against existing
roles. Disabled/historical members retain attribution but receive no access.

## Existing Agent Workspace links

Link tables remain owned by their current modules. The new Task projection
loads them only when requested, deployed, and authorized. Task-only mode does
not delete or migrate link rows; an integrated database can switch modes and
retain history.

If an integrated target becomes unavailable, the Task remains readable. Never
denormalize Thread/run/repository/PR state into core Task fields merely to make
standalone reads succeed.

## Backward-compatible APIs

- Keep current `/api/v1/.../work-items` routes and MCP tool names.
- Refactor their internals to M10 before or alongside adding new routes.
- Preserve current field meaning, conflict shapes, and idempotency behavior for
  pinned clients.
- Do not return task-platform-only required fields from compatibility routes.
- Track route use through bounded metrics without logging content/credentials.
- Remove compatibility only through a separate accepted deprecation plan.

## Historical activity and audit

Existing activity kinds remain valid. New API projections map them to public
event names without rewriting rows. New activity kinds are additive and
versioned in projection mapping.

Do not synthesize `task.created` activity for existing records: their current
`work_item_created` activity is the historical fact. Do not rewrite Actor IDs,
display names, or timestamps. Audit exports distinguish the transport/client
but preserve the initiating Actor and application command.

## Rollout

1. Back up PostgreSQL and required non-secret runtime state; verify checksum and
   restore in a clean location.
2. Deploy M10-accepted release with no new client behavior.
3. Deploy read-only contract/capability endpoint dark; run projection parity.
4. Enable task API reads for operators.
5. Enable task API writes one command family at a time with contract/effect
   parity tests.
6. Enable dedicated client for one Project/owner via server config.
7. Prove cross-client same-ID/version and restart recovery.
8. Enable task-only mode on a separate clean validation deployment.
9. Add labels/relationships/views only after the core client is accepted.

Feature flags/configuration gate routes and UI, not data correctness. There is
one write path at all stages.

## Rollback

- Application rollback returns to the prior binary/assets against compatible
  additive schema.
- New routes/client are disabled first; existing routes continue.
- Additive tables/columns remain during the rollback window.
- Never reverse by copying Task data back to Work Items; it never left.
- A migration requiring destructive or non-backward-compatible change is split
  into expand/migrate/contract releases and is outside initial v1.
- If a new command has written an unsupported activity kind, prior binaries
  must tolerate unknown rows before write enablement; otherwise do not enable.

## Backup and restore tests

- pre-migration backup restores and passes checksums
- migrated backup restores into a clean task-only deployment
- migrated backup restores into integrated mode with all links intact
- point-in-time/order checks preserve Task versions and timeline sequences
- no credentials or provider caches enter ordinary backups
- retained dirty worktrees remain outside task data migration and are not
  deleted by rollback

## Required migration tests

- fresh schema through latest migration on real PostgreSQL
- upgrade from accepted post-PR-#86 fixture
- upgrade from representative M4/M6/M9 data shapes
- idempotent startup/provisioning on already-migrated data
- constraint tests for Project/Actor/parent/relationship/label scope
- count/key/UUID/version/timeline reconciliation before and after
- old server compatibility against expanded schema during rollback window
- new server compatibility with old records and absent optional features
- cross-client and MCP command-effect parity

## Acceptance gate

Migration is accepted only when evidence shows zero copied Tasks, stable IDs and
keys, equal per-Project counts, preserved versions/timeline order/attribution,
working existing URLs/APIs/MCP, successful clean restore, safe prior-binary
rollback, and task-only core use without integrated dependencies.
