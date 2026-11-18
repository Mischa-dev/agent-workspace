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
  `connection_create`, `automation_write`, …), so anything an agent can do, a
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

A webhook is a connection with an address: the same delivery agents get
(A2), pushed. Settings › Account › Webhooks, or the `connection_list`,
`connection_create`, `connection_update`, `connection_secret_rotate`,
`connection_test`, `connection_delivery_list`, `connection_redeliver` and
`connection_remove` tools; `event_kind_list` lists the event kinds.

- A webhook belongs to the account that made it and hears what it can see
  in the spaces its subscription names (or the whole workspace), its own
  changes included when `echoes` is on. Each event is checked again when it
  is read: an event the account can no longer see, of a module that is off
  where it happened, or that is no longer true is skipped. One a person's
  app (or script) set up keeps to that app's limit, and stops, saying why,
  when the app is disconnected; so does one that keeps failing for 48 hours
  and 20 deliveries.
- Body (the `standard` format): `{"type", "timestamp", "id", "cursor",
  "workspace", "recipient", "reason", "actor", "data"}`. It carries ids,
  kinds and field names, never text: fetch the thing with your own access.
- Signing: Standard Webhooks. `webhook-id`, `webhook-timestamp` and
  `webhook-signature: v1,<base64 HMAC-SHA256 of "<id>.<timestamp>.<raw
  body>">`, keyed with the base64-decoded part of the `whsec_` secret. The
  older `X-Workspace-Delivery`, `X-Workspace-Event`, `X-Workspace-Timestamp`
  and `X-Workspace-Signature: sha256=<hex HMAC of "<timestamp>.<body>">` (keyed
  with the whole secret) headers are sent too. The secret is shown once at creation or rotation and
  stored encrypted. The id is the same on every retry and on a redelivery,
  so a receiver can drop repeats.
- Targets pass the same address checks as agent wakeups: https, no private
  or local addresses unless listed in
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
