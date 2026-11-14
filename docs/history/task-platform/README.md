> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# Task Platform Architecture

Status: accepted architecture proposal, grounded in `main` at
`3d2947fb7c3765785e860048968c2ada8a4419af` (merged PR #89). M10 is accepted;
the Task API, generated client, dedicated application, task-only mode, and later
schema additions remain unimplemented and separately authorization-gated.

## Recommendation

Build one canonical Task/Work Item domain inside the existing Rust modular
monolith and PostgreSQL database. Reuse the accepted M10 `work_item_commands`
mutation boundary where it applies and add a deliberate authorized read/query
boundary in TP1. Keep Agent Workspace as an integrated client, add a dedicated
task client as a second web application in this monorepo, and treat MCP, GitHub,
and future integrations as adapters without forcing integration-specific
commands into a lowest-common-denominator transport operation. A Task in the
dedicated product and a Work Item in the current product are the same record
with the same UUID, Project ownership, version, authorization, activity, and
events. There is no synchronization boundary.

Use `Task` in the dedicated product and new client-neutral API. Retain `Work
Item` in existing Agent Workspace UI, Rust modules, tables, and compatibility
routes until a separately justified rename. This is a terminology projection,
not a second entity.

## Repository and deployment decision

- Keep the Rust server and PostgreSQL schema in the current monorepo.
- Add `apps/tasks-web` only after the versioned task API and generated client
  exist.
- Add `packages/task-client` for generated types, runtime validators, and the
  transport client.
- Add `packages/task-ui` only when two clients demonstrably share behavior and
  accessibility semantics; do not extract page shells or product navigation.
- Serve the dedicated client from the same deployment in integrated mode and
  support a task-only runtime mode from the same server binary and database
  migration chain.
- Do not create a second service, database, schema, repository, event broker, or
  task copy for version one.

## Canonical ownership boundary

The canonical task domain owns task identity, project ownership, workflow,
fields, hierarchy, criteria, comments, activity, labels, relationships,
versions, command validation, authorization inputs, and task events. Agent
Workspace owns conversations, Threads, Agent Runs, Execution Workers,
repositories, and GitHub projections. Those domains attach to tasks through
explicit link records and adapters. The dedicated client owns only its routes,
navigation, presentation, local drafts, filters, and cache.

## Current-system assessment

The post-PR-#89 implementation already provides:

- Project-scoped `work_items` with UUIDv7 identity, readable project key/number,
  type, status, semantic status category, priority, assignee, rank, archive
  state, and one optimistic `version`.
- Project-owned types, statuses, and allowed transitions; the current defaults
  are Feature, Bug, Task, Request, Research, and Improvement, with Backlog
  through Canceled status semantics.
- Actor-backed authorship and assignment, centralized Project policy, opaque
  browser sessions, CSRF protection, scoped MCP/OAuth credentials, and
  external-agent credentials.
- Ordered acceptance criteria, immutable structured comments, typed activity,
  and one monotonically sequenced task timeline.
- Parent/subtask support, blocked state, Thread links, Repository links, primary
  Repository selection, explicit pull-request links, and Work-with-Worker.
- List and Kanban projections, filtering, stable cursor pagination, conflict
  responses, and server-returned per-resource capabilities.
- Browser HTTP handlers and local/external-agent MCP tools. Their overlapping
  top-level create and field/status update paths use the canonical M10 command
  types; comment append and surface-specific commands reuse shared locking,
  version, activity, audit, outbox, and external-event finalization primitives.
  Adapter-owned transaction/idempotency/wire behavior and intentionally
  surface-specific command SQL remain explicit.
- Transactional outbox invalidations for browser realtime and a separate
  connection-bound, retained polling stream for external agents.

Current gaps relevant to a dedicated product are a client-neutral versioned
contract, generated schemas/client, labels and saved views, a personal-task
entry point, task-only runtime mode, and a separate client shell. These are
roadmap items, not evidence of current implementation.

## Status vocabulary

- **Accepted baseline**: present in `PROJECT_SOURCE_OF_TRUTH.md` or current code.
- **Architecture decision**: selected by this package for later implementation;
  it is not implementation acceptance.
- **Accepted M10 boundary**: exact mutation commands and primitives are known;
  M10 added no shared query/projection layer and retained adapter-owned
  transaction lifecycle, idempotency replay, and response mapping.
- **Future option**: deliberately outside the first implementation sequence.

## Documents

| Document | Purpose |
|---|---|
| [HANDOFF.md](HANDOFF.md) | Branch/PR handoff, decision summary, dependency status, roadmap, and open decisions |
| [current-system-assessment.md](current-system-assessment.md) | Post-PR-#89 schema, API, MCP, event, frontend, deployment, and gap inventory |
| [product-spec.md](product-spec.md) | Product thesis, users, vocabulary, experience, scope, and non-goals |
| [domain-boundaries.md](domain-boundaries.md) | Ownership matrix and attachment rules between task and Agent Workspace domains |
| [system-architecture.md](system-architecture.md) | Components, data flow, repository structure, and runtime boundaries |
| [api-contract.md](api-contract.md) | Proposed versioned HTTP resources, commands, errors, concurrency, and compatibility |
| [identity-and-security.md](identity-and-security.md) | Identity, tenancy, authentication, authorization, privacy, and threat controls |
| [events-and-realtime.md](events-and-realtime.md) | Durable event taxonomy, realtime invalidation, polling, outbox, and delivery rules |
| [sdk-and-client-boundaries.md](sdk-and-client-boundaries.md) | Generated TypeScript client, validation, cache keys, and UI package boundaries |
| [deployment-topologies.md](deployment-topologies.md) | Integrated, standalone, and future hosted topologies |
| [migration-plan.md](migration-plan.md) | In-place adoption, compatibility, rollout, rollback, backup, and test plan |
| [implementation-roadmap.md](implementation-roadmap.md) | Dependency-ordered milestones, acceptance gates, and parallel tracks |
| [decision-log.md](decision-log.md) | Architecture decisions, risks, and genuinely open owner decisions |

## M10 evidence

M10 established `apps/server/src/work_item_commands.rs` with canonical create
and update command types, status-through-update, subtask-through-create, and
shared authorization, locking, conflict, version, activity, audit, outbox, and
addressed-event primitives. HTTP and MCP continue to own transaction lifecycle,
idempotency reservation/replay, wire validation, and result projection. Read
queries remain in their existing modules. The versioned Task API must call the
accepted commands where applicable, extract a shared command entry point before
adding a third adapter to a surface-specific mutation, and define authorized
TP1 projections rather than wrapping an HTTP handler or duplicating policy. See
[implementation-roadmap.md](implementation-roadmap.md) for the bounded sequence.
