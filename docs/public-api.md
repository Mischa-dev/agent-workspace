# Public API, webhooks and automations

Scripts and outside services reach the workspace three ways: the REST API
(calls in), webhooks (events out) and automation rules (the workspace acting
on its own, as the person who made the rule). All three act as a person and
see only what that person can see right now.

## REST API

- Base URL: `<server>/api/public/v1`. The version is in the path; breaking
  changes get a new one.
- Every operation is `POST <base>/<operation>` with a JSON body. Operations
  are the same tools agents use over MCP (`task_list`, `task_create`,
  `webhook_write`, `automation_write`, …), so anything an agent can do, a
  script can.
- `GET <base>/openapi.json` is the OpenAPI 3.1 document, generated from the
  Rust argument types (`apps/server/src/public_api.rs`). It needs no token.
  Operations carry `x-read-only`. In the app, Settings › Account › API
  reference renders it with a curl example per operation.
- Auth: `Authorization: Bearer <token>` with a personal token from Settings ›
  Account › API tokens (the same tokens as MCP). A token reaches one project
  and acts as its maker; write operations refuse a read-only token. Every call is checked against the maker's current memberships.
- Replies are JSON with an `outcome`. `ok` and `applied` are 200. Otherwise
  the status says what happened: 422 `invalid_request`, 403
  `permission_denied`, 404 `not_found` (also an unknown operation), 409
  `conflict`, 503 `temporarily_unavailable`, 401 without a valid token.
- Pass `clientRequestId` where an operation offers it, so a retry doesn't do
  the work twice.

```sh
curl -X POST "$SERVER/api/public/v1/task_list" \
  -H "Authorization: Bearer $AGENT_WORKSPACE_TOKEN" \
  -H "Content-Type: application/json" -d '{}'
```

## Webhooks

Settings › Account › Webhooks, or the `webhook_list`, `webhook_write` and
`webhook_deliveries` tools.

- Events: `task.created`, `task.updated`, `run.updated`, `run.finished`,
  `pull_request.checks_failed`, `pull_request.checks_passed`,
  `pull_request.merged`, `pull_request.closed`, `message.created`.
- A webhook belongs to the person (or connected app acting as them) who made
  it and hears one workspace: every project they can see, or one project.
  Each delivery is checked again when it's sent: messages go through
  `conversation_visible_to`, pull requests through `pull_request_visible_to`.
  A webhook whose owner left, or whose app was disconnected, turns itself
  off with the reason; so does one that fails twenty deliveries in a row.
- Body: `{"id", "type", "occurredAt", "workspaceId", "project", "data"}`.
- Signing: `X-Workspace-Signature: sha256=<hex HMAC-SHA256 of
  "<X-Workspace-Timestamp>.<raw body>">` with the webhook's secret. The secret
  is shown once at creation or rotation and stored encrypted with the W1
  secrets keyring. `X-Workspace-Delivery` is the same on every retry and on
  "Send again", so a receiver can drop repeats.
- Targets pass the same address checks as agent wakeups (PR #128): https,
  no private or local addresses unless listed in
  `AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS`, and the checked address is the
  one connected to.

## Automation rules

Settings › Project › Automation rules, or the `automation_list`,
`automation_write` and `automation_runs` tools.

- Triggers: `task_created`, `task_matches` (fires when a task enters the
  conditions, not on every change), `run_finished` (optionally by outcome),
  `pull_request_checks_failed`, `message_posted` (optionally one
  conversation and a phrase; the maker's own messages never count),
  `schedule` (daily, weekly or monthly at a local time) and
  `capacity_window` (a provider account resets soon with capacity left).
- Conditions: a saved view (and its filters) the task must match.
- Actions, up to five, in order: `launch` (with the project's run defaults
  or a chosen environment; `agentProfileId` launches as one of the project's agents),
  `set_field`, `notify` and `post_message`. Text can use `{{task.key}}`,
  `{{task.title}}`, `{{task.url}}`, `{{task.status}}`, `{{run.state}}`,
  `{{pr.number}}`, `{{pr.title}}`, `{{pr.url}}`, `{{rule.name}}` and
  `{{event}}`.
- A rule acts as its maker. Before each action it checks their access to the
  project again; field changes and messages go through the same tools as
  the maker would use. A rule that fails the check turns off, and its maker
  is told why, or the project's admins if the maker left the workspace. Only
  the maker can edit or turn a rule back on; admins can turn it off or
  delete it. A rule that fires more than 30 times in an hour turns off.
