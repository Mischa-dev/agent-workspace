# MCP compatibility

## Production contract

- Server SDK: official Rust SDK `rmcp` `3.0.1`
- Verified client: `codex-cli 0.146.0`
- Negotiated protocol: MCP `2025-06-18`
- Transport: Streamable HTTP at `http://127.0.0.1:3100/api/v1/mcp`
- Request ceiling: 1 MiB (a 100-task task_create_many plan fits)
- Session mode: legacy session compatibility with JSON request/response enabled
- Network boundary: the ordinary loopback application listener, reached from
  the Mac only through SSH forwarding

The server rejects missing, duplicated, malformed, expired, revoked, or
wrong-Project credentials with HTTP 401 and a standards-compatible Bearer
challenge. The challenge points to protected-resource metadata, which in turn
identifies the loopback authorization server. Browser cookies are never an MCP
resource credential.

Initialization instructions put immutable Project/Work Item identification,
useful-update restraint, optimistic concurrency, pull-request linking, and
permission limits inside their first 512 characters. Connecting, listing, or
calling tools creates no application Conversation, Thread, Message, Agent Run,
workspace, branch, or worktree.

## Authentication

The development fallback is a 256-bit Project-scoped bearer credential bound
to one active human Actor and explicit `read` or `read write` scopes. The host
prints the token once, stores only a SHA-256 digest plus a safe prefix, and
supports metadata listing, optimistic revocation, and atomic replacement.
Live revocation is checked on every request. Codex receives the token only from
the environment variable named by `bearer_token_env_var`.

The team path is OAuth authorization code with PKCE `S256`. Agent Workspace
publishes protected-resource and authorization-server metadata, dynamically
registers only public clients with exact HTTP loopback redirects and explicit
ports, and requires an authenticated browser session plus CSRF-protected
consent. Consent selects one active Project and shows the named client and
requested scopes. Codes are five-minute, single-use, and bound to the exact
client, redirect URI, and PKCE challenge. Access tokens expire after one hour;
refresh tokens expire after 30 days, rotate atomically, and revoke the complete
grant family if an old refresh token is reused. Codes and all tokens are stored
only as SHA-256 digests.

Bearer writes use `authentication_class=mcp_bearer`; OAuth writes use
`authentication_class=mcp_oauth`. Both preserve the authenticated human Actor
as the domain author and separately record `channel=mcp`, the bounded client
name, request ID, and the corresponding credential or grant ID. Tokens,
digests, authorization headers, codes, verifiers, arbitrary update text, and
repository content are excluded from audit metadata.

## Agent accounts across projects

A named external agent keeps its actor ID and connection when an administrator
adds it through Project settings, Team, Add existing member. The connection's
original project remains its enrollment and revocation anchor. Additional
projects must belong to the same organization and have an enabled membership
for that actor. Disabling the connection or its anchor stops it everywhere;
disabling an additional membership stops access only in that project.

Agents call `projects_list` to discover memberships. Use `projectId` for
project-wide tools; conversation, work item and reply request IDs resolve their
project per call. No session-wide selected project is stored. `inbox_list`
collects accessible requests across memberships and includes each `projectId`.
Human mentions in project channels and threads create required inbox requests for the mentioned agents. Agent messages do not trigger new project reply requests. Private conversation membership is still required.

People who connect an app through the consent screen (migration 0042) connect
their whole account: the app reaches every project they belong to, checked on
each call, and can be read-only or full access. Human bearer credentials and
OAuth grants made before 0042 remain restricted to their original project.

WorkOS AuthKit handles human browser sign-in. MCP continues to use the existing
Workspace OAuth and bearer endpoints; WorkOS Agent Auth is not required.

## Tools

The full list, grouped by job, is in
[docs/plans/agent-interface.md](../plans/agent-interface.md). There are 58.
Read-only tools: `briefing`, `projects_list`, `project_open`,
`workflow_get`, `project_members`, `conversation_list`, `conversation_open`,
`conversation_read`, `message_history`, `people_search`, `inbox_list`, `task_list`,
`task_open`, `label_list`, `search`, `page_list`, `page_get`,
`decision_list`, `repository_get`, `pull_request_get`, `approval_get` and
`external_event_poll`.

