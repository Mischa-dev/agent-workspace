# Dedicated Task Product Specification

## Product thesis

The dedicated task product is a fast, calm place for a person or small team to
capture, organize, discuss, and complete work, with agents participating through
the same identity and permission model when useful. It is independently usable
without Channels, hosted execution, repositories, or GitHub. When used beside
Agent Workspace, its Tasks are literally Agent Workspace Work Items—not imports,
mirrors, or synchronized copies.

## Target users

The first user is a solo operator managing personal and project work who wants
strong structure without fake enterprise ceremony. The same model supports a
trusted small team through Organizations, Projects, memberships, roles, and
Actor attribution already present in Agent Workspace.

Initial jobs to be done:

1. Capture a Task in seconds into a personal Inbox or a Project.
2. Find current work through list, board, search, filters, and stable URLs.
3. Plan with descriptions, criteria, subtasks, relationships, priority, type,
   status, assignee, labels, and comments.
4. See a trustworthy, attributable history of changes.
5. Let a human or external agent update the same Task through an authorized API
   or MCP tool without overwriting concurrent work.
6. Open integrated Agent Workspace context when a Task has Threads, runs,
   repositories, or pull requests, without making those features mandatory.

## Relationship to Agent Workspace

| Concern | Dedicated task product | Agent Workspace |
|---|---|---|
| Canonical record | Calls it a Task | Calls it a Work Item |
| Identity | Same `work_items.id` | Same `work_items.id` |
| Core mutations | Same canonical application operations | Same canonical application operations |
| Authorization | Same Actor/Organization/Project policy | Same policy |
| Core activity/events | Same canonical stream | Same canonical stream |
| Navigation | Task-first | Conversation/execution-first |
| Optional context | Displays links/capability hints | Owns Threads, runs, repositories, GitHub UX |

The dedicated client may deep-link to integrated features. It must not implement
Agent Runs, provider sessions, Git worktrees, or GitHub synchronization.
Accepted M10 already supplies canonical create/update operations and shared
mutation primitives. Any currently handler-owned family must gain a canonical
command entry point before the dedicated Task API exposes it; “same operation”
does not mean every surface must expose MCP-only or Agent Workspace-only
convenience commands.

## Standalone terminology

- **Workspace**: the dedicated UI name for an Organization. It is not a new
  record type.
- **Project**: the canonical work boundary inside a Workspace. Tasks always
  belong to exactly one Project.
- **Team**: the people and external-agent members visible through Organization
  and Project memberships. It is not a new tenancy level.
- **Task**: the dedicated UI/API name for the canonical Work Item record.
- **Inbox**: the default view of the user's private Personal Project. It is not
  an ownerless or tenantless task bucket.
- **My Tasks**: a projection of Tasks assigned to the current Actor across
  authorized Projects.
- **View**: a stored or ephemeral query and presentation definition. It never
  owns copies of Tasks.

Existing Agent Workspace code and compatibility APIs continue to say `Work
Item`. New dedicated-client schemas say `Task` and document the identity
equivalence explicitly.

## Navigation

The dedicated shell uses:

```text
Inbox
My Tasks
Projects
  Project A
    List
    Board
    Views
Archived
Settings
```

Workspace and Project switching stay explicit. In solo mode, the Personal
Workspace label is hidden and Inbox is first. Team and administrative surfaces
appear only when the user has more than one relevant membership or permission.

Task detail has one primary editing surface and one ordered activity/comments
timeline. Agent Workspace context appears in a separate `Connected work`
section when links exist and the deployment reports those capabilities.

## Canonical task capabilities

### Existing baseline

- Title and bounded plain-text description
- Type and priority
- Project-owned statuses and allowed transitions
- Optional human or external-agent assignee
- Ordered acceptance criteria with completion attribution
- One-level parent/subtask relationship
- Blocked reason and attribution
- Immutable structured comments with mentions and Task references
- Append-only typed activity and ordered timeline
- Archive/restore
- Linked Threads, repositories, and pull requests
- List, board, search, filters, and Project-scoped readable keys

