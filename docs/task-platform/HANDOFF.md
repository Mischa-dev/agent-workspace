# Task Platform Architecture Handoff

## Recommended architecture

Use one canonical Task/Work Item domain in the existing Rust modular monolith
and PostgreSQL database. Every transport delegates to the accepted M10
application command/query layer. Agent Workspace remains the integrated client;
a separate task-first web application becomes another client; MCP, GitHub, and
future integrations remain adapters. Dedicated Tasks and Agent Workspace Work
Items share the same UUID, Project, version, policy, activity, and events. There
is no synchronization system.

## Repository and deployment recommendation

- Same monorepo, separate future `apps/tasks-web` application
- Same Rust server binary, M10 layer, database, and migration chain
- New versioned `/api/task/v1` contract
- Generated strict `packages/task-client`; proven UI primitives may later enter
  `packages/task-ui`
- Integrated mode serves both clients; task-only mode disables Agent Workspace
  execution/Git/GitHub routes, workers, and readiness dependencies
- No new repository, service, schema, database, broker, or production code in
  this architecture PR

## Canonical ownership boundary

The task domain owns Task identity, Project ownership, fields, workflow,
criteria, hierarchy, comments, activity, labels, relationships, versions,
commands, and Task events. Shared identity/Project/policy infrastructure owns
Actors, Organizations, memberships, authentication, and authorization. Agent
Workspace owns Conversations, Threads, Agent Runs, Execution Workers,
repositories, GitHub projections, and provider state; these attach through
explicit links. Clients own only presentation, navigation, drafts, and cache.

## Decisions made

- **Product:** solo-first dedicated Tasks with Inbox/My Tasks/Projects; no
  requirement for execution or GitHub.
- **Domain:** one existing `work_items` identity; Task is client/API terminology,
  not another entity or table.
- **API:** client-neutral `/api/task/v1`, typed commands, strict contract,
  opaque cursors, expected versions, idempotency, structured errors, and
  compatibility routes.
- **Identity:** server-resolved Actor authorship and existing Organization/
  Project membership policy; Inbox uses a private Personal Project.
- **Events:** typed Task facts, PostgreSQL transactional outbox, browser
  invalidation/refetch, connection-bound external polling, at-least-once
  delivery and deduplication; no v1 broker.
- **SDK:** OpenAPI 3.1 source, deterministic generated TypeScript bindings and
  runtime validators; shared UI only after two proven implementations.
- **Deployment:** same monorepo/server/database with integrated and task-only
  modes; public hosted multi-tenancy remains future work.

## Documents

- `PROJECT_SOURCE_OF_TRUTH.md`: v0.31 revision and canonical Decision D-058.
- `docs/task-platform/README.md`: recommendation, status boundary, current
  summary, and document index.
- `docs/task-platform/current-system-assessment.md`: post-PR-#86 schema, HTTP,
  MCP, event, frontend, deployment, and gap evidence.
- `docs/task-platform/product-spec.md`: users, vocabulary, navigation, feature
  scope, personal work, agents, imports/exports, non-goals, and acceptance.
- `docs/task-platform/domain-boundaries.md`: ownership matrix, link/adaptor
  rules, server-only data, and invariants.
- `docs/task-platform/system-architecture.md`: components, data/write/read flow,
  repository structure, CI, extraction threshold, and recovery.
- `docs/task-platform/api-contract.md`: proposed resources, actions, batch,
  pagination, conflicts, idempotency, errors, validation, and deprecation.
- `docs/task-platform/identity-and-security.md`: authentication, tenancy,
  personal spaces, authorization, tokens, non-disclosure, and mode isolation.
- `docs/task-platform/events-and-realtime.md`: taxonomy, envelopes, browser
  invalidation, polling, webhook direction, privacy, and notifications.
- `docs/task-platform/sdk-and-client-boundaries.md`: generated SDK, validators,
  query keys, cache rules, UI extraction, versioning, and tests.
- `docs/task-platform/deployment-topologies.md`: integrated, standalone, and
  future hosted topologies plus repository/build decisions.
- `docs/task-platform/migration-plan.md`: in-place adoption, IDs/URLs/history,
  rollout, rollback, backup/restore, and real-PostgreSQL gates.
- `docs/task-platform/implementation-roadmap.md`: M10 evidence gate, TP1–TP8,
  safe parallel tracks, tests, exits, and first implementation PR.
- `docs/task-platform/decision-log.md`: TP-D001–TP-D012, risk review, and only
  genuinely unresolved owner decisions.
- `docs/task-platform/HANDOFF.md`: this cold-reader delivery summary.

## Branch, commit, and PR

- Branch: `codex/task-platform-architecture`
- Architecture commit: `fb752918673f5c866efc49651fb0d756348ae691`
- Pull request: <https://github.com/Mischa-dev/agent-workspace/pull/87>
- PR state at handoff: draft documentation PR

## Dependency status

### Assumptions about M10

M10 is assumed to consolidate existing Work Item browser and MCP behavior into
one transport-independent command/query layer with central transaction,
authorization, idempotency, optimistic concurrency, activity, audit, outbox,
and addressed external-event effects. This PR does not claim M10 exists or has
been accepted.

### Decisions that wait for M10 evidence

- exact Rust module, command, query, and DTO names
- child-resource locking granularity
- whether comment append participates in the Task version or remains an
  independently idempotent append command
- exact internal typed event-fact representation
- which current projections are reused versus remapped

These details do not reopen one canonical Task identity, Project ownership,
Actor authorship, one database, or M10 reuse.

## Implementation roadmap

Ordered sequence:

1. Accept M10 command/query evidence.
2. TP1: read-only versioned Task contract and parity-tested API.
3. TP2: generated TypeScript client and strict runtime validation.
4. TP3: typed Task command API over M10.
5. TP4: dedicated shell and independently usable core flows.
6. TP5: Personal Project marker, labels, relationships, saved views, and
   justified batch operations.
7. TP6: event/notification/integration hardening.
8. TP7: task-only deployment mode.
9. TP8: retained-data migration, compatibility, restore, rollback, and release
   acceptance.

Safe parallel tracks are contract examples plus UX exploration, task shell plus
TP3 wiring after schema freeze, and deployment inventory plus core API work.
Migration numbers, command semantics, and the source contract require one
integrating owner.

### First implementation PR after M10

Ship TP1 read-only contract foundation only: OpenAPI source, deterministic
generated types/validators, `/api/task/v1` discovery/capabilities, authorized
Task detail and Project list reads, and parity/contract tests. No writes, schema
migration, new app, or behavior change.

## Open decisions

Only four owner decisions remain:

1. Public product name and permanent URL vocabulary. Default: internal `Task
   Platform`, UI `Tasks`, integrated path `/tasks`.
2. Third-party artifact packaging. Default: one binary/image with allowlisted
   mode and two web asset builds.
3. Whether/when to publish and support an external SDK. Default: keep private
   through TP4.
4. Whether to pursue public hosted multi-tenancy. Default: remain private/
   self-hosted until O-006/O-008 and a new threat model are resolved.

## Verification

- `git diff --check`: passed
- `cargo fmt --check`: passed
- `corepack pnpm check`: passed
- frontend tests: 28 files, 125 tests passed
- frontend production build: passed
- local Markdown link check: passed
