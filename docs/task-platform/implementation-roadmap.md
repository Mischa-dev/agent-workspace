# Implementation Roadmap

This backlog is dependency-ordered. Milestone names here are task-platform
planning names, not claims that repository M10 or later milestones are accepted.

## Dependency graph

```text
M10 accepted command layer
        │
        ▼
TP1 contract + read API
        │
        ├────────► TP2 generated client
        │                    │
        ▼                    ▼
TP3 write API ───────► TP4 dedicated shell/core flows
        │                    │
        ├────────► TP5 labels/relationships/views
        │                    │
        └────────► TP6 events/integrations hardening
                             │
                             ▼
                   TP7 standalone deployment
                             │
                             ▼
                   TP8 migration/acceptance
```

## M10 dependency gate

### Assumed scope

M10 provides transport-independent application commands/queries for existing
Work Item behavior, central transaction ownership, server-resolved Actor,
centralized policy, optimistic versioning, idempotency, activity/audit, outbox,
and addressed external-agent event effects. Browser and MCP adapters use it.

### Required evidence

- documented command/query boundaries and DTO ownership
- browser/MCP effect-parity tests for create, update, transition, criterion,
  comment, subtask, blocked/review, links, archive/restore, and conflicts
- exactly-once domain effect under idempotent replay
- source-transaction activity/audit/outbox/external-event behavior
- no transport SQL for migrated command families
- real PostgreSQL concurrency tests

### Decisions that wait for evidence

- exact Rust module and command type names
- whether comment append is an aggregate-version or append-only idempotent
  command in the canonical layer
- exact internal event-fact representation
- child-resource locking granularity
- which current response projections can be shared versus remapped

These details do not reopen the one-engine, one-database, one-Actor decision.

## TP1: versioned contract and read API

### Scope

Create the OpenAPI 3.1 source, discovery/capabilities, Task detail/list/search,
workflow, assignee, board, criteria, and timeline reads over current data.

### Non-goals

Writes, new schema, task client shell, labels, saved views, imports, webhooks.

### Tasks

- add contract package and deterministic generation tooling
- define strict schemas, examples, errors, pagination, unknown-enum behavior
- add `/api/task/v1` router and read adapters to M10 queries
- add authorized global My Tasks projection
- add schema conformance and compatibility projection tests
- document contract revision/deprecation metadata

### Tests

Rust handler/schema tests, real PostgreSQL Project-isolation/pagination tests,
malformed cursor/non-disclosure tests, generated-diff check, representative old
record fixtures.

### Exit criteria

Every read returns the same canonical ID/version as the compatibility surface;
all responses validate; no unauthorized count/existence leak; CI is green.

### Parallelization

Schema/examples and authorized query planning can proceed in parallel after M10
query ownership is known. One owner resolves final wire naming.

## TP2: shared TypeScript client

### Scope

Generate strict types/runtime validators/transport functions and provide query
key helpers plus error normalization.

### Non-goals

Shared page UI, product shell, auth token storage, task mutations before TP3.

### Tasks

- pin generator/runtime validator versions
- add same-origin cookie/CSRF transport hook
- implement discovery/version negotiation and errors
- define canonical filter serialization/query keys
- add fixture, malformed response, and compatibility tests
- migrate Agent Workspace read paths incrementally where safe

### Tests and exit criteria

Reproducible generation; zero permissive defaulting of required/security fields;
query keys isolate Projects/filters; Agent Workspace read behavior remains green.

### Parallelization

Begins once TP1 schema components stabilize; UI prototyping may consume mocked
generated types but cannot claim integration.

## TP3: command API

### Scope

Expose create, update, transition, board move, archive/restore, criteria,
comments, subtasks, and blocked-state commands through `/api/task/v1`.

### Non-goals

Arbitrary PATCH, bulk, labels, import/export, Agent Workspace execution flows.

### Tasks

- map HTTP authentication/CSRF/idempotency into M10 commands
- produce stable conflict/error/retry envelopes
- add per-resource capabilities and transition targets
- prove compatibility-route and MCP effect parity
- add metrics by command/result without content logging

### Tests and exit criteria

Real PostgreSQL concurrent edits, idempotent replays/payload mismatch, revoked
principal, role matrix, invalid transition, atomic activity/audit/outbox, and
same result effects across transports. No handler-specific SQL.

## TP4: dedicated application shell and core flows

### Scope

Add `apps/tasks-web` with sign-in/session boundary, Inbox, My Tasks, Project
navigation, list, board, Task detail/edit, criteria, subtasks, comments,
timeline, filters, conflict UI, archive/restore, and capability-disabled states.

### Non-goals

