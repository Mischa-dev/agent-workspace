# Hosting shapes and what they cost idle

The same release runs in three shapes. They differ only in configuration:
which roles each process takes, where files live, whether a timer outside
drives the background work, and whether live connections are served by this
server or by a service implementing the live contract. Nothing in the code
names a provider; this page names none either.

## The pieces every shape uses

- **Three roles** (`AGENT_WORKSPACE_ROLES`, K5.7). `api` serves the app, its
  API, MCP and webhooks; `live` holds browsers' live connections; `work`
  runs the job queue, the event log's sequencer and consumers, agents'
  wake-ups and migrations. One process may hold all three (the default) or
  any of them. Stopping drains: readiness answers 503 at once, sockets close
  with 1012, leases are released.
- **The tick** (`POST /internal/work/tick`, K5.18). With
  `AGENT_WORKSPACE_WORK_LOOP=off` a `work` process does nothing by itself;
  an outside timer calls the tick with the token in
  `AGENT_WORKSPACE_WORK_TICK_SECRET_PATH`, and each call runs one bounded
  round (at most 25 seconds) and says whether more is due. An `api`-only
  process runs no migrations and answers within a second of starting.
- **Pooler rules** (K5.17, ruling R24). Settings are set only with
  `SET LOCAL` inside a transaction; there are no session advisory locks
  (leases instead), no hand-written `PREPARE`, and no transaction held
  across a network call. `LISTEN`/`NOTIFY` only makes things faster: with
  `AGENT_WORKSPACE_DATABASE_LISTEN_URL=off` everything still happens, on
  polls. Behind a transaction-mode pooler set
  `AGENT_WORKSPACE_DATABASE_STATEMENT_CACHE=0`.
- **The live contract** (`docs/contracts/live.md`, K5.19). Browsers' live
  connections can be served by any service that passes the contract's
  conformance suite. With `AGENT_WORKSPACE_LIVE_URL` set, one `work` process
  holding the short `live.publisher` lease tails the log and posts to it.
- **Nothing runs per workspace** (ruling R24 rule 6). No timer, loop,
  connection, cache or in-memory map exists because a workspace exists;
  schedulers query due rows. The idle guard below holds this.
- **Limits in shared storage.** Rate limits and plan limits are counted in
  PostgreSQL (`rate_limit`, `metering`), so any process can serve any
  request.

## One box (self-hosting)

One process with all three roles, PostgreSQL beside it, files on local
disk (`AGENT_WORKSPACE_FILE_STORE=local`). This is the container image's
default and what `docs/self-host/` describes.

Idle cost: one process and one database, always on. The database work an
idle server does is the same with one workspace or ten thousand (see the
guard below): the loops' own polling (lease renewals, the job queue, the
log's sequencer and consumers, replies), about 30 short transactions a
second.

## Small hosted

Two small machines, each running all three roles behind a load balancer;
managed PostgreSQL; files in S3-compatible object storage
(`AGENT_WORKSPACE_FILE_STORE=object`, with `files.redirect_downloads` on so
downloads go straight to storage); the static web app on a CDN
(`docs/self-host/static-web-app.md`).

Idle cost: two processes and one database, always on. Leases make sure
each piece of singular work (the built-in host, the log's sequencer, the
live publisher) runs on one machine at a time, so the second machine adds
only its own lease renewals and polls, not a second copy of the work.

## Edge

Designed and its contracts tested; built only when the numbers say so.

- The static app on a CDN.
- Files in zero-egress object storage, read through signed links
  (K5.12, K5.20).
- Live connections on a live service that hibernates idle sockets,
  implementing the live contract; one `work` process publishes to it.
- `api` in containers that stop when idle and answer within a second of
  starting (an `api`-only process starts cold and runs no migrations).
- PostgreSQL through a transaction-mode pooler (statement cache 0, listener
  off or pointed straight at the database).
- `work` with `AGENT_WORKSPACE_WORK_LOOP=off`, driven by an outside timer
  calling the tick.

Idle cost: the database, object storage at rest and whatever the timer
costs. No process has to stay awake: with no requests the containers stop,
idle sockets hibernate, and a tick that finds nothing due ends in
milliseconds.

## The idle guard

`tests/integration/idle_cost.rs` starts a full server (all three roles, its
loops running) on one workspace and counts what the database does while
nobody uses it: transactions committed and rows read, from PostgreSQL's
own statistics for the test database, over a fixed window. It then adds
10,000 empty workspaces in one SQL batch, lets the server settle, counts
again over the same window, and fails if either count grows by more than
its margin. A loop that ran a query per workspace, or scanned every
workspace on each pass, fails it.

It measures database work rather than CPU time and memory, because the
test shares its process with the rest of the suite, where those numbers
are noise; the database is where idle work shows up first and costs most
on a managed or pooled database. It doesn't need `pg_stat_statements`.

What it measured when it was added (debug build, CI-sized machine; the
numbers are per minute idle):

| | Transactions | Rows read |
|---|---|---|
| One workspace | 1,900 to 1,950 | 32,000 to 39,000 |
| 10,001 workspaces | 1,950 to 2,100 | 21,000 to 88,000 |

Three runs, each counting 20 seconds, scaled to a minute. The rows read
move with what the polls happen to find, not with the number of
workspaces; the test's margins are 1.5 times plus 50 transactions and
twice plus 40,000 rows per 20 seconds. A loop that did one query per
workspace would add 10,000 transactions on its first pass.

The polling itself is the next thing to make cheaper for the always-on
shapes: most of those transactions are loops finding nothing due. In the
edge shape they don't run at all between ticks.
