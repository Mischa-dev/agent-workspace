# Private workspace v0

This implementation follows planning v0.2, with the September 21 implementation authorization taking precedence. A substantive owner request renews a six-turn autonomous burst; agent activity never renews it. Checkpoints A, B and C are sequential checks, not approval stops. AgentOS integration and GitHub publishing remain unqualified.

## Processes and ownership

One pnpm application: React/Vite browser, Fastify HTTP server, and a separate Node worker sharing domain modules. Node 24 is pinned. The built server serves UI, commands and SSE from `127.0.0.1:4310`. PostgreSQL 18 has a dedicated cluster on port 55439. Drizzle owns schema/migrations; pg-boss owns durable wakeups. Database records, not queue delivery, decide whether a run may dispatch.

`packages/shared` owns transport types. `apps/server/src/domain` owns checked commands, context selection, routing, approval and claims. `apps/server/src/connectors` translates scoped requests to runtimes. `apps/server/src/storage` owns immutable byte storage. `apps/web` owns interaction and rendering. Root is integration owner; at most two coding workers operate concurrently after interfaces are established.

## Data and consistency

PostgreSQL holds workspace records, immutable Resource text revisions, messages, response requests, context manifests, execution receipts, command deduplication and an ordered event log. Domain changes and events commit together. SSE uses durable cursors and snapshot reconciliation; reconnect never dispatches work. Uploaded bytes live in a private configurable directory outside static assets, addressed by opaque keys with checksums. Metadata is committed after atomic file finalization; recovery quarantines orphans and reports missing files. Attachments pin immutable revisions. Revision edits use optimistic concurrency.

## Trust boundaries

Owner login uses a local bootstrap credential and an HttpOnly same-site cookie. Unsafe HTTP commands require allowed Host/Origin, JSON content type and a CSRF header. The server binds only loopback. Scoped connector tokens bind actor, request, mode and expiration; tools invoke the same domain authorization as UI commands. Read access is the intersection of conversation membership and request context. Resources have independent grants. Ordinary chat has no host/repository tools or uncontrolled memory; isolated sessions alone are insufficient. Independent opinions use a frozen snapshot and private outputs until publication.

A replaceable Codex app-server adapter uses existing supported authentication on this host. Builtin file, shell, network, app, skill and memory access must be disabled and verified for chat. If this cannot be qualified, chat dispatch fails closed. Code execution is a separate scoped profile against an explicitly approved disposable/local checkout; repository paths and credentials never come from a chat instruction.

## Startup and recovery

Setup creates ignored configuration, dedicated PG data, migrations and owner bootstrap access. Development runs Vite plus HTTP and worker; built startup runs HTTP and worker with static UI. PID files, health endpoints and log files support shutdown and diagnosis. Shutdown closes admission and interrupts active work. Startup marks uncertain dispatched work for reconciliation; it never retries an external call solely because a lease expired. A worker crash preserves requests, receipts and files. Durable stop is checked before every tool operation and new admission.

Work sessions freeze approved task revisions, allowed repository and limits. Claims and reservations are transactional. Two workers and finite deadlines are hard admission bounds. Dependencies require accepted evidence and, for code, verified integration into the approved baseline. Dirty checkouts are preserved. GitHub is an adapter with authorized reads and fixture-tested write contracts; this run never pushes or opens PRs.

Backup stops dispatch and captures both PG and files. Restore starts with dispatch disabled, uncertain runs held, and consistency checks required. No automatic replay of backed-up work.
