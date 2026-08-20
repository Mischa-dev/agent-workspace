# System Architecture

## Selected architecture

Extend the existing Rust modular monolith. M10 supplies transport-independent
task commands and queries. Existing Agent Workspace HTTP handlers, the new
versioned task API, MCP, GitHub automation, and internal jobs all adapt to that
layer. One PostgreSQL schema stores the canonical records and transactional
outbox. Two React applications consume the server through a generated client,
but neither owns domain state.

```text
Agent Workspace web ─┐
Dedicated task web ──┼──► versioned HTTP adapters ─┐
MCP / external agent ┤                            │
GitHub / jobs ───────┘                            ▼
                                             M10 application layer
                                      commands + queries + authorization
                                                   │
                         ┌─────────────────────────┼──────────────────────┐
                         ▼                         ▼                      ▼
                 canonical task domain      linked AW modules     identity/policy
                         │                         │                      │
                         └─────────────────────────┼──────────────────────┘
                                                   ▼
                                  PostgreSQL + transactional outbox
                                                   │
                                  ┌────────────────┴───────────────┐
                                  ▼                                ▼
                        browser invalidation              external event adapter
```

## Component responsibilities

### M10 application layer

- Defines command/query input types independent of HTTP or MCP.
- Resolves and revalidates Actor, Organization, Project, and resource scope.
- Opens transactions and applies typed domain operations.
- Enforces expected versions and idempotency before side effects.
- Writes activity, audit, integration invalidation, and outbox facts in the
  source transaction.
- Returns canonical result types plus machine-readable conflicts.

The API does not call existing HTTP handlers, and MCP does not call the API over
loopback. All transports call the same in-process application layer.

### Canonical task domain

Initially refactor the existing `apps/server/src/work_items.rs` behavior into
explicit command, query, policy, and projection modules without changing stored
identity. Keep the server as one crate until boundaries are proven.

Recommended internal shape after M10 evidence:

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

This is a target, not authority for a mechanical file split. M10 may establish
different names; preserve its accepted transaction and command ownership.

### HTTP adapters

- Existing `/api/v1/.../work-items` routes remain compatibility adapters.
- New `/api/task/v1/.../tasks` routes form the client-neutral contract.
- Both map request authentication, headers, and DTOs into the same commands.
- Responses use contract projections rather than serializing SQL rows.

### MCP adapter

MCP keeps tool discovery, annotations, Project-bound credentials, OAuth, MCP
cursor encoding, and tool-specific result envelopes. Task reads and writes map
to application queries/commands. Tool behavior must not diverge from HTTP.

### Event adapters

The canonical domain emits internal typed event facts. The outbox projects
minimal invalidation envelopes for browsers. The external-agent adapter creates
recipient-bound polling records. A later webhook adapter may project a
privacy-bounded public event. Each adapter has its own delivery cursor and
deduplication identity; none becomes the source of domain truth.

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
relationship commands participate in the parent version unless M10 evidence
shows a commutative append can remain independent. Immutable comment append
uses idempotency and timeline sequence allocation.

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

Every command follows this order:

1. Resolve the authenticated Actor and fixed tenant/resource identifiers.
2. Validate syntax and bounded payload sizes.
3. Begin a database transaction.
4. Load active Organization/Project membership and authorize the typed action.
5. Reserve idempotency where applicable.
6. Lock/load the current aggregate and compare `expectedVersion`.
7. Validate workflow and cross-record invariants.
8. Apply the mutation and increment the Task version once.
9. Append Task activity/audit and typed event/outbox facts.
10. Complete idempotency with the canonical response and commit.

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

1. Browser, MCP, and integration calls reach the same M10 command implementation.
2. One mutation produces one version increment, one activity fact, one audit
   fact, and one canonical task outbox fact.
3. Idempotent replay creates no duplicate child record or event.
4. Conflicting clients receive current version/state with zero partial writes.
5. Task-only mode starts and completes core flows with provider/GitHub config
   absent.
6. Integrated and dedicated clients read the same UUID/version immediately
   after ordinary refetch.
