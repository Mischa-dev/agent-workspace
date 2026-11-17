# Abuse controls

How the server keeps one caller, or one workspace, from taking what
everyone else needs: the limits, who each one belongs to, and what an
operator can do about a workspace that misbehaves.

## Limits and their scopes (H1.1)

Every limiter is declared once, with its scope, in `LIMITS` in
`apps/server/src/rate_limit.rs`. A unit test fails on a limiter that isn't
declared, and another on any new budget every caller shares.

| Scope | Whose budget it is |
| --- | --- |
| Address | One sender address, as the trusted proxy reports it |
| Login | One login name or email address |
| Credential | One session, token or credential |
| Account | One account |
| Workspace | One workspace, across all its people, agents and apps |
| Link | One link token (an invitation, a bug report or form link) |
| Client | One connected app (an OAuth client) |
| ServerFailuresOnly | The whole server, spent only by failures and junk |
| ServerUnattributed | The whole server, spent only by callers with no known address |
| Server | The whole server, spent by everyone: only the two below |

Successful sign-ins, valid MCP and public API calls, good token refreshes
and link tokens that resolve never touch a server-wide budget. Server-wide
budgets count failed sign-ins, tokens that didn't resolve, bad refreshes and
link tokens that were malformed or matched nothing. While one is spent, only
callers the server has seen succeed lately get through, where that applies.

The two budgets everyone shares, with their reasons, are in `SERVER_WIDE`:
GitHub webhook deliveries (they arrive from GitHub's shared addresses before
any workspace is known) and password reset emails (they leave through the
server's own mail sender).

### Per-workspace ceilings

- **MCP and public API calls**: 3,000 a minute per workspace, from all its
  agents and apps together, beside 600 a minute per credential. Browser
  calls aren't charged here; people have their own per-session budgets.
  Set with `AGENT_WORKSPACE_WORKSPACE_CALLS_PER_MINUTE`.
- **Invitations**: at most 20 attempts a minute per invitation link; misses
  across the server share the failure budget.
- **Bug reports and public form submissions**: 30 a minute per link and
  300 a minute per workspace across its links.

A refused call answers 429 (or, from an MCP tool, `rate_limited` with
`retryAfterSeconds`).

### Settings

| Variable | Default | What it sets |
| --- | --- | --- |
| `AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER` | unset | The header a trusted proxy puts the caller's address in (`cf-connecting-ip` behind Cloudflare). Unset, there are no per-address limits and callers share the unattributed budgets. |
| `AGENT_WORKSPACE_WORKSPACE_CALLS_PER_MINUTE` | 3000 | MCP and public API calls per workspace per minute. |
| `AGENT_WORKSPACE_OAUTH_MAX_UNCLAIMED_CLIENTS` | 1024 | Apps that registered but never finished connecting, before new registrations are refused. |

The limits are counted in PostgreSQL (K5.6, `rate_limit_counters`), so every
server process spends from the same budget and a restart forgets nothing.

## Suspending a workspace (H1.24)

An operator can stop a workspace at once, from the server's machine:

```sh
agent-workspace-server workspace-suspend --slug <address> --reason abuse
agent-workspace-server workspace-resume --slug <address>
```

The reason is `operator`, `abuse` or `billing`. While suspended, every MCP
tool, public API call and operation in the workspace is refused with
`workspace_suspended` (423 over HTTP); its people still sign in and see why
in the workspace's settings. Nothing is deleted.

A workspace's owners can also delete it themselves (`workspace_delete`,
which needs "Delete the workspace"). It is then refused the same way for
14 days, during which `workspace_delete_cancel` keeps it; after that the
`workspace.delete` job removes every row and file of the workspace and
leaves one row in `deleted_workspaces`.
