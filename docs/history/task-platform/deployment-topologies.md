> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# Deployment Topologies

## Topology 1: integrated Agent Workspace deployment

This is the recommended first topology.

```text
TLS / private access
        │
        ▼
one Rust server
  ├── /api/v1              existing Agent Workspace API
  ├── /api/task/v1         versioned task API
  ├── /api/v1/realtime     authenticated WebSocket
  ├── /                    Agent Workspace web assets
  └── /tasks               dedicated task web assets
        │
        ├── PostgreSQL
        ├── outbox/jobs
        ├── MCP
        ├── Git/GitHub modules
        └── Codex provider bridge
```

Both clients use the same origin, session, Actor, Project data, command layer,
and database. The task client can deep-link to Agent Workspace connected work.
The deployment remains loopback-first/private-host compatible and exposes only
the Rust application through TLS or the accepted tunnel boundary.

### Configuration

Conceptual settings (final names follow existing config conventions):

```text
AGENT_WORKSPACE_PRODUCT_MODE=integrated
AGENT_WORKSPACE_ENABLE_TASK_CLIENT=true
AGENT_WORKSPACE_TASK_CLIENT_BASE_PATH=/tasks
```

Integrated mode requires only the dependencies already required by Agent
Workspace. The task client itself does not add provider credentials.

## Topology 2: standalone task deployment

Standalone means independently usable, not a separate codebase or data model.

```text
TLS / private access
        │
        ▼
same Rust server binary, task-only mode
  ├── /api/task/v1
  ├── /api/v1/auth and required identity/project endpoints
  ├── /api/v1/realtime or versioned equivalent
  └── /                  dedicated task web assets
        │
        ├── PostgreSQL, same migration chain
        ├── task outbox publisher
        └── task jobs only

disabled: Codex, execution, Git worktrees, repositories, GitHub, Worker routes
```

### Readiness

Task-only readiness depends on database connectivity, migrations, identity,
authorized Task read projections, canonical mutation entry points, static
assets, outbox health within accepted bounds, and required task jobs. It must
not depend on Codex auth, provider processes, repository mirrors, GitHub App
configuration, or execution profiles.

### Configuration

```text
AGENT_WORKSPACE_PRODUCT_MODE=tasks
```

Mode is an allowlisted enum, not a collection of loosely interacting booleans.
The server registers only required routes/workers and reports a capability
document. Unknown/inconsistent configuration fails startup.

### Installation

Reuse the existing self-hosted PostgreSQL, migration, backup, service-user,
systemd, and loopback listener practices. A later task-only deployment guide
may provide a reduced configuration template, but it must use the same release
artifact and rollback mechanism.

## Topology 3: future hosted multi-tenant service

This is a future boundary, not an implementation plan or accepted security
posture.

```text
edge/TLS
  │
  ├── stateless Rust application replicas
  │      ├── task API
  │      └── realtime gateway
  ├── managed PostgreSQL with tenant controls
  ├── durable job/outbox workers
  └── isolated external delivery services

agent execution, if offered, runs in separately isolated remote executors
```

Before selection, resolve O-006/O-008 and prove public authentication,
hostile-tenant isolation, abuse controls, rate/fairness, database/resource
isolation, observability, backups, privacy, data residency, incident response,
and remote execution isolation. Do not expose the current private-host
execution model to hostile tenants.

## Repository strategy comparison

| Option | Benefits | Costs/risks | Decision |
|---|---|---|---|
| Same monorepo, separate web app | Independent UX; shared schema/client/CI; one domain and release; task-only build | Adds a second app build and routing | **Selected** |
| Same web app with product modes | Minimum initial files; maximal reuse | Conditional navigation spreads; bundles/capabilities couple; standalone UX becomes a skin | Rejected initially |
| Separate repository using API/SDK | Independent release/ownership | Premature contract publishing, duplicated tooling, cross-repo coordination | Future extraction only |

## Static assets and routing

- Build both web apps independently with distinct asset manifests.
- Integrated mode mounts dedicated assets under a stable base path; client-side
  routing falls back only within that path.
- Task-only mode mounts the dedicated app at `/`.
- API, auth, realtime, health, and static routes are reserved explicitly so SPA
  fallback never masks an API `404`.
- Content Security Policy and cache headers are generated per app. Hashed assets
  are immutable; HTML/config discovery is not.

## Database and migrations

Both modes run the same ordered migration chain. A deployment cannot use a
task-only fork of schema history. Integrated-only tables may remain present in
task-only databases; route/background initialization determines behavior.

Migrations are forward-compatible with rollback to the immediately prior
application during staged rollout. Destructive cleanup occurs only after the
compatibility window and verified backup/restore.

## CI matrix

| Gate | Integrated | Task-only |
|---|---:|---:|
| Rust fmt/clippy/unit | Yes | Yes |
| Real PostgreSQL migration tests | Yes | Yes |
| Agent Workspace web checks/build | Yes | Compatibility |
| Task web checks/build | Yes | Yes |
| API schema generation/diff | Yes | Yes |
| Cross-client same-ID E2E | Yes | — |
| Core task E2E without integrations | Yes | Yes |
| Provider/GitHub acceptance | Existing milestone gates | Must be absent |

## Independent usability acceptance

Start with a clean database and no Git/Codex/GitHub configuration. Bootstrap an
owner, sign in, create/capture Tasks, complete core collaboration and board
flows, restart, back up, restore, and upgrade using task-only mode. Network and
process inspection must show only expected server/PostgreSQL boundaries and no
provider or GitHub workers.

## Future extraction threshold

Consider a separate repository only when the versioned API and SDK have at
least one external consumer, independent releases are operationally necessary,
and ownership justifies cross-repository compatibility cost. Consider a
separate task service only after measured scaling/security/availability needs
and a no-dual-write data extraction plan. Client extraction does not imply
service or database extraction.
