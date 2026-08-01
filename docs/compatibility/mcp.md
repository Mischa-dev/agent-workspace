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

## Tools

Read-only, non-destructive, closed-world tools:

- `project_get`
- `project_instructions_get`
- `work_item_get`
- `work_item_search`
- `work_item_context_get`
- `repository_get`
- `pull_request_get`

Replay-safe write tools:

- `work_item_set_status`
- `work_item_post_update`
- `work_item_update`
- `work_item_create_subtask`
- `work_item_link_pull_request`
- `work_item_mark_blocked`
- `work_item_mark_ready_for_review`

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
Exact Red Hat and real local Codex evidence belongs in `docs/acceptance/m6.md`
after the accepted green commit is deployed.
