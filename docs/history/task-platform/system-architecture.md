> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# System Architecture

## Selected architecture

Extend the existing Rust modular monolith. Accepted M10 supplies a
transport-independent mutation boundary for canonical create/update operations
and shared authorization, locking, conflict, version, activity, audit, outbox,
and addressed-event primitives. Existing Agent Workspace HTTP and MCP adapters
use that boundary where their semantics overlap. The new versioned Task API must
reuse it and first extract a canonical command entry point for any still
surface-owned family. TP1 adds a deliberate authorized query/projection
boundary; M10 supplied no shared queries. GitHub automation, jobs, and other
integration-specific commands remain explicit domain adapters. One PostgreSQL
schema stores canonical records and the transactional outbox. Two React
applications consume the server through a generated client, but neither owns
domain state.

```text
Agent Workspace web ─┐
Dedicated task web ──┼──► HTTP adapters ───────────────┐
MCP / external agent ┘                                │
                                                      ▼
                                      authorized read projections (TP1)
                                      accepted M10 mutation boundary
                                                      │
GitHub / jobs ───────► explicit linked-domain commands│
                                                      ▼
                         canonical task records + identity/policy
                                                      │
                                                      ▼
                                  PostgreSQL + transactional outbox
                                                      │
                                  ┌───────────────────┴──────────────┐
                                  ▼                                  ▼
                        browser invalidation                external event adapter
```

## Component responsibilities

### M10 application layer

- Defines `CreateWorkItemCommand` and `UpdateWorkItemCommand` inputs/outcomes
  independent of HTTP or MCP, including status-through-update and
  subtask-through-create.
- Provides shared membership authorization, Work Item lock/conflict, workflow,
  assignee, version, activity/timeline, audit, outbox, and addressed-event
  primitives.
- Applies canonical create/update SQL inside a transaction supplied by the
  caller and returns transport-neutral outcomes.
- Does not own transaction begin/commit, idempotency reservation/replay, wire
  validation, public response projection, or reads.
- Leaves board ranking, archive/restore, criteria, links, blocker/review,
  GitHub, and sponsored execution as explicit command families that reuse
  primitives without pretending to be one common transport operation.

The new API must not call existing HTTP handlers, and MCP does not call the API
over loopback. Adapters call the same in-process command for genuinely shared
operations; surface-specific operations remain explicit until a canonical
entry point is extracted for another adapter.

### Canonical task domain

M10 established `apps/server/src/work_item_commands.rs` beside the existing
`work_items.rs` and `mcp/` adapters without changing stored identity. Keep the
server as one crate until boundaries are proven. TP1 may add explicit query and
projection modules only as the new contract demonstrates the seam.

Possible later internal shape after TP1/TP3 evidence:

```text
apps/server/src/tasks/
  mod.rs
  commands.rs
  queries.rs
  model.rs
  policy.rs
  projections.rs
  events.rs
  links.rs
```

This is a target, not authority for a mechanical file split. Preserve accepted
`work_item_commands.rs`, caller-owned transaction/idempotency lifecycle, and
transport response ownership unless later evidence justifies a replacement.

### HTTP adapters

- Existing `/api/v1/.../work-items` routes remain compatibility adapters.
- New `/api/task/v1/.../tasks` routes form the client-neutral contract.
- Both map request authentication, headers, and DTOs into the same command only
  where a canonical command entry point exists.
- Responses use contract projections rather than serializing SQL rows.

### MCP adapter

MCP keeps tool discovery, annotations, Project-bound credentials, OAuth, MCP
cursor encoding, and tool-specific result envelopes. Task reads and writes map
to authorized read projections and accepted commands where applicable. Existing
MCP reads remain in `mcp/reads.rs` until TP1 establishes a shared boundary.
Equivalent operations must converge on canonical state/effects, while MCP-only
subtask, pull-request, blocker/review, and sponsorship semantics retain their
intentional contracts.

### Event adapters

Current commands write typed activity plus coarse `work_item.updated` browser
outbox invalidation and separate recipient-bound external-agent records. M10 did
not add a generic internal Task-event-fact model. TP6 may introduce a granular
canonical taxonomy and privacy-bounded projections for later notifications or
webhooks. Each adapter keeps its own delivery cursor and deduplication identity;
none becomes the source of domain truth.

### Web clients

`apps/web` remains Agent Workspace. `apps/tasks-web` is the dedicated client.
Both use `packages/task-client`. Each owns its shell, routing, navigation, and
product composition. Shared task UI is extracted only after equivalent
behavior exists in both.

## Data ownership

PostgreSQL remains authoritative. Existing Task/Work Item tables are adopted in
place. New task features use additive migrations in the same migration chain.
No `task_platform` schema is introduced; schema namespaces are not a security
or service boundary.

Transactions lock the aggregate root for compound changes where needed. The
Task version is the user-visible optimistic token. Child criterion or
relationship commands participate in the parent version. Current criteria lock
and advance the parent; criterion update also locks the criterion row. Subtask
create locks its parent but creates a child at version 1 without advancing the
parent. Immutable comment append locks the Work Item, uses adapter-owned
idempotency and timeline sequence allocation, and keeps the current Task version.

## Read architecture

