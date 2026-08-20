# Task Platform Decision and Risk Log

These decisions define the architecture proposal. They do not declare M10 or
task-platform implementation accepted. Existing decisions cited from
`PROJECT_SOURCE_OF_TRUTH.md` remain canonical repository decisions.

## TP-D001: One canonical task system

**Status:** Architecture decision

**Decision:** Dedicated Tasks and Agent Workspace Work Items are one record,
command model, authorization model, and event model. No copy or synchronization.

**Reason:** Dual systems create identity mapping, lag, conflict, permission
drift, duplicated activity, failure recovery, and irreversible migration cost.
Different clients do not justify different truth.

## TP-D002: Same monorepo, separate dedicated web application

**Status:** Architecture decision

**Decision:** Add `apps/tasks-web` beside `apps/web`, backed by the same Rust
server. Do not put pervasive product-mode conditionals into one React shell and
do not create a separate repository initially.

**Reason:** Separate shells preserve genuine independent use and product focus;
the monorepo preserves atomic contract/client/server changes and low operating
complexity.

## TP-D003: Existing schema is adopted in place

**Status:** Architecture decision

**Decision:** `work_items` remains the canonical storage table. `Task` is new
product/API terminology for the same UUID. Additive features extend the current
migration chain.

**Reason:** A table rename provides little user value and raises migration/
rollback risk. A second `tasks` table would violate TP-D001.

## TP-D004: M10 is the sole application command/query boundary

**Status:** M10-dependent architecture decision

**Decision:** New HTTP, compatibility HTTP, MCP, GitHub automation, and jobs
delegate in-process to M10. No adapter owns domain SQL or calls another adapter.

**Reason:** Transport parity, atomic effects, authorization, idempotency, and
audit cannot remain reliable with duplicated command implementations.

## TP-D005: Client-neutral versioned Task API

**Status:** Architecture decision

**Decision:** Introduce `/api/task/v1` with Task terminology, strict schemas,
typed commands, opaque cursors, stable errors, expected versions, idempotency,
and capabilities. Keep existing `/api/v1/.../work-items` compatibility routes.

**Reason:** A dedicated client and future integrations need a deliberate
contract, while an abrupt rename would break current clients and MCP fixtures.

## TP-D006: Personal Tasks use a private Personal Project

**Status:** Architecture decision

**Decision:** Inbox is a projection of one marked private Personal Project in
the Personal Organization. Every Task remains Project-owned.

**Reason:** Nullable tenant ownership and user-owned exception paths cause
permission drift and make later collaboration/moves ambiguous.

## TP-D007: Integrated relationships stay outside the core aggregate

**Status:** Architecture decision

**Decision:** Threads, Agent Runs, Workers, repositories, and GitHub projections
remain Agent Workspace-owned and attach via explicit links/adapters.

**Reason:** Standalone task semantics must not require execution/GitHub while
integrated use must preserve rich connected work.

## TP-D008: PostgreSQL outbox and invalidation/refetch remain sufficient

**Status:** Architecture decision

**Decision:** Keep transactional outbox, in-process WebSocket broadcast, bounded
external polling, and canonical HTTP refetch. Do not add a broker in v1.

**Reason:** The current design already provides durable mutation facts and
restart recovery. No measured scale or delivery requirement justifies Kafka,
Redis, or another service.

## TP-D009: Generated schema-backed TypeScript client

**Status:** Architecture decision

**Decision:** OpenAPI 3.1 is the reviewed HTTP artifact; TypeScript types,
runtime validators, and bindings are deterministic generated outputs. Actual
Rust responses pass contract tests.

**Reason:** Two clients and external consumers cannot safely rely on duplicated
handwritten types or permissive response normalizers.

## TP-D010: Shared UI is proven, not preemptive

**Status:** Architecture decision

**Decision:** Share contract/client immediately; extract only stable semantic
Task primitives after both applications implement them. Shells, navigation, and
integrated UI remain product-owned.

**Reason:** Early shared pages couple product direction and accumulate mode
flags. Behavioral primitives offer reuse without erasing product identity.

