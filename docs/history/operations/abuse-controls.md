> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# Private-host abuse controls

This inventory is the M8 private-beta boundary for the single-process,
loopback-only deployment. Counters are process-local and reset at restart.
They reduce accidental and local abuse; they are not a public multi-tenant
defense and must not be described as one.

Limiter keys are UUIDs, a single global unit key, or SHA-256 digests computed
in memory. Plaintext login attempts, invitation capabilities, bearer tokens,
OAuth values, request bodies, and raw IP-derived identifiers are never stored
as limiter keys or written to logs.

## Entry-point inventory

| Class | Entry points | Input/output bound | Budget or concurrency | Rejection |
| --- | --- | --- | --- | --- |
| Health and static shell | `GET /health/live`, `GET /health/ready`, built web assets | no request body; static files are immutable build output | database readiness uses the five-connection pool without consuming mutation budgets | bounded health status or ordinary HTTP error |
| Browser authentication | session read, login, logout | global application body 256 KiB; password field 1,024 bytes; cookie values 256 bytes | 20 attempts per client address, then 10 per login digest (30 per hour), then 60 global per 60 seconds; password resets 30 per client address per hour, 5 per account per hour and 120 global per 60 seconds, with at most 30 reset emails sent per 60 seconds; two concurrent password KDFs | JSON `429 rate_limited` with `Retry-After` |
| Invitation capability | resolve, accept-new, accept-existing, issue, revoke, replace, list | global application body 256 KiB; capability parser and seven-day expiry bounds | 20 attempts per token-prefix digest and 120 global per 60 seconds; authenticated mutations also use the session-write budget | JSON `429` with `Retry-After` |
| Ordinary collaboration reads | Project, Channel, Thread, Message, Team, Work Item, repository, execution, account, and GitHub projections | global application body 256 KiB; pagination is endpoint-bounded | ordinary bounded reads remain available when another subsystem is exhausted | bounded JSON errors |
| Expensive browser reads | Project search, Work Item list/filter, assignee/reference search, mention search | search text 120 bytes/characters by endpoint; pages at most 50 or 100 | 120 requests per authenticated session per 60 seconds | JSON `429` with `Retry-After` |
| Browser mutations | every authenticated POST, PUT, PATCH, and DELETE domain operation | global application body 256 KiB; conversation bodies 128 KiB; domain field and idempotency limits apply before durable writes | 240 writes per authenticated session per 60 seconds | JSON `429` with `Retry-After` |
| Realtime | `GET /api/v1/realtime` WebSocket | client frame/message 4,096 bytes; server invalidation frame 70,000 bytes | one authorized session per connection; bounded subscriber buffers and refetch-on-lag | close/reconnect and durable refetch; collaboration reads stay healthy |
| MCP authentication | Streamable HTTP at `/api/v1/mcp` | 1 MiB MCP body; bearer token at most 128 bytes | 600 attempts per bearer digest and 900 global per 60 seconds before credential lookup | HTTP `429` with `Retry-After` |
| MCP authorized calls | all MCP read and write tools | schema field limits; result pages at most 50; no arbitrary database mutation | 600 requests per credential or OAuth-grant UUID per 60 seconds; writes retain optimistic versions and idempotency | HTTP `429`; ordinary tool conflicts remain structured tool results |
| MCP OAuth metadata | protected-resource and authorization-server metadata | no request body | no database or secret work | bounded public metadata |
| MCP OAuth ceremony | register, authorize/consent, token, refresh, revoke | body 64 KiB; client/state/redirect/token/verifier fields are individually bounded | 30 attempts per client/value digest and 240 global per 60 seconds; refreshes have their own budget of 300 per client and 1,200 global per 60 seconds, and failed refreshes 30 per client address, 60 per app and 600 global, so a stream of junk refresh tokens can't lock out working apps | OAuth `slow_down`, HTTP `429`, `Retry-After`, and `Cache-Control: no-store` |
| GitHub webhook | isolated listener on `127.0.0.1:3101` | body 1 MiB; strict unique headers, signature, event/action and repository checks | 300 total delivery attempts per 60 seconds before secret read, HMAC, JSON parsing, or database work | HTTP `429`; other failures use bounded non-disclosing webhook shapes |
| GitHub outbound/API jobs | installation token, repository sync, publish, PR, checks/reviews, automation | response body 1 MiB; child tokens are operation-scoped and discarded | one claimed job per 30-second lease; bounded attempts and serialized durable state | retry/backoff with bounded failure category |
| Git subprocesses | mirror refresh, inspection, worktree create/remove/repair, authenticated push | stdout 1 MiB, stderr 8 KiB, network timeout 90 seconds; argument arrays and cleared environments | four concurrent Git child processes plus repository-scoped metadata locks; no lock is held during provider execution | bounded `429` before spawn or non-secret Git conflict |
| Provider processes | Codex App Server supervisor and turns | JSONL line 1 MiB, stderr 256 KiB, buffered events 64, request timeout 120 seconds, shutdown timeout 5 seconds | Red Hat host limit two profiles/processes and one non-terminal run per Thread | actionable capacity conflict; collaboration reads remain healthy |
| Database | all durable operations | SQL parameters and bounded projections | pool maximum five connections on the private host | bounded unavailable response without leaking driver text or SQL values |
| Outbox and realtime publisher | committed invalidation rows | payload 65,536 bytes; error summary 2,000 characters | batches of 32, 30-second leases, at most 10 attempts; schema maxima are batch 100 and lease 300 seconds | durable retry/final bounded failure; no request-path blocking |
| Background scheduler/recovery | run admission, startup reconciliation, GitHub drain | leases 30 seconds; provider and Git bounds above | one process-local worker for each loop; stale leases are reclaimed deterministically | bounded state transition and actionable failure category |

No export endpoint or attachment upload exists in version one. Attachment
admission remains disabled. Any future entry point begins with the locked
25 MiB object and 2 GiB per-Project aggregate limits from the source of truth,
plus the M8 disk watermark, before it may be enabled.

## Operational interpretation

Rate-limit events log only the safe category and request ID already attached by
the HTTP layer. They do not log the limiter key. Exhausting login, OAuth, MCP,
webhook, provider, Git, or background capacity must not change readiness for
ordinary collaboration reads. `429` responses are bounded and include whole-
second `Retry-After` values wherever HTTP permits.

The application remains bound to `127.0.0.1:3100`, the webhook listener to
`127.0.0.1:3101`, and PostgreSQL to `127.0.0.1:55432`. Public ingress,
distributed counters, CAPTCHAs, IP reputation, billing quotas, model-usage
metering, and autoscaling are outside this private-beta contract.
