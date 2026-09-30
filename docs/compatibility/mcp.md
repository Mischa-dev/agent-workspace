# MCP compatibility

## Production contract

- Server SDK: official Rust SDK `rmcp` `3.0.1`
- Verified client: `codex-cli 0.146.0`
- Negotiated protocol: MCP `2025-06-18`
- Transport: Streamable HTTP at `http://127.0.0.1:3100/api/v1/mcp`
- Request ceiling: 256 KiB
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
[docs/plans/agent-interface.md](../plans/agent-interface.md). Read-only tools:
`briefing`, `projects_list`, `project_open`, `conversation_list`,
`conversation_open`, `conversation_read`, `people_search`, `inbox_list`,
`task_list`, `task_open`, `search`, `page_list`, `page_get`,
`repository_get`, `pull_request_get`, `approval_get` and
`external_event_poll`.

Task writes keep the canonical command contract below: `task_create`,
`task_update` (including status), `task_comment`, `task_mark_blocked`,
`task_mark_ready_for_review` and `task_link_pull_request`. They accept the
older `workItemId` argument name as well as `taskId`.

Every non-commutative command requires `expectedVersion`; every command
requires a stable `clientRequestId`. Exact completed-request replay succeeds
without duplicate activity, audit success, relation, outbox event, or version
advance. A stale version returns the current version, provable changed fields,
and a stable retry category. Invalid transitions, permission changes, archive,
missing external links, idempotency payload mismatch, and transient failure
remain distinct outcomes. Tool annotations mark only append, subtask creation,
and explicit PR linking as non-destructive; only PR linking is open-world.

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