## TP-D011: Standalone is a runtime mode of the same binary

**Status:** Architecture decision

**Decision:** Task-only mode registers only identity/Project/task routes and
workers, serves task assets at root, uses the same database/migrations, and has
no provider/Git/GitHub readiness dependency.

**Reason:** It proves independent usability at much lower operational and
migration cost than a new service.

## TP-D012: Personal/team offline and cross-Project moves are deferred

**Status:** Architecture decision

**Decision:** v1 is responsive online web with reconnect/refetch. Cross-Project
move and durable offline mutation queues require later contracts.

**Reason:** Both operations cross workflow, membership, key, conflict, and
disclosure boundaries. A simplistic implementation would corrupt semantics.

## Risk review

| Risk | Consequence | Mitigation / acceptance gate |
|---|---|---|
| Premature microservices | Distributed transactions, dual writes, operations burden | Same process/database until measured extraction threshold |
| Coupling to Agent Workspace Projects | Standalone feels execution-specific | Keep Project as neutral ownership boundary; capability-gate integrated links |
| Personal vs Organization Tasks | Nullable ownership and policy forks | Personal Organization + marked Personal Project |
| API compatibility | Existing web/MCP breaks | Compatibility adapters, schema diff, effect parity, deprecation window |
| Permission drift between clients | Data disclosure or inconsistent actions | Server-only centralized policy and returned capability hints |
| Event duplication | Duplicate notifications/integration writes | UUID event ID, at-least-once contract, consumer dedupe, canonical refetch |
| Schema ownership ambiguity | Adapters add conflicting fields/tables | Task module owns core; explicit link-table ownership matrix |
| M10 assumptions are wrong | API duplicates current handler logic | Hard evidence gate before write API; exact names deferred |
| Migration/rollback loss | IDs/history/links lost | In-place adoption, additive schema, inventory, restore and prior-binary test |
| Shared UI over-coupling | Conditional product shell and blocked releases | Share client first; primitives only after two implementations |
| Task-only mode accidentally starts execution | Secrets/process attack surface | Allowlisted mode, route/worker gating, negative listener/process tests |
| Mobile/offline expectation | Stale edits and user distrust | Online-first disclosure, optimistic conflict UI, no durable offline writes |
| Search/export disclosure | Cross-tenant inference/bulk data leak | Authorize before counts/rank; scoped cursors; export policy/audit |
| Long-term open-source extension | Forks depend on database internals | Stable API/schema/SDK and adapters; no privileged integration database access |

## Why this is one system, not synchronized systems

Two databases would require a source designation, identity mapping, conflict
resolution, reconciliation jobs, lag UX, delete/archive semantics, permission
mapping, event deduplication, operational alerts, and rollback of partial dual
writes. None produces user value. Multiple clients already provide independent
experiences over one server-authoritative domain. The correct seam is the
application/API contract, not replication.

## Genuinely open owner decisions

### TP-O001: Public product name and URL vocabulary

**Decision required before:** public beta branding or permanent external URLs.

**Recommended default:** keep internal `Task Platform`, dedicated UI `Tasks`,
and `/tasks` integrated base path. Do not rename database/Rust concepts now.

### TP-O002: Packaging/distribution of task-only mode

**Decision required before:** publishing install artifacts to third parties.

**Recommended default:** one server image/binary with allowlisted runtime mode
and two asset builds. Split artifacts only if size or release operations prove
material.

### TP-O003: External SDK publication and support promise

**Decision required before:** advertising a public API/SDK.

**Recommended default:** keep workspace-private through TP4; publish only after
compatibility telemetry, documentation, auth onboarding, and deprecation
ownership exist.

### TP-O004: Public hosted multi-tenancy

**Decision required before:** any public hosted deployment.

**Recommended default:** remain self-hosted/private; resolve existing O-006 and
O-008 with a new threat model and isolated execution architecture.

Everything else foundational—one canonical model, M10 reuse, Project ownership,
Actor identity, monorepo/separate client, generated contract, outbox, and
task-only mode—is decided by this package and should not be reopened during
ordinary implementation without new evidence and a replacement decision.