Execution UI, repository settings, GitHub settings, shared product shell,
offline mutations, saved views, labels if TP5 is incomplete.

### Tasks

- build independent responsive shell and routing
- consume only `task-client`
- implement keyboard-efficient capture and field editing
- implement invalidation/refetch and reconnect convergence
- deep-link connected work when capability exists
- meet WCAG 2.2 AA and performance budgets

### Tests and exit criteria

Unit/component accessibility tests, mocked contract failures, real browser core
flow, deliberate two-client conflict, reconnect, cross-client same-ID/version,
and task-only-capability rendering. App is useful without integrated features.

### Parallelization

Shell/design system and test harness can proceed against TP1 fixtures while TP3
lands. Real mutation wiring waits for TP3.

## TP5: canonical task features

### Scope

Add Personal Project marker/Inbox provisioning, labels, Task relationships,
validated saved views, and justified batch actions.

### Non-goals

Custom fields, cross-Project relations/moves, cycles/roadmaps, recurrence,
arbitrary bulk patch.

### Tasks

- design additive migrations and M10 commands per feature
- add API/schema/client/UI vertically one feature at a time
- add activity/audit/events and policy actions
- index/query-plan filtered views
- add import/export only after core feature acceptance

### Tests and exit criteria

Upgrade/rollback, scope/cycle constraints, concurrent attach/detach, filter
correctness, batch per-item outcomes, and stable old-client behavior.

### Parallelization

Labels and Personal Project marker are independent migrations if serialized by
one migration owner. Relationships depend on invariant review. Saved views
depend on final filter schema.

## TP6: events and integration hardening

### Scope

Canonical Task taxonomy, browser invalidation mapping, external-agent event
coverage, notification preferences/intents, and webhook design spike.

### Non-goals

New broker, guaranteed WebSocket replay, public webhook launch without threat
review, notification email provider by default.

### Tasks

- map internal task facts to bounded delivery envelopes
- add duplicate/lag/revocation/privacy tests
- expand addressed external polling where useful
- add outbox health metrics and operator diagnostics
- define notification preference schema and in-app intent projection

### Exit criteria

Crash/retry produces at-least-once delivery and no duplicate domain effects;
reconnect converges; revoked recipients stop; payload limits/privacy pass.

## TP7: standalone deployment mode

### Scope

Serve the dedicated client at root from the same binary with task-only route,
worker, health, config, documentation, CI, backup, upgrade, and rollback paths.

### Non-goals

Separate service/database/repository, public SaaS, remote executors.

### Tasks

- implement allowlisted product mode and capability computation
- gate routes/background workers and remove integrated readiness dependencies
- add task-only asset manifest and service configuration
- write reduced self-hosted guide and acceptance script
- prove no provider/GitHub/Git processes/listeners initialize

### Exit criteria

Clean-host task core flow, restart, backup/restore, upgrade/rollback, resource
budget, and negative process/listener/config evidence all pass.

## TP8: migration, compatibility, and release acceptance

### Scope

Run the in-place migration plan, old/new surface parity, cross-client E2E,
documentation, and operator handoff against an exact green commit.

### Non-goals

Removing compatibility APIs, public launch, billing, native apps.

### Tasks

- capture/restore pre-migration backup and inventory
- apply migrations to representative retained data
- validate counts, IDs, keys, versions, timelines, actors, links, and URLs
- run integrated and standalone acceptance
- exercise rollback with prior compatible binary
- publish exact limitations and remaining decisions

### Exit criteria

Zero Task copies, no lost history, accepted CI and real PostgreSQL evidence,
working old API/MCP, exact same IDs across clients, verified restore/rollback,
and task-only independence.

## Safe parallel tracks

- Contract prose/examples and UX exploration can run while M10 is implemented,
  but final mapping waits for evidence.
- Task client shell/accessibility work can use generated fixtures after TP1
  schema freeze while TP3 writes are built.
- Deployment mode inventory can begin read-only while core API/client work lands.
- Labels and relationship data designs can be reviewed in parallel but should
  ship as separate vertical PRs with serialized migrations.
- Documentation/acceptance harness can evolve throughout.

Do not parallelize competing changes to canonical command semantics, migration
numbers, or the contract source without a single owning integrator.

## First implementation PR after M10

Ship **TP1 read-only contract foundation**:

- `packages/task-contract/openapi.yaml`
- deterministic generated TypeScript types/validators
- `/api/task/v1` discovery and capabilities
- authorized Task detail plus bounded Project Task list reads
- contract/parity tests against existing Work Items
- no writes, schema migration, new app, or behavior change

This is the smallest production-shaped proof that the dedicated product can
consume the same canonical model through a stable client-neutral boundary.