Use direct, authorized PostgreSQL queries inside the modular monolith. Do not
add Elasticsearch or a read-model service for v1.

- Detail projections include canonical Task fields and optionally expanded
  links requested through `include`.
- List/search projections are bounded, cursor-paginated, and indexed.
- Board is a grouped projection over the same Tasks and workflow.
- Global `My Tasks` queries join through active Organization/Project
  memberships and apply non-disclosing policy.
- Realtime causes invalidation/refetch; it does not patch an event-sourced
  client model.

Add PostgreSQL full-text or trigram search only after query plans show the
current bounded title/key search is inadequate. Do not add search
infrastructure preemptively.

## Write architecture

The accepted adapter-plus-command flow follows this order:

1. Resolve the authenticated Actor and fixed tenant/resource identifiers.
2. Validate syntax and bounded payload sizes.
3. The adapter begins a database transaction.
4. Load active Organization/Project membership and authorize the typed action
   through shared policy primitives.
5. The adapter reserves transport-scoped idempotency where applicable.
6. The canonical command/helper locks and loads the aggregate and compares
   `expectedVersion` where the operation is versioned.
7. Validate workflow and cross-record invariants.
8. Apply the mutation. Versioned changes advance once; create starts at version
   1; comment append retains the current version.
9. Append the operation's accepted activity/timeline, audit, outbox, and
   addressed external-event effects. Comment append adds a comment timeline
   entry rather than an activity row.
10. The adapter completes idempotency with its response and commits.

External calls never occur inside a Task command. GitHub actions and
notifications are driven by durable jobs/outbox after commit.

## Runtime capability model

The server computes a deployment capability document from configuration and
initialized modules, then intersects it with Actor/resource policy:

```json
{
  "taskApiVersion": "1.0",
  "features": {
    "tasks": true,
    "labels": false,
    "savedViews": false,
    "agentWorkspaceLinks": true,
    "workerInvocation": true,
    "githubLinks": true,
    "imports": false,
    "exports": true
  }
}
```

Capabilities are hints for rendering and progressive enhancement. The server
still authorizes every action. Task-only mode reports integrated features as
false and does not initialize provider, worktree, repository, or GitHub jobs.

## Repository structure

Initial target:

```text
apps/
  server/                 Rust modular monolith and all APIs
  web/                    Agent Workspace client
  tasks-web/              dedicated task client
packages/
  task-contract/          source API schema and generated artifacts
  task-client/            generated TS client + runtime validation wrappers
  task-ui/                proven shared task primitives only
docs/task-platform/       this architecture package
```

`task-contract` may be schema files rather than a published npm package at
first. Choose OpenAPI 3.1 as the HTTP source artifact and generate JSON Schema
components/runtime validators from it. The Rust server must test response
conformance; TypeScript is not the source of wire truth.

## Build and CI

- Root install remains one pnpm workspace install.
- Add independent lint, typecheck, unit test, and build scripts per web app.
- Generate the task client deterministically and fail CI on a dirty generated
  diff.
- Run Rust contract tests that execute real handlers and validate responses.
- Run migration tests from pre-task-platform fixtures against real PostgreSQL.
- Build integrated and task-only assets/configurations.
- Add one cross-client E2E proving the same ID/version after mutation.

Do not require every client to publish a package release. In-repository clients
consume the workspace version; compatibility is enforced by schema diff and
contract tests.

## Extraction threshold

Keep one monorepo and server until at least two of these are true and measured:

- independent server scaling is required by sustained load
- separate security/operational ownership exists
- independent release cadence cannot be supported through the versioned API
- non-Rust consumers require a separately published SDK with external support
- task-only deployments need materially different availability or data
  residency from Agent Workspace
- the task module can run against contract tests without importing Agent
  Workspace execution modules

Even then, extract behind the versioned API and outbox contract. Do not split
the database first or introduce dual writes.

## Failure and recovery

- A committed Task mutation remains correct if realtime publication fails; the
  outbox publisher retries and clients refetch.
- A client disconnect loses only ephemeral invalidations.
- Idempotent command replay returns the stored canonical result.
- A version conflict returns current state and never partially applies.
- Task-only startup fails readiness if PostgreSQL/migrations are unavailable,
  but does not require Codex, Git, GitHub, or provider credentials.
- Integrated link expansion failure returns the canonical Task plus bounded
  unavailable link metadata where safe; it never substitutes stale copied Task
  data.

## Architecture acceptance tests

1. Equivalent browser, MCP, and future Task API operations reach the same
   canonical command implementation; surface-specific operations are not given
   artificial parity requirements.
2. A state-changing versioned command advances once and emits its accepted
   activity, audit, outbox, and addressed-event effects. Create starts at
   version 1. Comment append keeps the Task version and emits one comment
   timeline entry plus audit/outbox/addressed-event effects. No-op and replay
   behavior preserves the existing transport contract without duplicate domain
   effects.
3. Idempotent replay creates no duplicate child record or event.
4. Conflicting clients receive current version/state with zero partial writes.
5. Task-only mode starts and completes core flows with provider/GitHub config
   absent.
6. Integrated and dedicated clients read the same UUID/version immediately
   after ordinary refetch.