### Additive version-one task-product scope

- Labels with Project ownership, stable IDs, name, color token, archive state,
  and many-to-many Task links
- Relationship types `blocks`, `blocked_by`, `relates_to`, and `duplicates`,
  stored once with validated direction and no cross-Project link in v1
- Personal Inbox implemented as the Personal Project
- `My Tasks`, unassigned, blocked, recently updated, and archived projections
- Ephemeral URL-serializable filters and sorting
- Saved views only after ordinary filtering is accepted; saved views store
  validated query definitions, not arbitrary SQL
- Bulk assign, label, priority, archive, and valid status transition with
  per-Task outcomes and bounded batch size
- JSON export of canonical Task data and CSV export of a flat projection
- Import preview plus idempotent commit; imported rows use ordinary commands

### Explicitly later

- Recurring tasks, reminders, due dates, estimates, cycles, milestones, and
  dependencies across Projects require focused product decisions.
- Mobile offline mutation queues require a conflict and credential design.
- Third-party integrations use the versioned API/events after those contracts
  are accepted; no integration receives direct database access.

## Interaction ideas adopted deliberately

From Linear-like tools, adopt keyboard-efficient capture, command-driven field
changes, dense list rows, predictable boards, stable task keys, and filterable
views. These reduce navigation cost and make state legible.

Do not copy cycles, initiatives, roadmaps, custom workflow complexity, or a
large shortcut vocabulary before user evidence. Agent Workspace's distinctive
value is attributable human/agent collaboration and connected execution, not
visual imitation of another tracker.

## Personal versus Organization work

Every Task remains Organization- and Project-owned. The bootstrap process
creates or identifies one private Personal Project in the user's Personal
Organization. Inbox Tasks are created there. This avoids nullable tenancy,
user-owned exceptions, and a later migration when a personal Task becomes team
work.

Moving a Task between Projects is not a v1 mutation because workflows,
memberships, readable keys, labels, links, and disclosure boundaries differ.
The safe first behavior is copy-through-command with an explicit source link
and optional source archive; a true move requires a separate atomic design.

## Agents and automation

- Human and external-agent Actors may create, comment, assign, and mutate Tasks
  only through ordinary policy and commands.
- Execution Workers are not assignees. Their bounded runs remain Agent
  Workspace concepts linked to Tasks.
- Automation uses Integration or System Actors, idempotent commands, valid
  workflow transitions, and visible activity.
- GitHub automation remains an adapter and cannot complete or reopen Tasks.
- No agent may choose authorship, expand its Project scope, or silently resolve
  optimistic conflicts.

## Imports and exports

Export is authorized like a list read, applies a bounded server-side query, and
records audit metadata for sensitive/bulk exports. It excludes secrets and
Agent Workspace-only provider payloads.

Import is two-phase:

1. Parse and validate into a temporary, expiring preview with normalized
   fields, mapping choices, errors, and counts.
2. Commit with an idempotency key through ordinary create commands, returning
   per-row results.

No import writes IDs, Actor attribution, versions, or activity directly.

## Version-one non-goals

- A second task service, database, schema, or synchronization worker
- Public multi-tenant SaaS
- Native mobile or desktop applications
- Full offline editing
- Arbitrary custom fields or user-authored workflow code
- Cross-Project task relationships or atomic task moves
- Gantt charts, resource planning, time tracking, billing, or automation canvas
- Reimplementation of conversations, GitHub, code review, or agent execution
- External integration marketplace
- Attachments; the existing source-of-truth boundary remains disabled

## Product acceptance criteria

The dedicated product is independently usable when a user can start a task-only
deployment, sign in, capture an Inbox Task, create a Project Task, use list and
board views, edit fields and criteria, create a subtask and relationship, add a
comment, filter/search, resolve a deliberate conflict, and export authorized
data without any execution provider, repository, GitHub App, or Agent Workspace
client being configured. The same Task must appear by the same UUID and version
in Agent Workspace when both clients are enabled.
