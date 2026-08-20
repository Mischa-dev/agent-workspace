# Current System Assessment

Assessment base: `main` at `61bb577034d77dbcdb1d06f141036f51ede4bb7f`
(merge of PR #86), inspected August 19, 2026. This is implementation evidence,
not a claim about the not-yet-implemented M10 layer.

## Storage and schema

M4 introduced the canonical Work Item aggregate in migrations `0015`–`0019`;
M5 added pull-request links/automation; M6 added parent, blocker, and MCP command
state. The core tables are:

- `work_items`: UUID, Project, Project-local number, title, description, stable
  type/status IDs, priority, assignee Actor, creator Actor, archive attribution,
  board rank, version, parent, blocked state, timestamps, and next timeline
  sequence.
- `work_item_types`, `work_item_statuses`, and
  `work_item_status_transitions`: Project-owned workflow definitions.
- `work_item_acceptance_criteria`: ordered criteria and completion attribution.
- `work_item_comments`: immutable structured version-one AST plus plain-text
  projection and author Actor.
- `work_item_activity` and `work_item_timeline_entries`: typed append-only
  activity/comments with per-Task monotonic sequence.
- structured mention/reference projections for comments and Messages.
- conversation, Repository, pull-request, and Worker-invocation link tables.

Project-scoped composite foreign keys constrain workflow, membership, and link
ownership. UUIDv7 is used for application identity; readable keys combine the
Project key and monotonically allocated number. Archive preserves history.

## Workflow, type, priority, and state

Default types are Feature, Bug, Task, Request, Research, and Improvement.
Default status semantics are Backlog, Ready, In Progress, Review, Verification,
Done, and Canceled, stored through stable status IDs plus semantic categories.
The allowed transition graph is persisted. Business logic validates transitions
instead of trusting display names.

Priorities are `no_priority`, `low`, `medium`, `high`, and `urgent`. Board rank
is stored on the same Work Item. List and Kanban are projections; there is no
separate board entity. GitHub automation can optionally move an explicitly
linked item to Review on PR open and, by default, to Verification on merge. It
never completes or reopens work and must pass ordinary transition validation.

## Ownership and identity

Every Work Item belongs to one Project; every Project belongs to one
Organization. Project membership references `actors.id`. Creation, updates,
comments, criteria completion, archive, automation, and links retain Actor
attribution. Assignment accepts active human or external-agent Project members,
not Execution Workers.

Humans, external agents, Execution Workers, integrations, and system actions
share the Actor model. Credentials/sessions authenticate a client; centralized
policy authorizes an Actor. Browser and MCP requests cannot select authorship.

## HTTP surface

`apps/server/src/work_items.rs` currently owns these `/api/v1` families:

- Project list/create and board projection
- detail by UUID and lookup by readable key
- update, archive, restore, and board move
- workflow, assignee search, and reference search
- criteria list/create/update/remove
- immutable comment append and ordered timeline
- Thread link/unlink
- Repository link/unlink/set-primary
- Work-with-Worker orchestration

Handlers use opaque cursor pagination, bounded filters, server-returned
capabilities, optimistic `expectedVersion`, and `Idempotency-Key` on mutations.
Conflict responses include the current canonical Work Item.

The HTTP representation currently expands criteria, linked Threads,
repositories, and pull requests on detail. M6 parent and blocker fields are
available through the canonical storage/MCP context but are not yet a complete
dedicated-browser product surface.

## MCP and external agents

The Streamable HTTP MCP server exposes bounded reads including
`project_get`, `project_instructions_get`, `work_item_get`,
`work_item_search`, `work_item_context_get`, and `pull_request_get`.
Work Item commands include create, set status, post update, constrained update,
create subtask, link pull request, mark blocked, and mark ready for review.

MCP credentials resolve one Actor and Project with scopes, active memberships,
ordinary policy, audit, idempotent `clientRequestId`, and optimistic versions.
OAuth authorization code with PKCE is supported alongside managed scoped
bearers. External-agent connections reuse the MCP boundary and add connection-
bound polling, credential rotation/revocation, approval tools, and human-
sponsored Worker requests.

Current browser and MCP implementations share concepts and policy but retain
mutation logic in separate modules. That is the concrete reason M10 must become
the single application command layer before another write transport is added.

## Realtime and external event delivery

Canonical browser invalidations are inserted into `outbox_events` in source
transactions. A PostgreSQL-backed publisher leases, serializes, broadcasts, and
marks records with retry/failure recovery. WebSocket delivery is authenticated,
Project-filtered, at-least-once at the publication boundary, and used for
invalidation/refetch rather than correctness.

Supported current browser event families include conversation create/archive,
message create, read-state update, Agent Run update, Work Item update, and
membership update. The current Work Item envelope is a coarse
`work_item.updated` invalidation.

External-agent events are separate addressed records with per-connection
sequence, HMAC-signed connection/Project/credential-bound cursors, 30-day
retention, bounded summary/resource/path, at-least-once polling, and canonical
refetch. Current task-related addressed events focus on assignment/update and
approval attention rather than a complete public domain event log.

## Comments, activity, and references

Comments are immutable and reuse the versioned Message content AST. Mention and
Work Item reference nodes are projected into constrained tables and validated
against the stored AST. Task activity is typed and separate from comments. A
single timeline interleaves both by an allocated sequence, so timestamp ties do
not reorder history.

Acceptance criteria are normalized ordered rows. Criteria creation, editing,
reorder, completion/reopen, and removal create attributable activity and
participate in Work Item concurrency.

## Subtasks and relationships

The schema has one optional `parent_work_item_id`, same-Project enforcement,
and a not-own-parent constraint. MCP provides subtask creation. There is not yet
a generalized canonical relationship table for blocks/relates/duplicates;
blocker state is stored as a reason/Actor/time on the Work Item.

## Threads, repositories, pull requests, and runs

Threads link many-to-many and remain separate Conversations. Repositories link
many-to-many with at most one primary Repository. Pull requests link explicitly
with confirmed/unconfirmed source and retain GitHub-owned state projections.
Work-with-Worker creates and links a Thread before invoking the accepted Agent
Run path; Worker completion does not complete the Work Item.

These are intrinsically Agent Workspace integrations and should remain link
adapters around the reusable Task aggregate.

## Frontend

The React app has Project-scoped routes for Work Item list/board and detail.
`WorkItemsPage` supports filters, pagination, create, and Kanban movement.
`WorkItemDetailPage` supports field edits, archive, criteria, timeline/comments,
Thread and Repository links, pull-request state, Codex deep link, and Work with
Worker. TanStack Query keys are Project-scoped. Realtime invalidates/refetches
task roots and details.

The client types and response normalizers are handwritten. Some normalization
uses permissive fallbacks. A second client would make schema-backed generation
and strict runtime validation materially safer.

## Deployment and authentication

Version one is one Rust server, PostgreSQL, static React assets, WebSocket,
MCP, jobs/outbox, Git/GitHub modules, and Codex App Server on a private Project
Host. The server listens loopback-first and is the only application surface
exposed through the accepted private/TLS boundary.

Human authentication uses Argon2id local passwords, opaque digest-only
sessions, strict same-site cookies, allowed-Origin plus CSRF enforcement,
rotation/revocation, and invitation-based team onboarding. PostgreSQL, provider,
Git, and secret material are not browser-exposed.

## Reusable versus Agent Workspace-specific

| Reusable task platform | Agent Workspace-specific |
|---|---|
| Task identity/fields/version/archive | Channels, Messages, Threads |
| Project workflow/types/status/rank | Agent Runs and Provider Sessions |
| Assignment, criteria, comments/activity | Execution Workers and Profiles |
| Parent/subtask and blocker state | Git worktrees and repository preparation |
| Search/list/board projections | GitHub credential/webhook/sync operations |
| Actor/Organization/Project policy | Work-with-Worker orchestration |
| Idempotency/concurrency/audit/outbox | Provider events, approvals, recovery |
| Task event facts and notification intent | Integrated context presentation |

Identity, membership, Project, policy, audit, idempotency, and outbox are shared
platform infrastructure used by both domains rather than duplicated inside the
Task module.

## Concrete gaps before a dedicated client

- M10 application command/query evidence and transport effect parity
- reviewed versioned Task contract and strict generated client
- dedicated shell and task-only capability/runtime mode
- stable Personal Project marker and Inbox provisioning
- labels, generalized relationships, saved views, and justified batch actions
- richer canonical Task event taxonomy and notification preferences
- migration/rollback/cross-client acceptance against retained data
