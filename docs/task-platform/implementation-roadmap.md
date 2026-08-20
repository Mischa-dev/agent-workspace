# Implementation Roadmap

This backlog is dependency-ordered. Repository M10 is accepted. TP1 through TP8
are planning names, not implementation authorization or acceptance.

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

## Accepted M10 baseline

PR #89 established `apps/server/src/work_item_commands.rs` with
`CreateWorkItemCommand`, `UpdateWorkItemCommand`, canonical create/update,
status-through-update, subtask-through-create, and shared authorization, lock,
conflict, version, activity/timeline, audit, outbox, and addressed-event
primitives. Real-PostgreSQL parity proves equivalent browser/MCP top-level
create and common-field update reach equal canonical row state and each
state-changing call adds one activity, audit, and outbox row.

The accepted boundary is intentionally narrower than the earlier assumption:

- M10 provides no shared queries or read DTOs.
- adapters begin/commit transactions, reserve/complete idempotency, validate
  wires, and project their public results
- comment append retains the current Task version, adds one comment timeline
  entry rather than activity, and shares only finalization; structured browser
  content and MCP bounded text remain distinct
- subtask create locks the parent and creates child version 1 without advancing
  the parent; acceptance criteria lock and advance their parent
- board move, archive/restore, criteria, Thread/Repository links, MCP pull-
  request/blocker/review commands, GitHub automation, and sponsorship remain
  explicit command families that reuse primitives where applicable
- M10 retained coarse `work_item.updated` outbox invalidation and addressed
  external events rather than introducing a generic Task-event-fact model

Future parity is scoped to equivalent operations. Top-level create, common
field update, and status transitions can compare canonical state/effects.
Comment tests compare accepted side effects and plain-text projection while
preserving transport content differences. HTTP-only and MCP-only commands gain
cross-surface parity only if a counterpart is later added. Conflict semantics
converge, but public envelopes remain transport-specific.

## TP1: versioned contract and read API

### Scope

Create the OpenAPI 3.1 source, discovery/capabilities, Task detail/list/search,
workflow, assignee, board, criteria, and timeline reads over current data.

### Non-goals

Writes, new schema, task client shell, labels, saved views, imports, webhooks.

### Tasks

- add contract package and deterministic generation tooling
- define strict schemas, examples, errors, pagination, unknown-enum behavior
- add `/api/task/v1` router plus deliberate authorized query/projection modules
  over existing canonical rows; do not wrap an HTTP handler
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

Schema/examples and authorized query ownership may be designed in parallel
inside TP1. One owner resolves final wire naming and projection ownership.

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
comments, subtasks, and explicit block/unblock commands through `/api/task/v1`.

### Non-goals

Arbitrary PATCH, bulk, labels, import/export, Agent Workspace execution flows.

### Tasks

- map create, update, transition, and subtask routes into accepted M10 commands
- extract or add canonical command entry points for board move,
  archive/restore, criteria, structured comment append, and block/unblock before
  the Task API becomes another adapter for each family
- keep HTTP authentication, CSRF, transaction, idempotency reservation/replay,
  and response projection in the adapter
- produce stable conflict/error/retry envelopes
- add per-resource capabilities and transition targets
- prove compatibility-route and MCP effect parity only where operations are
  genuinely equivalent; retain documented surface-specific distinctions
- add metrics by command/result without content logging

### Tests and exit criteria

Real PostgreSQL concurrent edits, idempotent replays/payload mismatch, revoked
principal, role matrix, invalid transition, atomic accepted side effects, and
same canonical effects across equivalent transports. New Task API handlers own
no duplicated mutation SQL; each invokes a canonical command entry point.

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

- design additive migrations and canonical application commands per feature
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

- Contract prose/examples and UX exploration may use the accepted M10 evidence;
  final read ownership and wire mapping are TP1 decisions.
- Task client shell/accessibility work can use generated fixtures after TP1
  schema freeze while TP3 writes are built.
- Deployment mode inventory can begin read-only while core API/client work lands.
- Labels and relationship data designs can be reviewed in parallel but should
  ship as separate vertical PRs with serialized migrations.
- Documentation/acceptance harness can evolve throughout.

Do not parallelize competing changes to canonical command semantics, migration
numbers, or the contract source without a single owning integrator.

## Recommended first implementation PR after M10

Ship **TP1 read-only contract foundation**:

- `packages/task-contract/openapi.yaml`
- deterministic generated TypeScript types/validators
- `/api/task/v1` discovery and capabilities
- authorized Task detail plus bounded Project Task list reads
- contract/parity tests against existing Work Items
- no writes, schema migration, new app, or behavior change

This is the smallest production-shaped proof that the dedicated product can
consume the same canonical model through a stable client-neutral boundary. It
remains unimplemented and requires explicit owner authorization before work
begins.
