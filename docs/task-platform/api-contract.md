# Versioned Task API Contract

Status: proposed public contract. Exact handler-to-command mapping is
M10-dependent. Existing `/api/v1/.../work-items` routes remain the implemented
compatibility surface until this contract is delivered.

## Principles

- Base path: `/api/task/v1`
- JSON over HTTPS; UTF-8; camelCase fields
- Task is the public name for the existing canonical Work Item
- Project remains the ownership/scope term
- Resource reads plus typed action endpoints; no database-shaped generic patch
- Opaque cursors, bounded page sizes, stable ordering
- `Idempotency-Key` for every retryable creation, append, action, and batch
- `expectedVersion` for non-commutative Task mutations
- Server-resolved Actor identity; no author field
- Capabilities in representations are hints, not authorization grants

## Discovery and capabilities

```text
GET /api/task/v1
GET /api/task/v1/capabilities
GET /api/task/v1/projects
GET /api/task/v1/projects/:projectId
```

The root returns contract version, documentation/schema URL, minimum supported
client version if enforced, and deprecation notices. Project representations
contain safe capabilities calculated for the current Actor.

## Task resources

```text
GET  /api/task/v1/projects/:projectId/tasks
POST /api/task/v1/projects/:projectId/tasks
GET  /api/task/v1/projects/:projectId/tasks/:taskId
POST /api/task/v1/projects/:projectId/tasks/:taskId/update
POST /api/task/v1/projects/:projectId/tasks/:taskId/transition
POST /api/task/v1/projects/:projectId/tasks/:taskId/archive
POST /api/task/v1/projects/:projectId/tasks/:taskId/restore
POST /api/task/v1/projects/:projectId/tasks/:taskId/move-on-board
```

`update` accepts only ordinary fields: title, description, type, priority, and
assignee. Status changes use `transition`; archive/restore and board moves have
typed commands. Omitted means unchanged; explicit `null` is supported only for
nullable fields such as assignee.

Example detail:

```json
{
  "id": "019...",
  "projectId": "019...",
  "key": "AW-42",
  "number": 42,
  "title": "Define task client cache boundaries",
  "description": "...",
  "type": { "id": "019...", "name": "Task" },
  "status": {
    "id": "019...",
    "name": "In Progress",
    "semanticCategory": "started"
  },
  "priority": "high",
  "assignee": {
    "actorId": "019...",
    "displayName": "Mischa",
    "handle": "mischa",
    "kind": "human",
    "active": true
  },
  "parentTaskId": null,
  "blocked": null,
  "version": 7,
  "archivedAt": null,
  "createdAt": "2026-08-19T18:00:00Z",
  "updatedAt": "2026-08-19T18:05:00Z",
  "capabilities": {
    "view": true,
    "edit": true,
    "comment": true,
    "archive": true,
    "transitionToStatusIds": ["019..."]
  }
}
```

Create returns `201`, `Location`, the canonical Task, and an idempotent replay
indicator header or metadata. A repeated key with an identical normalized
request returns the stored result. Reuse with another payload returns
`IDEMPOTENCY_KEY_REUSED`.

## Workflow and assignees

```text
GET /api/task/v1/projects/:projectId/workflow
GET /api/task/v1/projects/:projectId/assignees?q=&cursor=&limit=
```

Workflow returns stable IDs, names, semantic categories, order, defaults, and
allowed transitions. v1 is read-only unless workflow management is explicitly
implemented through separate typed commands.

## Criteria, comments, and timeline

```text
GET    /api/task/v1/projects/:projectId/tasks/:taskId/criteria
POST   /api/task/v1/projects/:projectId/tasks/:taskId/criteria
POST   /api/task/v1/projects/:projectId/tasks/:taskId/criteria/:criterionId/update
POST   /api/task/v1/projects/:projectId/tasks/:taskId/criteria/:criterionId/remove
POST   /api/task/v1/projects/:projectId/tasks/:taskId/comments
GET    /api/task/v1/projects/:projectId/tasks/:taskId/timeline
```

Criteria mutations require `expectedVersion` because order and completion
participate in the Task aggregate. Comments are immutable append operations,
require idempotency, and use the existing versioned structured-content AST.
Timeline cursors are scoped to Project and Task and traverse monotonically by
the stored timeline sequence.

## Subtasks, labels, and relationships

```text
GET  /api/task/v1/projects/:projectId/tasks/:taskId/subtasks
POST /api/task/v1/projects/:projectId/tasks/:taskId/subtasks
GET  /api/task/v1/projects/:projectId/labels
POST /api/task/v1/projects/:projectId/tasks/:taskId/labels/:labelId/attach
POST /api/task/v1/projects/:projectId/tasks/:taskId/labels/:labelId/detach
GET  /api/task/v1/projects/:projectId/tasks/:taskId/relationships
POST /api/task/v1/projects/:projectId/tasks/:taskId/relationships
POST /api/task/v1/projects/:projectId/tasks/:taskId/relationships/:relationshipId/remove
```

Subtask creation is ordinary Task creation with a validated parent in the same
Project. v1 permits one direct parent and rejects cycles. Relationships are
same-Project, canonicalized so the inverse is projected rather than stored as a
duplicate edge.

Labels and generalized relationships are roadmap features; the API paths do
not claim current implementation.

## Views and search

```text
GET /api/task/v1/projects/:projectId/tasks?query=&statusId=&statusCategory=&typeId=&priority=&assigneeActorId=&labelId=&blocked=&archived=&sort=&cursor=&limit=
GET /api/task/v1/projects/:projectId/board?...same-filter-subset...
GET /api/task/v1/tasks/my?...authorized-cross-project-filters...
GET /api/task/v1/search/tasks?query=&projectId=&cursor=&limit=
```

