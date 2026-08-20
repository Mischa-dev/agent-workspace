# Events, Realtime, and Integrations

## Event model

Separate four concepts:

1. **Domain fact**: a typed fact committed with a Task mutation.
2. **Outbox record**: durable publication work with a database-ordered cursor.
3. **Delivery envelope**: a privacy-bounded projection for one transport.
4. **Notification intent**: recipient-specific attention derived from facts and
   preferences.

An event is not a second write model. Consumers refetch canonical state.

## Canonical taxonomy

Use namespaced, past-tense event types. Version each envelope and payload
schema independently where useful.

### Task lifecycle

- `task.created`
- `task.fields_updated`
- `task.transitioned`
- `task.archived`
- `task.restored`
- `task.assigned`
- `task.unassigned`
- `task.blocked`
- `task.unblocked`
- `task.parent_changed`

### Collaboration

- `task.comment_added`
- `task.criterion_added`
- `task.criterion_updated`
- `task.criterion_completed`
- `task.criterion_reopened`
- `task.criterion_removed`
- `task.label_attached`
- `task.label_detached`
- `task.relationship_added`
- `task.relationship_removed`

### Integration links

- `task.thread_linked` / `task.thread_unlinked`
- `task.repository_linked` / `task.repository_unlinked`
- `task.pull_request_linked` / `task.pull_request_unlinked`

Provider/run/check details remain events of their owning Agent Workspace or
GitHub domains. They do not become Task events unless a canonical Task mutation
or link changed.

## Durable versus ephemeral

Durable facts include every accepted Task mutation, immutable comment append,
relationship/link change, and automation transition. They commit in the source
transaction with Task version, activity, audit, and outbox insertion.

Ephemeral signals include typing, hover/presence, drag preview, optimistic local
state, and stream progress. They may be dropped and must never be required for
correctness, audit, or notification. Presence is outside v1.

## Canonical envelope

```json
{
  "eventId": "019...",
  "eventCursor": 1842,
  "schemaVersion": 1,
  "type": "task.transitioned",
  "organizationId": "019...",
  "projectId": "019...",
  "resource": { "kind": "task", "id": "019...", "version": 8 },
  "actor": { "id": "019...", "kind": "human" },
  "occurredAt": "2026-08-19T18:05:00Z",
  "payload": {
    "fromStatusId": "019...",
    "toStatusId": "019..."
  }
}
```

Internal envelopes may contain the Actor ID needed for audit and recipient
derivation. Browser invalidations should usually contain only event identity,
cursor, type, Project, resource ID/version, and safe hints. Webhook payloads
require a separate allowlist and never inherit internal payload wholesale.

## Browser realtime

Retain one authenticated WebSocket per browser session. The current server
publishes typed outbox envelopes through an in-process broadcast channel. For
task v1:

- subscribe by authenticated session, not Actor/Project query parameters
- filter to Projects the Actor can currently access
- send small invalidation envelopes
- map Task events to affected query roots and detail IDs
- coalesce bursts and refetch over HTTP
- close or rescope immediately on session/membership revocation
- on lag, schema mismatch, or unknown continuity, instruct the client to
  resynchronize

Reconnect uses exponential backoff with jitter. After reconnect, refetch
capabilities, active Project navigation, visible lists/board columns, open Task,
and notification counts. v1 does not promise complete WebSocket replay.

## Client invalidation mapping

| Event | Minimum invalidations |
|---|---|
| any `task.*` for Task ID | Task detail and Project task root |
| create/archive/restore/transition | Project list, board, counts, My Tasks |
| assign/unassign | Project list, Task detail, assignee views, My Tasks |
| comment/criterion | Task detail/timeline; list only if summary fields change |
| label/relationship | Task detail and any filter/view using affected field |
| workflow definition change | workflow, board, all Project task projections |
| membership/capability change | capabilities, all scoped data, socket reauth |

Event handlers invalidate; they do not reconstruct canonical Tasks from event
payloads.

## Transactional outbox

Keep PostgreSQL-backed outbox and at-least-once publication. Requirements:

- globally unique UUIDv7 event ID
- database-ordered cursor; never timestamp ordering
- source transaction includes the domain change and outbox row
- leased claim, bounded retry/backoff, failure classification, and restart
  recovery
- publishing with zero connected browsers is success for browser broadcast
- deduplication by `eventId` at consumers
- metrics for oldest unpublished age, ready count, failures, and publish latency

A broker is not required for v1. Revisit only after measured multi-process
fan-out, throughput, retention, or external-delivery requirements exceed the
PostgreSQL/outbox design.

## External-agent polling

Retain the accepted M9 model initially:

- connection- and credential-bound opaque signed cursor
- per-connection monotonically increasing sequence
- 30-day bounded retention unless a later decision changes it
- at-least-once delivery
- compact summary, resource kind/ID, canonical path, and occurrence time
- canonical MCP/HTTP refetch for current authorized state
- credential rotation invalidates cursors bound to the prior credential

The existing external events are addressed notifications, not the complete
canonical domain log. Expand taxonomy carefully and only for events useful to
the connected Actor. A connection must not enumerate unrelated Project work.

## Webhooks and future integrations

Do not expose generic webhooks in the first dedicated-client milestone. When
implemented:

- subscriptions are Organization/Project-scoped and human-authorized
- destinations are allowlisted against SSRF policy and resolved safely
- payload schemas are versioned, bounded, and privacy-reviewed
- secrets are one-time displayed, digest/encrypted as appropriate, rotatable,
  and never logged
- deliveries use a dedicated durable delivery table, unique event/subscription
  key, signatures, attempts, next-attempt time, and terminal state
- at-least-once delivery is explicit; consumers deduplicate by `eventId`
- retries honor bounded exponential backoff; operators can inspect/redeliver
- revocation stops new delivery immediately without deleting audit history

Webhook delivery state must not mark the source outbox event unpublished.

## Event compatibility

- Event type meaning is immutable within a schema version.
- Add optional fields compatibly; consumers ignore unknown fields/types and
  refetch when needed.
- Breaking payload changes use a new `schemaVersion` or new event type.
- Never reuse a type name for a different authorization or recipient meaning.
- Keep internal, browser, external-agent, and webhook payload schemas separate
  even when they share a domain fact.

## Privacy and size

- Browser invalidation target: at most 8 KiB; webhook/external event target at
  most 32 KiB unless explicitly justified.
- Do not include full descriptions, comments, criteria lists, provider output,
  secrets, or inaccessible linked-resource data.
- Include display names only when required and authorized; prefer stable IDs.
- Recipient projection occurs with current authorization. Revocation closes
  streams and prevents new deliveries.
- Retention and export policy must cover delivery records separately from
  canonical Task activity.

## Notification preferences

Model preferences per Actor, with Organization default and optional Project
override. Initial categories:

- assigned to me
- mentioned in Task comment
- comment on assigned/watched Task
- Task blocked/unblocked
- status moved to review/verification/completed
- integration link/check attention, when integrated

Preferences control notification intent/delivery, not the canonical activity
or event fact. Security events and directly addressed approvals may have
non-disableable in-app records under existing policy.

## Acceptance tests

1. Mutation, activity, audit, and outbox fact commit or roll back together.
2. Publisher crash after send yields a duplicate, not a missing canonical fact;
   clients deduplicate/refetch.
3. WebSocket lag/reconnect converges through HTTP.
4. Disabled membership/session/token stops delivery without server restart.
5. External cursor cannot be reused across connection, Project, or credential.
6. Payloads remain under limits and exclude descriptions/comments/secrets.
7. Duplicate external or webhook delivery produces no duplicate domain write.