Task writes keep the canonical command contract below: `task_create`,
`task_update` (including status), `task_comment`, `task_mark_blocked`,
`task_mark_ready_for_review` and `task_link_pull_request`. They accept the
older `workItemId` argument name as well as `taskId`, as `task_criteria` and
`task_link` do.

Every non-commutative command requires `expectedVersion`; every command
requires a stable `clientRequestId`. Exact completed-request replay succeeds
without duplicate activity, audit success, relation, outbox event, or version
advance. A stale version returns the current version, provable changed fields,
and a stable retry category. Invalid transitions, permission changes, archive,
missing external links, idempotency payload mismatch, and transient failure
remain distinct outcomes.

Among the task tools, `task_create`, `task_create_many`, `task_comment`,
`task_reorder`, `task_claim`, `task_release` and `task_link_pull_request` are
marked non-destructive; `task_update`, `task_update_many`, `task_criteria`,
`task_mark_blocked`, `task_mark_ready_for_review`, `task_link`, `task_move`
and `label_write` are destructive. Only `task_link` and
`task_link_pull_request` are open-world.

## MCP Phase 2 changes (October 1, 2026)

New tools, taking the count from 50 to 55:

- `label_list` and `label_write`. `label_write` is idempotent by state, so it
  takes no `clientRequestId`; creating a name that already exists returns
  that label with `created: false`. Its `expectedVersion` is optional.
  Owners, admins and members (people and agents) write labels; viewers and
  read-only apps only read them.
- `task_criteria`: requires `expectedVersion` and takes `clientRequestId`.
- `task_create_many` and `task_update_many`. `task_create_many` requires
  `clientRequestId` unless `preview: true`; `task_update_many` takes one, and
  `expectedVersions` keyed as the tasks were named. A retry within 24 hours
  returns the first result with `idempotentReplay: true`. Their results add
  one outcome to the shared shape: `rejected`, meaning nothing changed, with
  a `message` and a `problems` list (index, ref or task key, field, message,
  and `currentVersion` when a task changed since it was read).

`WriteResult` gained only optional fields. Stored replays and older clients
still decode, and `retryCategory` values are unchanged:

- `applied`: `key`, `url`, `message`, and `labels` (the task's labels after
  the change, when the call touched them).
- `conflict`, `not_found`, `permission_denied` and `invalid_request`:
  `message`, in plain words.