List order is explicit and stable. Initial supported sorts:

- `updatedAt:desc,id:desc`
- `createdAt:desc,id:desc`
- `priority:desc,updatedAt:desc,id:desc`
- `boardRank:asc,id:asc` for one status/board column

Cursors encode the filter/sort contract version and last sort tuple, are signed
or otherwise tamper-evident where they carry scope, and expire only when
documented. They are opaque to clients.

Search is authorized before ranking and returns no cross-tenant counts or
existence hints. v1 search covers key, title, and bounded description text.

## Integrated links

```text
GET /api/task/v1/projects/:projectId/tasks/:taskId/links
```

The response groups safe link summaries under `threads`, `repositories`, and
`pullRequests`, each guarded by deployment and resource capability. Task-only
mode may return empty groups with `available: false`. Mutation of these links
stays on Agent Workspace compatibility/integration APIs initially; adding them
to the task API requires neutral link commands and ownership tests.

## Batch operations

```text
POST /api/task/v1/projects/:projectId/tasks/batch
```

Use only for user-selected, homogeneous operations. Maximum: 100 Task IDs.
Request includes one operation, one idempotency key, and `expectedVersion` per
Task. Supported first operations: assign, set priority, attach/detach label,
archive, and transition to one status.

The server processes in deterministic Task-ID order and returns one result per
Task. The batch is not all-or-nothing: a stale or unauthorized Task must not
roll back unrelated authorized Tasks. Idempotent replay returns the identical
result set. Do not support arbitrary per-row patches.

## Import and export

```text
POST /api/task/v1/projects/:projectId/imports/preview
POST /api/task/v1/projects/:projectId/imports/:importId/commit
POST /api/task/v1/projects/:projectId/exports
GET  /api/task/v1/projects/:projectId/exports/:exportId
```

These endpoints are capability-gated future additions. Preview has no domain
side effects. Commit uses ordinary commands and bounded per-row outcomes.
Exports use durable jobs only when synchronous bounded generation exceeds the
request budget; download URLs are short-lived and authorized.

## Optimistic concurrency

Every non-commutative mutation includes:

```json
{ "expectedVersion": 7 }
```

On mismatch return HTTP `409`:

```json
{
  "error": {
    "code": "VERSION_CONFLICT",
    "message": "The Task changed since it was loaded.",
    "requestId": "019...",
    "retry": "refetch_required",
    "details": {
      "resource": "task",
      "currentVersion": 8,
      "changedFields": ["status", "assignee"],
      "current": { "id": "019...", "version": 8 }
    }
  }
}
```

The server never silently merges. A client may automatically retry only when
its operation is demonstrably commutative and the contract marks it safe.

## Idempotency

`Idempotency-Key` is 1–200 bounded safe characters and scoped internally to
authenticated Actor, command, and canonical resource. Store a hash of the
normalized request and the canonical response. Rules:

- identical replay: return prior status/body with no new event/activity
- same key, different normalized payload: `409 IDEMPOTENCY_KEY_REUSED`
- command still running: `409 COMMAND_IN_PROGRESS`, retry with bounded backoff
- retention: publish the minimum window in discovery; clients must not assume
  indefinite retention

MCP `clientRequestId` maps to the same application idempotency concept.

## Error and retry categories

All errors use the envelope above. Stable codes include:

| HTTP | Code | Retry |
|---:|---|---|
| 400 | `INVALID_REQUEST` | `correct_input` |
| 401 | `UNAUTHENTICATED` | `reauthenticate` |
| 403 | `FORBIDDEN` | `do_not_retry` |
| 404 | `NOT_FOUND` | `do_not_retry` |
| 409 | `VERSION_CONFLICT` | `refetch_required` |
| 409 | `INVALID_TRANSITION` | `choose_allowed_transition` |
| 409 | `IDEMPOTENCY_KEY_REUSED` | `new_key_or_correct_payload` |
| 409 | `COMMAND_IN_PROGRESS` | `retry_with_backoff` |
| 413 | `PAYLOAD_TOO_LARGE` | `reduce_payload` |
| 429 | `RATE_LIMITED` | `retry_after` |
| 503 | `TEMPORARILY_UNAVAILABLE` | `retry_with_backoff` |

Unauthorized cross-tenant lookup returns the same external shape and timing
class as absent where practical. Validation details identify fields, never SQL
constraints, role internals, or hidden resources.

## Authentication and CSRF

Browser clients use the existing opaque cookie session. Mutations require the
allowed Origin and exact double-submit CSRF header/cookie/digest checks.
Non-browser clients use scoped OAuth access tokens or explicitly supported
high-entropy development/integration bearers; they never reuse browser cookies.

## Runtime validation and generation

- OpenAPI 3.1 is the reviewed contract artifact.
- Every request and response component has a stable schema name.
- Rust contract tests validate actual handler responses against the schema.
- TypeScript types and runtime validators are generated deterministically.
- The client rejects or quarantines invalid responses instead of filling
  security-relevant fields with permissive defaults.
- Examples in the schema are tested fixtures, not hand-maintained prose only.

## Compatibility and deprecation

- `/api/task/v1` receives additive compatible changes only: optional response
  fields, new enum values only where clients must handle `unknown`, and new
  endpoints/actions.
- Removing/renaming fields, changing meaning, tightening accepted input, or
  changing cursor semantics requires `/api/task/v2` or an announced migration.
- Deprecations are documented in discovery and response headers for at least
  two repository releases and 90 days for any externally published client,
  whichever is longer.
- Existing `/api/v1/.../work-items` routes remain until both in-repository
  clients and pinned MCP compatibility no longer depend on them and migration
  telemetry/tests prove removal safe.
- Server supports at least the current and immediately preceding published task
  client minor line when packages become external.
