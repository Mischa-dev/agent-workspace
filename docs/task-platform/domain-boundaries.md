# Domain Ownership Boundaries

## Boundary rule

The canonical task domain is the only owner of Task/Work Item state. Clients
and adapters request commands and consume projections/events. Agent Workspace
features attach through explicit link records; they do not add nullable
conversation, execution, or provider columns to the canonical Task record.

## Ownership matrix

| Capability or record | Canonical task domain | Agent Workspace domain | Dedicated client | Adapter/integration |
|---|---|---|---|---|
| Task identity, key, version, archive | Owns | Reads | Reads | Reads |
| Title, description, type, priority, status | Owns commands and validation | Requests commands | Requests commands | Requests commands |
| Workflow definitions and transitions | Owns | Projects status UI | Projects status UI | Resolves semantic command |
| Parent/subtask | Owns | Reads/links | Edits | Uses bounded commands |
| Acceptance criteria | Owns | Edits | Edits | Uses bounded commands |
| Labels and Task relationships | Owns when implemented | Projects | Edits | Uses bounded commands |
| Comments and Task activity | Owns immutable records and order | Renders | Renders | Appends/consumes |
| Task event facts | Owns | Consumes | Consumes | Consumes authorized subset |
| Actor, Organization, memberships | Shared identity domain owns | Consumes | Consumes | Authenticates through identity adapter |
| Project | Shared Project domain owns | Full Project environment | Task container | Never substitutes its own tenant |
| Capability computation | Server policy owns | Displays | Displays | Enforced server-side |
| Channels, Messages, Threads | — | Owns | Links/deep-links only | May create/link through AW adapter |
| Agent Runs and provider sessions | — | Owns | Capability hint/deep-link only | External runtime remains separate |
| Execution Workers/profiles | — | Owns | Never owns | Invoked only through AW command |
| Repositories/worktrees | — | Owns | Displays safe link summary | Git/GitHub adapter owns sync |
| Pull-request/check/review projection | — | GitHub module owns | Displays safe link summary | GitHub adapter owns refresh |
| Saved view presentation | Owns validated query definition | May have AW-specific views | Owns route/layout preferences | — |
| Query cache, drafts, selection | — | Client-local | Client-local | Client-local |
| Notification delivery channel | Notification module owns | Configures/renders | Configures/renders | Delivery adapter sends |

`—` means the domain does not own the concept, not that it may bypass it.

## Canonical task aggregate

The aggregate root is the existing Work Item row, surfaced as `Task` in the new
contract. Commands that change any non-commutative Task state require the
current Task version. One successful command increments the version once and
commits its activity, audit, and outbox records atomically.

Owned children and projections:

- acceptance criteria
- immutable comments and structured reference projections
- ordered task timeline
- parent/subtask edge
- future labels and canonical task relationships
- Project workflow definitions and board rank
- blocked state
- archive state

Project workflow configuration is a Project-owned task-domain aggregate. A
Task references stable type/status IDs. Business logic uses semantic status
categories rather than editable names.

## Identity and membership boundary

Identity is shared infrastructure, not client-owned task state:

- `actors.id` is the authorization and attribution identity.
- Browser sessions, MCP/OAuth grants, external-agent bearers, internal Worker
  delegation, and system jobs authenticate or resolve an Actor.
- Organization membership establishes tenant presence; Project membership and
  centralized policy establish resource authority.
- Task commands receive an authenticated principal, never an author or
  unrestricted tenant ID from a request body.

The task domain may ask policy questions such as `TaskView`, `TaskCreate`,
`TaskEdit`, `TaskComment`, `TaskArchive`, `TaskManageWorkflow`, and
`TaskExport`. It does not compare role strings inside transports.

## Agent Workspace attachments

Agent Workspace-specific records attach with join tables or adapter-owned link
records:

```text
Task (work_items.id)
  ├── work_item_conversation_links ──► Thread
  ├── work_item_repository_links ────► Repository
  ├── github_pull_request_work_item_links ─► Pull request projection
  └── work_item_worker_invocations ──► Thread + Agent Run request
```