- `invalid_transition`: `currentStatus` and `requestedStatus` by name,
  `allowedStatuses` (id, name, category), `path` (the shortest way to the
  requested status, one status per update; empty when it can't be reached)
  and `message`.

New arguments, all optional:

- `task_create`: `status`, `type`, `labels`, `acceptanceCriteria`,
  `position` (top, bottom, or a number where 1 is the top; `3` and `"3"`
  both work), `beforeTaskId` and `afterTaskId`.
- `task_update`: `type`, `parentId`, `clearParent`, `addLabels`,
  `removeLabels`, `position`, `beforeTaskId`, `afterTaskId` and `archived`.
  `assignee` takes `"none"` to unassign.
- `task_list`: `projectIds`, `labels`, `type`, `priority`, `createdBy`,
  `updatedSince`, `blocked`, `claimed` (yes, no, me, an @handle, or true and
  false) and `includeArchived`.
- `task_open`: `include: ["history"]`, `historyLimit` and `historyBefore`.
- `task_reorder`: `beforeTaskId` and `afterTaskId`.
- `task_link`: `repositoryId`, `pullRequestId`, `role` (primary or related)
  and `expectedVersion`.

New result fields: `total` on `task_list`, and `type`, `labels` and
`archived` (only on archived tasks, at `boardPosition` 0) on its task lines;
`labels` on `task_open`'s task and on `project_open`; `history` on
`task_open` when asked for; `changed` and `tasks` on `task_reorder`; and
`key`, `changed`, `version`, `role` and `notes` on `task_link`.

Behavior changes:

- A task whose status changes goes to the bottom of its new column unless
  the call gives a place. Before, it kept its old rank. `task_update_many`
  puts tasks there in the order given. This applies in the web app too.
- Archiving a task releases its claim, through MCP and in the web app.
- Restoring the archived original of a moved task is refused. `task_update`
  answers `conflict` with `retryCategory: do_not_retry`, naming the new key
  only to someone who can reach its project; the web API answers 409.
- `archived` must be passed on its own in `task_update` and
  `task_update_many`.
- An exact status name wins over a status group spelled the same way.
- Every `task_link` change bumps the task's version, as other task writes
  do. Before, linking a thread, message, page or decision left the version
  alone, so a version read before a link is now stale after it. Archived
  tasks, new links to archived repositories, and threads the caller can't
  see are refused.
- `clientRequestId` is at most 128 characters, the audit log's limit. A
  longer one is now refused as `invalid_request` instead of failing every
  write.
- A subtask create sent before this release with `priority` or `assignee`
  hashes differently now, because those fields are no longer dropped. Retried
  within the 24-hour window, it answers `conflict` with
  `retryCategory: use_new_request_id`.
- `task_list` cursors read `{offset}.{fingerprint}`, and a cursor from a
  different search is refused. A bare number still works as an offset.
- In the web API, linking a thread or repository to an archived task, or
  changing an archived task's repository links, is 409; unlinking a thread
  from one still works. Linking a thread the caller can't see is 404.

## MCP Phase 3 changes (October 2026)

New tools, taking the count from 55 to 58:

- `message_edit`: the author only. Takes `expectedRevision` (optional) and
  `clientRequestId`; it shares the web edit's idempotency scope and stored
  result, so a key retried on either surface replays. Editing to the same
  content returns `changed: false` and changes nothing.
- `message_delete`: destructive and idempotent by state (removing twice
  returns `alreadyRemoved: true`). Authors remove their own messages;
  project owners and admins, people only, remove anyone's. The text of the
  message and of every earlier version is deleted, along with every copy the
  server made of it; a send replayed with the original `clientRequestId`
  after removal is refused as a different payload.
- `message_history`: read-only, archived conversations included.

`conversation_read` and `conversation_open` items gain `revision` (once
edited), `editedAt`, `removed` and `removedBy`; a removed message's `text` is
empty. `conversation_list` shows a removed last message as "Message removed".
Messages you can't see are `not_found` for every tool, never
`permission_denied`.

## Verification history

The M0 development spike established the `2025-06-18` negotiation with Codex
`0.146.0` on Red Hat and proved an authenticated read, optimistic update,
conflict, and no-Thread behavior against a fake store. That store and its fake
tools are not mounted by the application.

M6 automated verification replaces that shortcut with the real PostgreSQL
domain. It covers metadata, registration constraints, browser consent, PKCE,
single-use codes, access and refresh expiry, rotation, refresh-reuse family
revocation, bearer/OAuth parity, Project and Actor revalidation, every read and
write category, conflict/replay behavior, audit attribution, secret
redaction, Streamable HTTP initialization, and zero Conversation creation.

Exact Red Hat and real local Codex acceptance is recorded in
`docs/acceptance/m6.md`. The live `0.146.0` compatibility findings are:

- Codex sends the same protected resource in the authorization request and in
  the token request. Agent Workspace accepts only an identical duplicate and
  rejects an absent/mismatched binding where the protocol requires it.
- Codex refreshes before MCP initialization when its local OAuth `expires_at`
  metadata is due. Expiring only the server-side access-token row does not
  cause `0.146.0` to refresh after a resource 401 during the same startup.
- The macOS Keychain service is `Codex MCP Credentials`; the account key for
  the accepted server was `agent_workspace|bc870134026a181b`. Changing that
  record non-interactively correctly requires macOS user authorization. The
  supported file store uses an owner-only `.credentials.json` and was used for
  the controlled refresh-rotation proof without exposing its contents.
- `codex mcp logout agent_workspace` removes the local credential but does not
  call the advertised revocation endpoint in `0.146.0`. Operators must revoke
  the grant at Agent Workspace before or together with local logout. The live
  acceptance called `/oauth/revoke`, observed family-wide revocation, removed
  the local file, and then proved required MCP startup fails closed.
- Chrome automation could not follow its top-level OAuth form POST or loopback
  callback navigation because the browser extension blocked those navigations.
  The same real Chrome tab submitted the exact CSRF-protected same-origin form,
  validated the returned 303 callback origin/path/state/code, and dispatched
  that callback to the waiting local Codex listener. This is an automation
  transport quirk, not an application OAuth relaxation.
