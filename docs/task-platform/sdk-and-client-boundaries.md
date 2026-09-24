# SDK and Client Boundaries

## Contract source and generation

Use a reviewed OpenAPI 3.1 document under `packages/task-contract` as the
versioned HTTP source artifact. Generate:

- TypeScript request/response types
- runtime response validators
- typed endpoint functions
- test fixtures or fixture type checks
- optional JSON Schema components for non-HTTP consumers

Generation is deterministic, pinned, and checked in CI. Rust handlers remain
the implementation; contract tests validate their actual responses against the
schema. Do not hand-maintain a second parallel type hierarchy in each client.

## Package boundaries

### `packages/task-contract`

Owns API schema, examples, compatibility metadata, and generation config. It
contains no UI, fetch implementation, cookies, or product wording beyond the
public Task contract.

### `packages/task-client`

Owns:

- generated wire types and endpoint bindings
- strict runtime parsing of untrusted responses
- normalized `TaskApiError` and retry categories
- base URL and authentication transport hooks
- idempotency-key injection helpers
- cursor/result types
- version/capability negotiation

It does not own TanStack Query, React, navigation, notifications, optimistic UI,
authorization decisions, or browser token storage. The default browser
transport uses same-origin cookies and the application's existing CSRF-token
provider; it never reads the session cookie.

### `packages/task-ui`

Create only after both applications have accepted equivalent behavior. Good
candidates:

- Task key/title row
- status, priority, type, label, and assignee controls
- accessible criteria editor
- structured Task comment renderer/composer primitives
- conflict resolution panel
- filter chips/query builder primitives
- board card and drag accessibility behavior

It must not own:

- Agent Workspace Project rail, Channels, Threads, Worker actions, or context
  drawer
- dedicated task-product shell, Inbox, or Workspace navigation
- routing, server requests, query cache, or deployment capability policy
- GitHub/provider-specific cards

Prefer headless behavior plus semantic markup and token hooks over a package
that forces one product's layout.

## Runtime validation

Current Agent Workspace work-item normalization permissively defaults malformed
fields in several places. The shared client must instead distinguish:

- valid response
- compatible unknown enum value
- absent optional additive field
- invalid/missing required field
- unsupported contract version

Invalid responses produce one redacted diagnostic and a typed client error;
they do not silently turn missing capabilities into authorization or empty IDs.
Unknown enum values render as `Unknown` with safe behavior until upgraded.

Validate at the transport boundary once. Internal components consume parsed
domain projections rather than rechecking arbitrary `unknown` values.

## Query keys

Keys are structured, tenant-scoped, and produced by one helper:

```ts
taskKeys.capabilities()
taskKeys.projects()
taskKeys.project(projectId)
taskKeys.workflow(projectId)
taskKeys.root(projectId)
taskKeys.list(projectId, canonicalFilters)
taskKeys.board(projectId, canonicalFilters)
taskKeys.detail(projectId, taskId)
taskKeys.timeline(projectId, taskId)
taskKeys.assignees(projectId, query)
taskKeys.myTasks(canonicalFilters)
```

Canonicalize filter objects before key construction: omit defaults, sort array
values, and use stable serialized primitives. Never omit Project scope for a
Project-owned Task. Cross-Project queries include Actor/session scope implicitly
through cache reset on authentication changes.

## Cache and realtime behavior

- HTTP is canonical; WebSocket events invalidate keys.
- A successful command writes its returned Task into detail cache, then
  invalidates affected list/board/count roots.
- Optimistic updates are limited to reversible, simple field mutations.
- Status/board moves may render optimistically but retain the prior snapshot
  and reconcile the server's canonical rank/version.
- Comment append may show a pending client row keyed by idempotency key, then
  replace it with the canonical timeline sequence.
- On conflict, restore canonical state and present changed fields; never retry a
  non-commutative mutation blindly.
- Logout, Actor/session change, or membership revocation clears all protected
  query data before rendering the next principal.

## Client-specific responsibilities

### Agent Workspace web

- Keeps Work Item wording unless product design changes it.
- Composes Task details with Threads, runs, repositories, GitHub, and Work with
  Worker.
- Migrates current handwritten Work Item API code to `task-client` incrementally.
- Owns Agent Workspace deep links and connected-work presentation.

### Dedicated task web

- Uses Task, Workspace, Inbox, My Tasks, and task-first navigation.
- Renders only capabilities the deployment and Actor expose.
- Works completely when integrated features are false.
- Owns Task-product filters, saved-view navigation, capture flow, and responsive
  presentation.
- Deep-links to Agent Workspace rather than importing its execution UI.

## Versioning

- In-repository packages share the root version while private.
- The generated client records the supported `/api/task/v1` contract revision.
- Schema compatibility checks classify changes as additive or breaking.
- External publication, if later required, uses semantic versioning: breaking
  TypeScript API or wire requirement is a major; additive endpoints/types are a
  minor; implementation-only fixes are patch.
- Server discovery can declare minimum client version only after a safe upgrade
  path exists; avoid lockstep checks for additive changes.

## Server-only data

Never generate or export types for SQL rows, secret-bearing configuration,
token digests, provider payloads, webhook bodies, job leases, internal policy
tables, filesystem paths, or unrestricted audit metadata. Public schemas are
deliberately authored projections.

## Testing

- generated artifact is reproducible and has no dirty diff
- fixture corpus passes runtime validators
- malformed/unknown-version fixtures fail safely
- endpoint contract tests cover success and every stable error category
- query key tests prove Project/filter separation
- realtime tests prove duplicate invalidation and reconnect convergence
- component tests cover keyboard, focus, conflict, loading, empty, denied, and
  capability-disabled states
- one E2E mutates in one client and verifies the same Task ID/version in the
  other after refetch