Rules:

1. The Task domain validates the Task side and delegates target validation to
   the owning module in the same transaction.
2. Link activity uses the initiating Actor and canonical Task timeline.
3. Deleting or disabling an integration never deletes the Task.
4. Task-only mode may omit integrated link expansions or return them as
   unavailable capabilities; canonical Task reads still succeed.
5. A Task status does not derive directly from a run. GitHub/Worker automation
   requests an ordinary transition and records its Integration Actor.

## Conversations and Threads

Threads and Tasks remain many-to-many and independently valid. A comment is not
a Thread Message, and a Task is not a conversation. The dedicated client uses
Task comments. It may show Thread link summaries only when the server advertises
`agentWorkspaceLinks.read` and the Actor may view the target.

Conversation reference nodes may refer to Task IDs; the task domain provides a
safe reference projection. It does not own Message content or conversation
sequence allocation.

## Agent runs and execution workers

Agent Runs remain bound to Threads and Execution Profiles. A Task can request
`Work with Worker` only through an Agent Workspace orchestration command that
creates/links a Thread and invokes a Worker under existing sponsorship,
approval, repository, and profile rules. The dedicated client should deep-link
to Agent Workspace for this flow in the first release.

Execution Workers are never Task assignees. Human and external-agent Actors are
valid assignees under the accepted model.

## GitHub integration

GitHub owns repositories, commits, pull requests, reviews, and checks. The
GitHub module owns credentials, webhook verification, refresh jobs, canonical
projection, and automation evaluation. Task links are explicit and confirmed;
heuristic matches remain unconfirmed hints.

GitHub automation may request only valid ordinary transitions, never mark Done,
and never reopen terminal work. The Task domain remains authoritative even when
the Integration Actor initiated a change.

## External agents and MCP

MCP is an adapter over Project-scoped reads and commands. Its current tools
enforce Actor identity, expected versions, idempotency, audit, and bounded
polling. After accepted M10, overlapping create and field/status updates use the
same canonical commands as HTTP, and comments/surface-specific mutations reuse
shared primitives where applicable. MCP reads remain in `mcp/reads.rs`; TP1 must
define a deliberate authorized projection boundary before another read client
is added. MCP retains transport-only concerns such as schema annotation, tool
naming, cursor encoding, idempotency replay, and result mapping. MCP-only
subtask, pull-request, blocker/review, and sponsored-execution semantics are not
forced into artificial HTTP parity.

External agent runtimes own their model, memory, prompts, schedules, and loops.
The platform owns only their Actor, credential, memberships, authorized data,
events, approvals, and audit.

## Notifications and attention

The task domain emits facts such as assignment, mention, comment, status
change, blocking, and criteria completion. A notification/attention module
derives recipient-specific notification intents using current authorization
and preferences. Delivery adapters handle in-app, email, or push later.

Do not put `notified=true` on Tasks or treat event publication as delivery.
Read/seen state is per Actor. Disabling a membership must stop future delivery
and reauthorize live subscriptions.

## Server-only boundaries

The following never enter shared client packages:

- SQL rows, transaction objects, database constraint details, and internal
  idempotency reservations
- secret digests, session hashes, OAuth refresh tokens, GitHub credentials,
  provider credentials, and filesystem paths
- raw provider events and webhook bodies
- role-to-action tables or trusted authorization decisions
- internal audit payloads beyond explicitly safe projections
- integration retry/job lease state unless exposed as a bounded diagnostic

## Invariants

1. A Task belongs to exactly one Project and therefore one Organization.
2. Task ID and version are identical in every client and adapter.
3. A client cannot submit authorship.
4. Every mutation is authorized inside the command transaction.
5. Non-commutative mutations use optimistic concurrency.
6. Retryable creation/append/batch actions use idempotency.
7. Activity, audit, and durable event facts commit with the mutation.
8. Cross-tenant absence and denial do not disclose resource existence.
9. Views and boards are projections, not owners.
10. Agent Workspace links cannot make the core Task unreadable in task-only
    mode.
