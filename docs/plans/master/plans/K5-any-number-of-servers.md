# K5: Any number of servers

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Plan for workstream K5 of the Agent Workspace master plan. Written 2026-11-14 against main at
`fe95a74`. Read-only research: nothing was built or run. Paths are relative to
`/home/user/agent-workspace` unless they start with `/home/user/`.

## 1. Goal

Today the server is correct only as one process: live updates reach only the browsers on the
process that happened to claim an event, a second process starting up would take over and
interrupt the first one's running agents, and every limit is counted separately in each
process. When K5 is done, two or more copies of the same program run against one database
and act as one: a message sent through one copy appears in a browser connected to another,
exactly one copy drives the built-in host and the scheduler, sign-in throttles hold across
all copies, files live on disk or in any S3-compatible store, and a deploy restarts one copy
at a time without anyone noticing. A small install still runs one process with no extra
services.

## 2. Where the code is today

### 2.1 Process shape

`apps/server/src/main.rs` is one binary. After the CLI subcommands it runs two legacy
preflights (`main.rs:42-60`), `sqlx::migrate!` (`main.rs:63`), provisions default workflows
(`main.rs:67`), builds `AppState` (`main.rs:323-330`; the struct is `lib.rs:133-139`:
`config, pool, rate_limits, realtime, supervisors`), reconciles every non-terminal agent run
(`main.rs:343-357`), encrypts old webhook secrets (`main.rs:362`), binds two HTTP listeners
(application and GitHub webhooks), and spawns six loops: harness reloader (`main.rs:391`),
run scheduler (`:392`), GitHub job worker (`:397`), outbox publisher (`:399`), reply worker
(`:407`) and job workers (`:413`). If the scheduler, the GitHub worker or the publisher
returns, the whole server shuts down (`main.rs:451-525`), and the scheduler and GitHub worker
return on any database error (`scheduler.rs:81-92`, `github.rs:142-150`).

The database connection pool defaults to 5 (`config.rs:231`) and the production guide uses 5
(`docs/self-hosted-red-hat.md:210`). The production unit is one systemd service with
`Restart=on-failure` (`deploy/redhat/agent-workspace.service`); a release is switched by
symlink and `systemctl restart` (`deploy/redhat/deploy-release.sh:90-94`), so every deploy is
a short outage and interrupts local agent runs.

### 2.2 Every piece of state held in one process's memory or on its local disk

Checked in the code. "With two processes" says what goes wrong; the last column says which
slice fixes it.

| State | Where | With two processes | K5 slice |
| --- | --- | --- | --- |
| Realtime hub: a `tokio::broadcast` channel of capacity 256 | `realtime.rs:130-169`, `main.rs:323` | Each outbox event is claimed once with `FOR UPDATE SKIP LOCKED` (`outbox.rs:942-955`) by one process's publisher, broadcast to that process's sockets only (`realtime.rs:701`, `:304`). Browsers on the other process never see it and are not told to refetch. | K5.4, K5.5 |
| Per-event, per-socket authorization: a session query and a membership plus `conversation_visible_to` query for every event of every workspace | `realtime.rs:384-426`, `:483-560` | Cost is sockets times events across all tenants, on a 5-connection pool | K5.8 |
| Ephemeral agent text streaming (`message.delta`) | `provider.rs:785-795` via `RealtimeHub::publish` (`realtime.rs:146-156`) | Reaches only the run's own process | K5.5 |
| Session revocations | `realtime.rs:133, 158-168`; called from `auth/mod.rs:371, 417`, `auth/workos.rs:676`, `password_reset.rs:875`, `account_security.rs:230, 281, 390` | Logout on A leaves the socket on B open until the 15 s revalidation (`realtime.rs:30, 462-478`) | K5.5 |
| Typing and presence of people | Not built. Agent "presence" is derived from database state (`apps/web/src/agents/presenceModel.ts:5-13`); agents' working line is the durable reply progress | Nothing to fix yet; must be designed for many processes before it is built | K5.10 |
| Live document rooms (pages, sheets, whiteboards) | **None in memory.** A page's Yjs document is stored in the database and every write locks the page row (`page_docs/store.rs:1-18`); browsers send updates and sync by HTTP (`page_docs/http.rs:1-15`, `:186-268`) every 400 ms (`apps/web/src/memory/live/pageSync.ts:64`) and hear `page.updated` over realtime. Whiteboards and sheets also keep their state in the database and merge per element or per cell (`whiteboards.rs:4-10`, `sheets.rs:10-18`) | Already correct across processes once `page.updated` reaches every process. Cost: each update reloads and re-encodes the stored document | K5.5 (correct), K5.10 (fast path), K5.16 (later) |
| Rate limiters and sign-in throttles: 27 fixed-window maps and a "recent OAuth clients" map, all `Arc<Mutex<HashMap>>` | `rate_limit.rs:76-106`, `:432-527`, `:529-560` | Each process grants the full budget; counters reset at restart (`docs/operations/abuse-controls.md:4`) | K5.6 |
| Password hashing semaphore (2 at once) | `rate_limit.rs:130-134` | Per machine, which is what it protects; correct as is | kept, stated |
| Per-address limits read a hard-coded CDN header | `rate_limit.rs:420-425` (`cf-connecting-ip`) | Not a multi-process issue, but names one provider in core code | K5.6 |
| Provider device-code sign-ins | `provider/registry.rs:109`, `:411-500` (`Mutex<HashMap<Uuid, LoginCeremony>>`) | The follow-up poll landing on the other process fails | K5.14 |
| Startup run recovery with no owner check | `recovery.rs:29-57`: clears every queued run's scheduler claim (`:33-45`), then moves every `starting/working/blocked/recovering` run to `recovering` and resumes or interrupts it | A second process starting (or a rolling deploy) takes over or kills runs the first is still driving | K5.3 |
| Worktree and approval reconciliation at startup | `recovery.rs:249`, `:284`, called at `main.rs:349-357` | Same as above | K5.3 |
| Run driving: a `tokio::spawn` per turn holding a child process or cloud follower | `provider.rs:415-450`, slots `provider/slots.rs:35`, process pools `provider/codex_app_server.rs:138`, `provider/codex_cloud.rs:389`, `provider/claude_cloud.rs:181` | A run lives and dies with its process. Host-local by nature until A1 moves the built-in host out | K5.3 (one holder) |
| Scheduler "kick" from request handlers | `scheduler.rs:66-75`, called at `api.rs:511, 759`, `external_execution.rs:822` | Admits and starts a run on whichever process served the request | K5.3 |
| Host-wide run cap counts every local run in the database | `scheduler.rs:263-274` (`codex_host_concurrency_limit`, `config.rs:244`) | Assumes one machine | K5.3 |
| Repository clone and refresh | `repositories.rs:448` (`tokio::spawn`, no job, no lease) | A crash leaves it pending; on two hosts the mirror lands on the wrong disk | K5.3 |
| Git mirrors, worktrees, provider homes on disk under the data root | `config.rs:232-242`, `repositories.rs:274`, `provider/secret_fd.rs:38` (temp file) | Host-local; must stay with the one built-in host | K5.3 (A1 owns the move) |
| Uploaded files and link preview images on local disk | `files/store.rs:20-42` (the `FileStore` enum has one variant), callers `files.rs:402`, `files/http.rs:282`, `link_previews.rs:302-323`, `mcp/file_tools.rs:306`, `mcp/sheet_tools.rs:1314` | Every process needs the same disk | K5.11-K5.13 |
| Uploads buffered whole in memory (up to 100 MB) | `files.rs:46`, `:349-403`; route limit `files/http.rs:36` | Memory per upload; not a correctness issue | K5.11 |
| Module on/off cache (10 s TTL) and project-to-workspace map | `modules/gate.rs:46-64` | `forget` (`:60`) clears only the local copy, so other processes are stale for up to 10 s | K5.5 |
| Custom harness registry reloaded every 30 s | `harnesses.rs:130-140` | Bounded staleness; only needed where runs are driven | K5.7 |
| MCP sessions | `mcp.rs:4087-4175` (`ResumeAnySession`), `:4208-4217` | Any process resumes any session already. Only the "ended with DELETE" list is per process (`mcp.rs:4101-4127`) | K5.7 |
| Secrets keyring file and its cache | `secrets.rs:436-480` | Every host needs the same key file; the `KeyEncryption` seam exists | H1 |
| Metrics registry, alert de-duplication | `metrics.rs:57`, `:161-174` | Per-process numbers; acceptable with an instance label | K5.7 |
| Image processing semaphore | `files/image.rs:78-80` | Per machine; correct as is | kept |
| Backup record read from the data root | `server_status.rs:140` | Read-only, host-local | H1 |
| Startup one-off tasks: legacy preflights, workflow provisioning, secret migration | `main.rs:42-67`, `:362` | Two processes run them at the same time | K5.3 |

Multi-process safe already: the job queue (`jobs.rs:538-600`: `SKIP LOCKED`, lease token,
renewal; recurring jobs deduplicated by `unique_key`, `jobs.rs:515-536`), the outbox claim
(`outbox.rs:911-975`), scheduler claims under an advisory transaction lock
(`scheduler.rs:240-258`), GitHub jobs, agent webhook deliveries and reply leases (per the
G report, F4). Sign-in state is already in the database: browser sessions
(`migrations/0004_m1_identity_projects.sql:69`), two-step sign-in challenges
(`20261016200100_account_security.sql:53`), WorkOS sign-in attempts
(`0040_member_accounts.sql:59`), and MCP OAuth codes, grants and tokens
(`0029_m6_mcp_oauth.sql:4-90`). Run interruption is read from the database by the driving
loop (`provider.rs:878-887`), so a stop pressed on another process works.

### 2.3 What has the right shape and must be kept

- **Events carry ids, not text, and the browser refetches over HTTP.** Correctness comes from
  refetching, so a missed event costs a refetch, not a wrong screen (B report, "keep that
  contract"). The web client already de-duplicates by event id
  (`apps/web/src/realtime/useRealtimeConnection.ts:69-79`) and refetches on every reconnect
  (`:272-281`).
- **The outbox claim protocol** (`outbox.rs:911-1060`) for side effects that must happen once
  (notifications, webhook and automation routing). K3 replaces the outbox with its event log
  and consumers; K5 does not change who does side effects.
- **The job queue** (`jobs.rs`). New background work goes there, and K5 adds "host-scoped"
  kinds rather than another loop.
- **Bounded frames and the Origin check** on the WebSocket (`realtime.rs:32-34`, `:331-333`,
  `auth::origin_is_allowed`).
- **The `FileStore` seam** with opaque keys made of workspace and file ids (`files/store.rs:1-5`,
  `files.rs:400`), and the per-workspace quota lock (`files.rs:388-392`).
- **The documents' "database is the room" design**: one row lock per write, merge by the
  document's own rule. It is already multi-process correct.

### 2.4 What is in the way

- `event_cursor` is `GENERATED ALWAYS AS IDENTITY` (`migrations/0005_m2_conversations.sql:523`):
  numbers are handed out when a row is inserted, not when its transaction commits. A reader
  that tails "everything after the last number I saw" can skip an event whose transaction
  started earlier and committed later. The publisher avoids this today only because it claims
  "unpublished" rows instead of tailing. Any cross-process tail needs a commit-ordered number.
- The publisher both broadcasts and acknowledges side effects (`realtime.rs:675-764`), so the
  broadcast is welded to "claimed once".
- `RealtimeHub`, `RateLimiters` and `SupervisorRegistry` are built inside `main` and copied
  into each test by hand (`lib.rs:370-377`, `tests/integration/m1_realtime_websocket.rs:56-65`).
  There is no way to start two complete server instances in one test.
- NOTIFY channels are database-wide while each integration test uses its own schema in one
  database (`tests/integration/m1_realtime_websocket.rs:35-44`), so channel names must carry
  the schema.

## 3. The target design

### 3.1 Words used in this plan

- **Instance**: one running copy of the server program, with a fresh id at start.
- **Role**: what an instance does (ruling R24 rule 4): `api` (HTTP and MCP, stateless, may
  be stopped when idle and answers within a second of starting), `live` (browser sockets and
  agent streams, nothing else) and/or `work` (jobs, consumers, deliveries, indexing, the
  scheduler, the built-in host). Default all three in one process.
- **Lease**: a named right, held by one instance at a time, that expires unless renewed.
- **Position**: an event's place in the log, dense and in commit order. The browser's and the
  agent's cursor.
- **Wake**: a PostgreSQL notification that says "look at the log". Never the truth.
- **Tail**: one reader per instance that follows the log by position and hands events to that
  instance's connections.
- **Subscription set**: what one connection may hear, computed when it connects and when
  access changes.
- **Ephemeral frame**: a signal never stored in the log (typing, presence, cursors, streaming
  text, session revoked, cache invalidation).

### 3.2 Instances and leases

**Tables** (migration `apps/server/migrations/<ts>_server_instances_and_leases.sql`). Both are
server-level, not workspace rows, so they carry no `workspace_id`.

```sql
CREATE TABLE server_instances (
    id UUID PRIMARY KEY CHECK (substring(id::text FROM 15 FOR 1) = '7'),
    roles TEXT[] NOT NULL CHECK (
        cardinality(roles) BETWEEN 1 AND 3 AND roles <@ ARRAY['api', 'live', 'work']::text[]
    ),
    label TEXT CHECK (label IS NULL OR char_length(label) BETWEEN 1 AND 80),
    release TEXT NOT NULL CHECK (char_length(release) BETWEEN 1 AND 80),
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    draining_at TIMESTAMPTZ,
    stopped_at TIMESTAMPTZ
);
CREATE INDEX server_instances_live ON server_instances (heartbeat_at) WHERE stopped_at IS NULL;

CREATE TABLE leases (
    name TEXT PRIMARY KEY CHECK (
        char_length(name) <= 160 AND name ~ '^[a-z][a-z0-9_.-]*(:[a-z0-9_.-]{1,120})?$'
    ),
    -- No foreign key: the row must outlive its holder so `token` keeps increasing.
    holder_instance_id UUID NOT NULL,
    token BIGINT NOT NULL CHECK (token > 0),
    acquired_at TIMESTAMPTZ NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    CHECK (expires_at >= acquired_at)
);
CREATE INDEX leases_holder ON leases (holder_instance_id);
```

`label` is an operator-set name (`AGENT_WORKSPACE_INSTANCE_LABEL`), never derived from the
machine. Stopped instances are deleted by the retention job after 7 days.

**Rules.**

- All times are the database's clock (`now()`), never the instance's, so clock skew between
  machines doesn't matter.
- Default time to live 15 s. Each instance renews every lease it holds in one statement every
  5 s, together with its heartbeat. Taking over is allowed only after `expires_at`.
- **Self-fencing**: if an instance has not renewed successfully for 10 s (two thirds of the
  time to live, measured on its own monotonic clock from the start of the last successful
  renewal), it treats every lease as lost and stops the work under them before anyone else may
  take over.
- **Fencing tokens**: `token` grows by one each time a different instance takes a lease. A
  write that must only happen under a lease checks the lease row in its own transaction with
  `FOR SHARE`, so a takeover waits for that transaction and a holder that lost its lease fails
  the write.
- **Graceful release** sets `expires_at = now()` and sends a wake on the lease channel, so a
  standby takes over at once (this is what makes a deploy smooth).

**SQL the module runs.**

```sql
-- try_acquire($name, $instance, $ttl_seconds) -> token, or no row when held by another
INSERT INTO leases AS l (name, holder_instance_id, token, acquired_at, expires_at)
VALUES ($1, $2, 1, now(), now() + make_interval(secs => $3))
ON CONFLICT (name) DO UPDATE SET
    token = CASE WHEN l.holder_instance_id = EXCLUDED.holder_instance_id AND l.expires_at > now()
                 THEN l.token ELSE l.token + 1 END,
    acquired_at = CASE WHEN l.holder_instance_id = EXCLUDED.holder_instance_id
                            AND l.expires_at > now()
                       THEN l.acquired_at ELSE now() END,
    holder_instance_id = EXCLUDED.holder_instance_id,
    expires_at = EXCLUDED.expires_at
WHERE l.expires_at <= now() OR l.holder_instance_id = EXCLUDED.holder_instance_id
RETURNING token;

-- heartbeat($instance, $ttl_seconds): one statement renews everything this instance holds
WITH beat AS (
    UPDATE server_instances SET heartbeat_at = now()
    WHERE id = $1 AND stopped_at IS NULL RETURNING id
)
UPDATE leases SET expires_at = now() + make_interval(secs => $2)
WHERE holder_instance_id = (SELECT id FROM beat) AND expires_at > now()
RETURNING name, token;

-- fence, inside the caller's transaction
SELECT 1 FROM leases
WHERE name = $1 AND holder_instance_id = $2 AND token = $3 AND expires_at > now()
FOR SHARE;

-- release
UPDATE leases SET expires_at = now()
WHERE name = $1 AND holder_instance_id = $2 AND token = $3;
```

The heartbeat compares the returned names and tokens with what the instance believes it
holds; anything missing is lost and its work is cancelled.

**Rust API** (new module `apps/server/src/cluster/`, files `mod.rs`, `instance.rs`,
`leases.rs`). `AppState` gains `cluster: cluster::Cluster`.

```rust
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct InstanceId(pub Uuid);

#[derive(Clone, Copy, Debug)]
pub struct Roles { pub api: bool, pub live: bool, pub work: bool }

/// A lease's name. Kinds are listed here so names can't drift.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum LeaseName {
    BuiltinHost,                    // "builtin-host"
    StartupMaintenance,             // "server.startup" (the name H1 uses)
    Consumer(&'static str),         // "consumer:<name>"   (K3, A2)
    Partition(&'static str, u16),   // "partition:<name>.<n>"
    Document(DocumentRef),          // "document:<kind>.<uuid>" (K5.16, later)
}

#[derive(Clone)]
pub struct Lease {
    pub name: LeaseName,
    pub token: i64,
    /// Cancelled when the lease is lost or released.
    pub lost: tokio_util::sync::CancellationToken,
}

#[derive(Clone)]
pub struct Cluster { /* pool, instance id, roles, ttl, held leases, shutdown */ }

impl Cluster {
    pub async fn register(pool: PgPool, roles: Roles, label: Option<String>) -> Result<Self, AppError>;
    pub fn instance_id(&self) -> InstanceId;
    pub fn roles(&self) -> Roles;
    pub async fn try_acquire(&self, name: LeaseName) -> Result<Option<Lease>, AppError>;
    pub async fn release(&self, lease: &Lease) -> Result<(), AppError>;
    /// Inside a transaction: fails with `AppError::LeaseLost` unless `lease` is still held.
    pub async fn fence(&self, tx: &mut Transaction<'_, Postgres>, lease: &Lease) -> Result<(), AppError>;
    pub fn holds(&self, name: &LeaseName) -> bool;
    /// The heartbeat loop: renews, self-fences, marks draining and stopped.
    pub async fn run_heartbeat(&self, shutdown: watch::Receiver<bool>);
    pub async fn mark_draining(&self) -> Result<(), AppError>;
    pub async fn mark_stopped(&self) -> Result<(), AppError>;
}

/// Runs `work` whenever this instance holds `name`. Tries again every 5 s and on a
/// lease-released wake. `work` must return soon after `lease.lost` is cancelled.
pub async fn while_held<F, Fut>(cluster: Cluster, name: LeaseName,
    shutdown: watch::Receiver<bool>, work: F)
where F: Fn(Lease) -> Fut + Send + Sync + 'static,
      Fut: Future<Output = Result<(), AppError>> + Send;
```

**What uses a lease** (and what deliberately doesn't):

| Work | Mechanism | Why |
| --- | --- | --- |
| Built-in host: run scheduler, run recovery, worktree reconcile, approval clean-up, repository preparation, provider sign-ins | Lease `builtin-host` | Child processes and disk are host-local; exactly one instance may drive them. It is the only built-in host lease: A1 has no per-run leases, and A1.9 adds only the `fail_run` fix and `SKIP LOCKED` claims under it |
| Startup one-off tasks (`main.rs:42-67`, `:362`) | Lease `server.startup`, waited for (up to 60 s) by `work` only; `api` never waits for it, so it answers within a second of starting | Two instances starting together must not run them twice |
| Event sequencer (3.4, defined by K3) | The row lock on K3's `event_position_counter`, no lease | A lease would pause live updates for up to 15 s when its holder dies |
| Job workers, outbox side-effect publisher, reply worker, GitHub jobs | Existing `SKIP LOCKED` claims | Already safe; a lease would only add a single point of failure |
| Recurring jobs | Existing `unique_key` (`jobs.rs:515-536`) | Already safe |
| K3's ordered consumers, A2's per-connection delivery when order matters | `consumer:<name>` or `partition:<name>.<n>` | K5.2 switches K3's consumers from their per-batch cursor-row lock to `consumer:<name>`; A2 chooses |
| Document rooms (later) | `document:<kind>.<id>` | K5.16 |

Migrations at start: `sqlx::migrate!` already takes a database lock while it runs, so two
instances starting together run each migration once. The legacy preflights and other
one-off start tasks move under `server.startup`. Under a transaction pooler (R24 rule 3)
migrations run from the `work` role or the `migrate` command over the direct (listen) URL,
never from `api`.

### 3.3 The built-in host behind one lease, and the recovery fix

Until A1 moves the built-in host into its own program, it lives inside the `work` role and
is driven by exactly one instance at a time.

```rust
// server.rs (new app assembly, see K5.1), work role, when AGENT_WORKSPACE_BUILTIN_HOST=on
cluster::while_held(cluster.clone(), LeaseName::BuiltinHost, shutdown.clone(), move |lease| {
    let state = state.clone();
    async move {
        recovery::reconcile_owned(&state, &lease).await?;     // was main.rs:343-357
        let result = tokio::select! {
            result = scheduler::run(state.clone(), lease.clone()) => result,
            () = lease.lost.cancelled() => Err(AppError::LeaseLost),
        };
        // Losing the lease stops every local turn before anyone else may recover them.
        state.supervisors.shutdown_all().await;
        result
    }
}).await;
```

Changes, each tied to today's code:

1. `recovery.rs:33-45` (clear every queued run's scheduler claim) is deleted. `claim_one`
   already clears lapsed claims (`scheduler.rs:242-252`); a live claim belongs to whoever
   holds it.
2. `reconcile_non_terminal_runs` (`recovery.rs:29`) becomes `reconcile_owned(state, lease)`
   and is called only by the lease holder, right after it acquires the lease (and so also when
   it takes over from a crashed holder, not only at boot). Each transition to `Recovering`
   (`recovery.rs:62-71`) calls `cluster.fence(&mut transition, lease)` first. Same for
   `reconcile_registered_worktrees` (`recovery.rs:249`) and `cancel_terminal_run_approvals`
   (`recovery.rs:284`).
3. Admission (`scheduler.rs` `claim_one` and `admit`) fences in its transaction, so a holder
   that lost the lease cannot start a run.
4. `scheduler::kick` (`scheduler.rs:66-75`) drains only when `cluster.holds(BuiltinHost)`;
   otherwise it sends a wake on the host channel, which the holder listens for. The 1 s tick
   stays as the safety net.
5. The host-wide cap (`scheduler.rs:263-274`) stays as it is: with one holder, "local runs in
   the database" are exactly this host's runs.
6. Repository preparation (`repositories.rs:448`) becomes a job of a new **host-scoped** kind
   (`repositories.prepare`). `JobRegistry` gets `register_host(kind, handler)`; ordinary
   workers skip host kinds, and the lease holder claims them with the existing `claim`
   (`jobs.rs:538`) filtered to host kinds. This fixes "a crash leaves the repository pending".
7. Provider device-code sign-ins move to a table (K5.14), driven by the holder.
8. The scheduler and GitHub loops log and continue on database errors instead of stopping the
   server (G report F3): done by P0.2; here only what goes beyond it (nothing).

On a crash restart the old instance's lease is still unexpired, so the new one waits up to
15 s before recovering; on a clean stop the lease is released and recovery starts at once.

### 3.4 Live updates across processes

K3 owns the event log. K5 owns getting each new event to every instance's browsers and agent
streams. K3.2 mirrors every outbox event into `events`, so the tail reads `events` from its
first day; there is no outbox-backed tail and no switch.

#### Positions: avoiding the sequence gap, together with K3

The problem (section 2.4): transaction A inserts event 10, transaction B inserts event 11 and
commits first; a tailer reads "after 9", sees 11, moves on, and never sees 10.

Three ways out, and the choice:

| Way | How | Cost | Verdict |
| --- | --- | --- | --- |
| Serialize all event inserts | One lock for every writer of an event | Kills write concurrency | No |
| Transaction-id frontier | Store the writer's transaction id; read only rows from transactions older than the oldest still running | No extra write; but one long transaction (the agent webhook loop holds one for up to about 80 s, B report finding on `replies.rs:1315-1440`) stalls every live update | No |
| **Sequencer stamp** | Rows are inserted without a position. A short step stamps committed, unstamped rows with the next dense positions in insert order, holding a row lock on a one-row head so stamps commit strictly one after another | One small update per event batch; a few milliseconds of latency | **Yes** |

**The design is K3's (ruling R8).** The sequencer stamp is chosen, and K3 §3.3.1 is its only
definition: writers stage rows in `event_staging`; one sequencer transaction takes a batch with
`FOR UPDATE SKIP LOCKED`, advances `event_position_counter` (whose row lock is held to commit,
so positions become visible strictly in order and are dense), inserts into `events` and
notifies `realtime_channel('events')`. The `realtime_channel`, `wake_event_log` functions and
the `event_staging_wake` trigger are created by K3.2. K5's earlier design for this section
(an `outbox_events.position` column, an `event_log_heads` row with a floor, a three-statement
stamp over `outbox_events`) is dropped: K3.2 mirrors every outbox event into `events`, so the
tail reads `events` from its first day and there is one log, one envelope and one sequencer.
K5 takes from its old design what K3 adopted: the cheap pre-check before the lock, `SET LOCAL
lock_timeout = '2s'`, a sequencer every instance may run (no lease), and a gap treated as a
bug. A2's agent cursor and the browser's resume cursor are both `events.position`.

#### The wake

Each instance opens one dedicated listening connection (`sqlx::postgres::PgListener`, not
from the pool) and listens on four channels: `log` (run the sequencer), `events` (run the
tail), `ephemeral` (3.6) and `control` (host kicks, lease released, cache invalidation).

- **Payload limit**: PostgreSQL caps a notification at 8000 bytes. Wakes carry at most a
  position number. Ephemeral frames are batched up to 7,500 bytes and split above that.
- **Missed notifications**: notifications are lost while a listener is disconnected.
  `PgListener::try_recv` returns `None` when the connection drops; the wake task then runs the
  sequencer and the tail immediately after it reconnects. The tail also polls every 1 s and the
  sequencer every 500 ms, so a lost wake costs at most that much delay, never an event.
- **Reconnects**: exponential back-off from 100 ms to 5 s. While the listener is down the
  instance reports itself not ready (3.10).
- **Connection poolers** (ruling R24 rule 3: LISTEN/NOTIFY is only an optimisation).
  LISTEN needs a session connection. `AGENT_WORKSPACE_DATABASE_LISTEN_URL` (optional,
  defaults to the main URL) lets an operator point the listener straight at PostgreSQL when
  the main URL goes through a transaction-mode pooler. When it is set to `off`, or the
  listener can't connect, the instance runs on the polls alone (sequencer 250 ms, tail
  250 ms, `control` channel replaced by polling `server_instances`/`leases` every 1 s) and
  stays ready: no feature depends on a notification arriving. Ephemeral frames (3.6) then go
  through the `live` contract (3.10a) instead of NOTIFY. The cluster test suite runs once
  with LISTEN off.
- **Commit cost**: a transaction that notifies takes a database-wide lock briefly at commit.
  The trigger is per statement and identical notifications in one transaction are merged, so
  this is one notification per writing transaction. Monitored with
  `pg_notification_queue_usage()` (exported as a metric).

#### The tail and back-pressure

One tail per instance that runs the live role in process (3.10); with a separate live service
the tail runs on the `work` instance holding `live.publisher` and publishes through the
contract (3.10a). It reads with K3's `events::tail(conn, after, limit)`
and gets K3's `events::LoggedEvent`, whose envelope fields (workspace, space, thing, event
kind, actor, credential, requested by, run, request id, version, changes) and `Audience`
(workspace, space, conversation with root and restricted, account, thing) are defined once in
K3 §3.3.2. K5 defines no second envelope or audience type; the browser's wire frame is built
from `LoggedEvent` (with the event's `legacy_type` as `type` until W1 moves the client).

- The tail starts at the head when the instance starts (nobody to replay to).
- Wakes are coalesced: at most one read in flight, at most 20 reads a second, 500 events a
  read; if more remain it reads again at once.
- Each event is serialized **once** per instance (`Arc<str>`) and offered to the matching
  connections with a non-blocking `try_send` into each connection's bounded queue (512
  frames). The tail never waits for a socket.
- A connection whose queue is full gets the existing "lag" treatment: close code 1012 with
  "realtime lag; reconnect and refetch" (`realtime.rs:31`, `:891-897`), which the web client
  already handles. Ephemeral frames are dropped first, once a queue is three quarters full.
- If the tail itself falls more than 10,000 positions behind (a stalled instance), it jumps to
  the head and sends `resync` to every local connection rather than replaying a backlog.

**Database connections**: one listening connection per instance plus short pool use: per
wake, one sequencer transaction and one tail query, coalesced to at most about 40 short
queries a second per instance under load, independent of the number of sockets. Today it is
two queries per event per socket. K5.7 raises the default pool from 5 to 10 and documents
sizing; the listener is outside the pool.

**The publisher keeps side effects only.** `publish_ready_outbox_batch` (`realtime.rs:675-764`)
stops broadcasting; it claims, writes notifications, queues routing and acknowledges, as now.
K3 replaces it with consumers; P0 or K3 fixes its "acknowledged even when notification failed"
bug (G report F2).

#### Why not a broker yet

A message broker (a pub/sub server or a streaming log) would carry wakes and ephemeral frames
with more throughput and no commit-time lock. It is not worth it yet: the durable truth stays
in PostgreSQL either way, so a broker only replaces the wake; it adds a service every
self-hoster must run, secure, back up and monitor, against principle 7's "a small install runs
one process"; and the load here (tens of events a second per workspace, a handful of
instances) is far inside what NOTIFY handles. The wake and the ephemeral bus sit behind one
trait (`realtime::wake::WakeTransport`), so a broker can replace NOTIFY in one module. Revisit
when any of these is seen: notification queue usage above 10 %, commit latency caused by the
notify lock, more than about 20 live instances, or ephemeral traffic (cursors, streams) above
about 2,000 frames a second.

### 3.5 Subscriptions

#### What a connection may hear

Access is decided when a connection subscribes and when access changes, not per event.

```rust
pub struct SubscriptionSet {
    pub account_id: Uuid,
    pub workspaces: BTreeSet<Uuid>,             // a browser: the current one (R20); an agent: every one its credential reaches (R19)
    pub spaces: BTreeSet<Uuid>,                 // today: project ids, all kinds
    pub restricted_conversations: BTreeSet<Uuid>, // root conversations it is a member of, DMs included
    pub direct: bool,                           // hears the direct scope (DMs and groups, R20); an agent's subscription says
    pub unread_only: BTreeSet<Uuid>,            // other workspaces: unread counts only, for the switcher (R20)
    pub documents: BTreeSet<DocumentRef>,       // live documents it is watching (3.7)
}
// The audience of an event is K3's `events::Audience` (Workspace, Space, Conversation { root,
// restricted }, Account, Thing), carried on every `LoggedEvent` (ruling R8). K5 defines none.

pub async fn compute(pool: &PgPool, account_id: Uuid) -> Result<SubscriptionSet, AppError>;
```

Today's queries, which become K1's access filter once it exists:

```sql
-- spaces: same rule as today's per-event check (realtime.rs:520-541)
SELECT pm.project_id, p.organization_id
FROM project_memberships pm
JOIN projects p ON p.id = pm.project_id AND p.archived_at IS NULL
JOIN organization_memberships om
  ON om.organization_id = p.organization_id AND om.actor_id = pm.actor_id AND om.enabled
WHERE pm.actor_id = $1 AND pm.enabled;

-- restricted root conversations (conversation_visible_to, 0037_direct_conversations.sql:160-181)
SELECT c.id FROM conversation_members m
JOIN conversations c ON c.id = m.conversation_id
WHERE m.actor_id = $1 AND c.parent_channel_id IS NULL AND c.visibility <> 'project'
  AND c.project_id = ANY($2);
```

**Routing an event** (in-memory, no database query): `Workspace` reaches connections
subscribed to the workspace; `Space` reaches connections subscribed to the space; `Conversation` reaches the space's connections if not restricted, otherwise only
connections with the root in `restricted_conversations`; `Account` reaches that account's
connections; `Thing` is the one exception that still checks per event, and only for the
connections of that workspace (today: workspace whiteboards, `realtime.rs:514-519`, using
`whiteboards::visible_to`). Every event type must map to one of these (a guard test, section 6).

The router needs `space`, `root` and `restricted` per event. K3's envelope carries them (the
writer knows them; `events.conversation_id` and `conversation_restricted`), so no lookup and no
per-instance conversation cache is needed. Direct-scope events (R20) are always
`Conversation { restricted: true }` or `Account`, so they reach participants only.

**Index**: `HashMap<ScopeKey, HashSet<ConnectionId>>` with `ScopeKey = Workspace | Space |
Conversation | Account | Document`. Delivery cost is the number of interested connections.

**When access changes**, processed by the tail in position order, before any later event is
routed:

| Event (today) | Effect on local connections |
| --- | --- |
| `project.membership.updated` (payload names the account, `outbox.rs:196-201`) | Recompute that account's sets |
| `conversation.member.removed` (names the account) | Drop the root from that account's sets |
| `conversation.created`, `conversation.updated` for a restricted conversation | One query for its members; add or drop it for local accounts |
| K1's `workspace.member.*`, `role.*`, `account.disabled`; K4's `space.member.*`; M1's module switch events | Recompute the named account, or every local connection in that workspace for role and module changes |
| Session revoked (ephemeral, 3.6) | Close that session's connections |

A recompute that fails closes the affected connections (they reconnect and compute afresh):
fail closed. Backstops: session validity is checked for all of an instance's connections in
one batched query every 15 s (replacing one query per socket, `realtime.rs:462-478`), and every
connection's set is recomputed every 5 minutes, staggered.

The worst case of a stale set is that a removed member hears event ids for a few
milliseconds; content always comes through HTTP, which checks access again.

#### The wire (protocol 2)

Same address, `GET /api/v1/realtime?v=2`. Protocol 1 (today's bare envelopes) stays until W1
has switched (removed in K5.15). Positions are strings because they can exceed JavaScript's
safe integers.

Client to server (each frame under the existing 4,096-byte limit):

```json
{"type":"hello","protocol":2,"resumeFrom":"18342","client":"web"}
{"type":"watch","document":{"kind":"page","id":"0193a1f0-...-7c1e"}}
{"type":"unwatch","document":{"kind":"page","id":"0193a1f0-...-7c1e"}}
{"type":"typing","conversationId":"0193a0aa-..."}
{"type":"awareness","document":{"kind":"whiteboard","id":"..."},"state":{"cursor":[412,88]}}
```

Server to client:

```json
{"type":"ready","protocol":2,"position":"18350","resumed":true,"replayed":8}
{"type":"event","position":"18343","event":{"event_id":"0193...","event_cursor":90211,
  "version":1,"project_id":"0192...","conversationId":"0193a0aa-...",
  "type":"conversation.message.created","occurred_at":"2026-11-14T10:02:03.120000Z",
  "payload":{"conversationId":"0193a0aa-...","messageId":"0193...","sequence":41,
             "authorActorId":"0192..."}}}
{"type":"access","added":{"spaces":["0192..."]},"removed":{"conversations":["0193..."]}}
{"type":"resync","reason":"too_far_behind","position":"40112"}
{"type":"ephemeral","kind":"typing","conversationId":"0193a0aa-...","accountId":"0192...",
  "until":"2026-11-14T10:02:09Z"}
{"type":"ephemeral","kind":"stream.delta","conversationId":"...","messageId":"...",
  "runId":"...","delta":"…next words…"}
{"type":"ephemeral","kind":"presence","workspaceId":"...","accountId":"...","online":true}
{"type":"ephemeral","kind":"awareness","document":{"kind":"page","id":"..."},
  "connection":"c-5f2a","accountId":"...","state":{"anchor":120,"head":131}}
{"type":"ephemeral","kind":"document.update","document":{"kind":"page","id":"..."},
  "generation":"0193...","version":42,"update":"AQLy...base64"}
{"type":"reconnect","afterMs":1800,"reason":"server_draining"}
```

**Resume**: if `resumeFrom` is given, is still in the log (retention keeps 30 days,
`retention.rs:356`) and is at most 5,000 positions behind, the server replays what the
connection's subscription set allows, then sends `ready` with `resumed: true`. The replay
uses "catch up, then live": the connection is registered for live events first (buffered),
the catch-up read stops at the position live delivery started from, and buffered events at or
below it are dropped. Otherwise `ready` says `resumed: false` and the client refetches as it
does today.

**What the web client must do (W1 owns it)**: send `hello` with the last position it saw;
when `resumed` is true, skip today's broad refetch on reconnect
(`useRealtimeConnection.ts:272-281`), which is the main win for flaky phone connections; on
`resync` or `resumed: false`, refetch as today; on `reconnect` or close code 1012, wait
`afterMs` plus jitter; de-duplicate by position as well as event id; handle the ephemeral
kinds; send `watch` when a live document opens and `typing` at most once every 3 s. A shared
fixture (`apps/server/tests/fixtures/realtime_wire_v2.json`) holds example frames that both
the server's and the web's tests parse, like the existing parity fixtures.

#### What agents' streams get (A2 owns the endpoints)

A2 builds the agent event stream (server-sent events with `Last-Event-ID`) and long-poll on
K3's log. K5 provides, in `realtime::follow`:

```rust
pub struct FollowSpec {
    pub account_id: Uuid,
    pub subscription: SubscriptionSet,          // from `compute`, narrowed by A2's filters
    pub filter: Arc<dyn Fn(&LoggedEvent) -> bool + Send + Sync>, // A2: event kinds, reasons
}
/// Catch up from `after` through `events::read` (K3), then switch to live delivery from this
/// instance's tail without gaps or repeats. Ends with `FollowEnd::Resync` if too far behind.
pub fn follow(state: &AppState, spec: FollowSpec, after: i64)
    -> impl Stream<Item = Result<FollowItem, AppError>>;
pub enum FollowItem { Event(Arc<LoggedEvent>), Ephemeral(Arc<EphemeralFrame>), Heartbeat }
```

plus the same access-change handling (a follower's set is recomputed on the same events),
heartbeats every 15 s, and the connection registry in 3.6 (`realtime_connections`), which A2
uses to cap streams per account across instances. Any instance can serve any stream because
the stream is the log read with a cursor (as `R2-harness-and-connecting-agents.md:432`
recommends).

### 3.6 Ephemeral signals, presence and typing

**The ephemeral bus** (in the in-process live implementation; an outside live service gets the
same frames through the live contract's `publish`, 3.10a): an instance publishes a frame by
sending it on the `ephemeral` channel
(batched every 50 ms, at most 7,500 bytes a notification, `{"v":1,"from":"<instance>","frames":[...]}`).
Every instance's listener receives it and routes it with the same index as durable events.
The sender routes its own frames locally at once and ignores its own notifications by `from`.
Frames are never stored and may be lost; nothing correct depends on them. Without LISTEN (a
transaction pooler, R24 rule 3), `publish_ephemeral` posts frames to the live role through the
contract instead; an operator then runs one `live` instance or an outside live service.

```rust
pub enum EphemeralFrame {
    StreamDelta { space: Uuid, conversation: Uuid, message: Uuid, run: Uuid, delta: String },
    Typing { space: Uuid, conversation: Uuid, root: Uuid, account: Uuid, until: DateTime<Utc> },
    Presence { workspace: Uuid, account: Uuid, online: bool },
    Awareness { document: DocumentRef, connection: String, account: Uuid, state: Value },
    DocumentUpdate { document: DocumentRef, generation: Uuid, version: i64, update: String },
    SessionRevoked { session: Uuid },
    Invalidate { cache: CacheName, workspace: Option<Uuid> },   // module switches, harnesses
}
pub async fn publish_ephemeral(state: &AppState, frame: EphemeralFrame); // `realtime::publish_ephemeral`, the name C2 uses
```

Moves onto it: `message.delta` (`provider.rs:785`; and later the host protocol's streamed
text, A1), session revocations (all callers listed in 2.2), module switch invalidation
(`modules/gate.rs:60` is called on every instance), and the harness registry reload.

**Presence, cheaply**: one unlogged table, shared with A2 for connection caps.

```sql
CREATE UNLOGGED TABLE realtime_connections (
    instance_id UUID NOT NULL,
    connection_id UUID NOT NULL,
    account_id UUID NOT NULL,
    session_id UUID,
    client TEXT NOT NULL CHECK (client IN ('web', 'agent_stream', 'host')),
    -- A person's connection spans their workspaces, so this is a list, not one workspace_id.
    workspace_ids UUID[] NOT NULL CHECK (cardinality(workspace_ids) <= 500),
    connected_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (instance_id, connection_id)
);
CREATE INDEX realtime_connections_account ON realtime_connections (account_id);
CREATE INDEX realtime_connections_workspaces ON realtime_connections USING gin (workspace_ids);
```

A row is inserted on connect and deleted on close; each instance refreshes `seen_at` for all
its rows in one statement every 30 s. Online means a row seen in the last 90 s. On the first
connection of an account (a count after insert) and the last one (a count after delete) the
instance publishes `Presence`. A crashed instance's rows go stale and are deleted by a job
once its heartbeat has lapsed. Unlogged because it is advisory and rebuilt within 30 s.
`presence::online_in(workspace) -> Vec<Uuid>` is offered to C2, which builds the people list.
An agent's stream (`client = 'agent_stream'`) counts as presence like a browser socket, so
`@here` reaches agents that are connected (C2's request, accepted).

**Typing**: the client sends `typing`; the server checks the conversation against the
connection's set (no query), then publishes `Typing` with `until = now + 6 s`, routed like a
conversation event. Clients expire it themselves. Agents show work through durable reply
progress as today; an agent that wants a typing line calls an operation that publishes the
same frame (A2 and K2 decide whether to add one).

### 3.7 Live documents (pages, sheets, whiteboards co-editing)

Today there are no in-memory rooms (2.2): the database is the room. Editors on different
instances each send updates by HTTP to whatever instance they reach, every write locks the
document row and merges by the document's own rule, and the durable `page.updated` /
`sheet.updated` / `whiteboard.updated` event tells the other editors to sync. Once K5.5
delivers those events everywhere, co-editing is correct across instances.

| Option | How | For | Against |
| --- | --- | --- | --- |
| Sticky routing by document | The load balancer hashes the document id so its editors share an instance | No new code in the app | Needs a balancer that can do it; breaks on every rebalance; gives no correctness guarantee by itself |
| Relay through the database and the log (today) plus an ephemeral fast path | Durable path unchanged; after a write commits, the instance also sends the update bytes as a `DocumentUpdate` frame to watchers | Correct with no new parts; works with any balancer | Every update reloads and re-encodes the stored document (up to 5 MB, `page_docs/store.rs:43`); latency is one HTTP round trip plus a row lock |
| One owner per document | An instance takes `document:<kind>.<id>`, keeps the document in memory, merges updates and saves in batches; other instances forward to it | Cheapest per keystroke, lowest latency, true cursors | Forwarding between instances, failover of an in-memory document, the most code |

**Now**: relay through the database and the log, plus the fast path. After a page update
commits (`page_docs/http.rs:186-268`), if the reply update is at most 6 KB in base64, the
instance publishes `DocumentUpdate` to the document's watchers. A browser applies it as a Yjs
update (merging is idempotent, so a repeat is harmless) and keeps syncing on `page.updated`
as the truth. Watching (`watch` frame) checks access once (C1's `documents::can_view`) and is
re-checked on access changes. Cursors and selections (`awareness`, at most 2 KB, at most 10 a
second per connection) go to watchers the same way. Whiteboards and sheets get awareness;
their update fast path is C1's call.

**Live document rooms are audiences** (ruling R24 rule 5): a document's watchers are the
audience `document:<kind>.<id>`; a connection's ticket (3.10a) names the documents it may
watch, `api` checks `documents::can_view` when it issues the ticket, and `DocumentUpdate` and
awareness frames are published to that audience like any other. The live role needs no
document code and no database, so an outside live service holds rooms the same way (K5.21).

**Later** (K5.16, deferred until a trigger is hit): one owner per document under a lease,
with the balancer hashing by document id as a hint and the database path as the fallback.
An editor whose request lands on a non-owner is served by today's database path, which stays
correct; the owner reloads when the stored version moves past its own. Triggers: page update
p95 above 150 ms, document saves above about 50 a second on one document, or C1 needing
per-keystroke cursors in large documents.

### 3.8 Shared limits

**Two tiers**, both counted in PostgreSQL:

- **Exact** for anything that guards security or abuse: sign-in (per login, per hour, per
  address, global), password resets and reset emails, invitation tokens, MCP token attempts,
  OAuth ceremonies and refresh failures, WorkOS starts, bug-report links, webhook attempts.
  Low volume; one round trip per check.
- **Chunked** for throughput: authenticated writes (240 a minute per session), expensive reads
  (120), MCP requests (600 per credential), and the per-workspace and per-account buckets H1
  and A2 add. An instance reserves a chunk of the budget (about a twentieth of the limit, at
  most 10) in one round trip and spends it locally. Bound: never more than the limit in total;
  at most `chunk × (instances − 1)` of a window's budget left unused at its end.

```sql
CREATE UNLOGGED TABLE rate_limit_counters (
    bucket TEXT NOT NULL CHECK (bucket ~ '^[a-z][a-z0-9_.]{0,62}$'),
    key_digest BYTEA NOT NULL CHECK (octet_length(key_digest) = 32),
    window_start TIMESTAMPTZ NOT NULL,
    window_seconds INTEGER NOT NULL CHECK (window_seconds BETWEEN 1 AND 86400),
    used INTEGER NOT NULL CHECK (used >= 0),
    expires_at TIMESTAMPTZ NOT NULL,
    workspace_id UUID,               -- set for per-workspace buckets, for clean-up and isolation
    PRIMARY KEY (bucket, key_digest, window_start)
);
CREATE INDEX rate_limit_counters_expiry ON rate_limit_counters (expires_at);

-- take($bucket, $digest, $window_seconds, $cost, $limit, $workspace)
INSERT INTO rate_limit_counters AS c
    (bucket, key_digest, window_start, window_seconds, used, expires_at, workspace_id)
VALUES ($1, $2,
        to_timestamp(floor(extract(epoch FROM now()) / $3) * $3), $3, $4,
        to_timestamp(floor(extract(epoch FROM now()) / $3) * $3 + $3), $6)
ON CONFLICT (bucket, key_digest, window_start) DO UPDATE
    SET used = c.used + EXCLUDED.used
    WHERE c.used + EXCLUDED.used <= $5
RETURNING used, expires_at;
-- No row returned: over the limit; Retry-After = the window's end minus now().
```

Fixed windows as today (same burst behaviour). Unlogged because the counters only matter for
minutes; a database crash or failover resets them, which is the stated bound and is far
better than today's reset on every restart. A clean-up job deletes expired rows every minute.

```rust
pub enum Tier { Exact, Chunked { chunk: u32 } }
pub struct Bucket { pub name: &'static str, pub limit: u32, pub window: Duration, pub tier: Tier }
pub enum LimitKey<'a> {
    Global, Secret(&'a [u8]), Address(&'a str), Session(Uuid), Credential(Uuid),
    Account(Uuid), Workspace(Uuid),
}
/// Where a bucket's number comes from for a key: defaults here, plans (H1), budgets (A2).
pub trait LimitSource: Send + Sync {
    fn limit_for(&self, bucket: &Bucket, key: &LimitKey<'_>) -> Option<u32>;
}
#[derive(Clone)]
pub struct Limits { /* pool, local chunks, Arc<dyn LimitSource> */ }
impl Limits {
    pub async fn check(&self, bucket: &'static Bucket, key: LimitKey<'_>) -> Result<(), AppError>;
    pub async fn check_cost(&self, bucket: &'static Bucket, key: LimitKey<'_>, cost: u32)
        -> Result<(), AppError>;
    /// Seconds until `key` may try again, without spending (today's `spent_at`).
    pub async fn spent(&self, bucket: &'static Bucket, key: LimitKey<'_>) -> Result<Option<u64>, AppError>;
}
```

`RateLimiters` keeps its method names so the 35 call sites change only by `.await`
(`account_security.rs`, `auth/mod.rs`, `auth/workos.rs`, `auth/workos/agents.rs`,
`bug_reports.rs`, `conversations.rs:3296`, `github.rs`, `invitations.rs`, `mcp.rs`,
`mcp/oauth.rs`, `password_reset.rs`, `public_api.rs`, `push/actions.rs`, `workspace.rs`,
`work_items/list.rs`, `work_items/views.rs`). The password-hashing semaphore stays per
instance. "Recent OAuth clients" (`rate_limit.rs:529-560`) is read from the token tables'
`created_at` with a 60 s per-instance cache instead of a map. If the counter query fails, exact
buckets refuse (fail closed) and chunked buckets allow (fail open), and both log.

The client address header (`rate_limit.rs:420-425`) becomes the setting
`AGENT_WORKSPACE_TRUSTED_ADDRESS_HEADER` (no default; the live server's environment file sets
today's header in the same slice, and the Red Hat guide says so).

**Per-workspace and per-account limits** that later work builds on: `LimitKey::Workspace` and
`LimitKey::Account`, with `LimitSource` the single place a plan (H1) or a budget (A2)
supplies the number. K2's operation runner calls H1's `metering::admit` once per write
(ruling R13); `admit` checks spend limits and, for the short rate buckets, calls
`limits.check_cost(&OPS_WRITE, ...)` for the account and for the workspace, with generous
defaults that change no current behaviour. Before H1's `admit` lands, the runner calls
`check_cost` directly. K5 owns the limiter store; H1 owns `admit` and spend limits. Metering for billing (monthly counts) is H1's `usage_counters`, not this
table: rate limits are short and lossy, metering is durable.

### 3.9 The file storage seam

```rust
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct StoreId(String);          // "local", "object"; stored on each file row

pub struct ObjectKey(String);        // today's opaque keys, `<org>/<file>` (files.rs:400)

pub struct PutOptions { pub content_type: String, pub length: Option<u64> }
pub struct StoredObject { pub body: axum::body::Body, pub length: u64, pub etag: Option<String> }
pub struct LinkOptions { pub ttl: Duration, pub content_type: String, pub disposition: String }

#[async_trait::async_trait]
pub trait ObjectStore: Send + Sync {
    fn id(&self) -> &StoreId;
    /// Streams `body` to `key`, which must not exist yet. Returns the length and SHA-256.
    async fn put(&self, key: &ObjectKey, body: BoxStream<'static, io::Result<Bytes>>,
        options: PutOptions) -> Result<Stored, StorageError>;
    async fn get(&self, key: &ObjectKey, range: Option<Range<u64>>) -> Result<StoredObject, StorageError>;
    async fn head(&self, key: &ObjectKey) -> Result<Option<u64>, StorageError>;
    async fn delete(&self, key: &ObjectKey) -> Result<(), StorageError>;   // gone already = ok
    async fn list(&self, prefix: &str, older_than: DateTime<Utc>)
        -> Result<BoxStream<'static, Result<ObjectKey, StorageError>>, StorageError>;
    /// A short-lived URL the store itself serves, when it can (object stores). None for disk.
    async fn presign_get(&self, key: &ObjectKey, options: &LinkOptions)
        -> Result<Option<Url>, StorageError>;
    /// A short-lived URL a client uploads `key` to directly (ruling R24 rule 2), limited to
    /// `length` bytes and `content_type`. None for disk, which keeps the streaming path.
    async fn presign_put(&self, key: &ObjectKey, options: &PutLinkOptions)
        -> Result<Option<Url>, StorageError>;
}
pub struct PutLinkOptions { pub ttl: Duration, pub content_type: String, pub length: u64, pub sha256: Option<[u8; 32]> }

pub struct Stores { primary: Arc<dyn ObjectStore>, all: HashMap<StoreId, Arc<dyn ObjectStore>> }
impl Stores {
    pub fn from_config(config: &AppConfig) -> Result<Self, AppError>;
    pub fn primary(&self) -> &Arc<dyn ObjectStore>;
    pub fn get(&self, id: &StoreId) -> Result<&Arc<dyn ObjectStore>, AppError>;
    /// Where a workspace's new uploads go: its own bucket when it has one (ruling R23),
    /// otherwise the primary store. Loaded on demand and cached briefly, never per workspace
    /// at start (ruling R24 rule 6).
    pub async fn for_workspace(&self, conn: &mut PgConnection, workspace: Uuid) -> Result<Arc<dyn ObjectStore>, AppError>;
}
```

`AppState` gains `stores: Stores` and `FileStore::from_config` (`files/store.rs:38-42`)
disappears.

**The seam does not assume the bytes are ours** (ruling R23). A file row with `source_id`
set (C1 adds `files.source_id`/`source_ref`; M1 owns the file-source contract) never reaches
`ObjectStore`: reads go to the source module through C1's file thing, and K5's code paths
(`files::link`, the orphan sweep, `files-move`, quota) skip such rows. A source that can `put`
and is chosen as a space's upload target is likewise C1's and M1's, not a K5 store. K5 owns
only where Workspace keeps the bytes of its own uploads.

**A workspace's own bucket** (ruling R23, hosted): `workspace_stores (workspace_id PK,
store_id, endpoint, bucket, region, prefix, path_style, secret_ref, verified_at)`; the access
keys live in the secrets keyring under `secret_ref`, never in the row, the browser or the
log. An owner sets it from workspace settings (operation `workspace_store_set`, permission
`settings.manage`, K1); the server verifies it by a put, get and delete of a probe object
before it is used. New uploads go to it; existing files stay where they are until the owner
runs the move (the `files-move` job, scoped to one workspace). The outbound address guard
applies to a workspace-supplied endpoint (unlike the operator's).

**Direct upload and download** (ruling R24 rule 2). When the store can sign:
`file_upload_start` (operation) checks quota on the declared length, inserts a `files` row in
state `pending` and returns a signed `PUT` link (15 minutes, length and type fixed);
the client uploads straight to the store; `file_upload_finish` checks `head` (length, and
SHA-256 when the client declared it), marks the row ready, and emits `file.attached`. Images
upload to a quarantine key; a `work` job strips location data, writes the final object and
marks the row ready (the image shows as processing until then). Pending rows older than 24 h
and their objects are swept. Downloads redirect to a presigned `GET` for attachments, as
below. Disk storage keeps today's path through the server.

- **Local disk** keeps today's behaviour (temporary name, `fsync`, rename,
  `files/store.rs:92-120`) and gains streaming writes.
- **S3-compatible**: endpoint, bucket, region, prefix, path-style flag, and access keys from
  secret files (the `optional_secret_path` pattern, `config.rs:380-381`). The recommended
  library is the `object_store` crate (one interface for disk and S3, multipart uploads,
  presigned URLs); `aws-sdk-s3` is heavier and `rusty-s3` would mean writing multipart by hand.
  Confirm presigning support in the version chosen at build time. The endpoint is operator
  configuration, never user input, so the outbound address guard doesn't apply.
- **Streaming uploads**: non-image uploads stream to the store while being counted and hashed;
  the first bytes are sniffed for type. Images still need processing to strip location data
  (`files.rs:358-371`), so they are buffered up to a smaller cap (25 MB). Quota: a cheap
  check on the declared length before streaming, then the existing per-workspace lock and an
  exact check before the row is inserted (`files.rs:388-392`); on refusal the object is
  deleted. A daily job deletes objects older than 24 h that have no row (crash leftovers).
- **Signed short-lived download links** (A2 needs them for agents; A1 for host artefacts):
  `files::link(state, reader, file_id, variant, ttl) -> Url`. The link is
  `/api/v1/files/links/{token}`, where the token is the file id, variant, expiry and the
  reader's account, signed with HMAC-SHA256 under a key derived from the secrets keyring
  ("file-links"). Default lifetime 15 minutes, at most 1 hour. Opening a link checks the
  signature, the expiry, that the file isn't removed, and that the account can still read it
  (one query, so access is checked at use as elsewhere). It then streams the bytes, or
  redirects to a presigned store URL when the store offers one. Browsers keep using the
  existing content route; with `files.redirect_downloads` on, attachment downloads (not inline
  images, which the content security policy keeps on our own origin) redirect to a presigned
  URL so large files don't pass through the app.
- **Columns**: `files ADD COLUMN store_id TEXT NOT NULL DEFAULT 'local'` and
  `link_previews ADD COLUMN image_store_id TEXT` (null when there is no image), both with a
  `^[a-z][a-z0-9_-]{0,31}$` check. Adding a column with a constant default is instant, so no
  backfill.
- **Moving existing files**: an operator command `files-move --to object` enqueues a resumable
  job that walks files by id in batches of 100, copies each object, checks length and SHA-256,
  switches `store_id` in a transaction, and leaves the source in place. A second command
  (`files-prune-source`) deletes sources that have been moved, so going back is possible until
  then. Reads follow `store_id`, so moved and unmoved files are served side by side.
- **Backups**: on disk, the backup must include `<data root>/files` (the gap AWD-19 tracks; H1
  and P0 own the scripts). Take the database dump first, then the files: files are
  write-once under unique keys, so every row in the dump has its file unless it was deleted
  in between. On an object store, turn on bucket versioning with a lifecycle rule that expires
  old versions after the backup retention; a deleted file then stays recoverable for that
  long without the app delaying deletes. Deleting a file stays immediate in the app.
- Git mirrors, worktrees and provider homes are not files in this sense; they stay on the
  built-in host's disk and leave with it (A1).

### 3.10 Process roles, shutdown, deploys, health

**One binary, three roles by setting** (ruling R24 rule 4): `AGENT_WORKSPACE_ROLES=api,live,work`
(default; any non-empty subset), `AGENT_WORKSPACE_BUILTIN_HOST=on|off` (default on; H1's hosted
profile sets off).

| Piece | api | live | work |
| --- | --- | --- | --- |
| Application and GitHub webhook listeners, MCP, operation routes, ticket issue and resume reads (3.10a) | yes | no | no (a health listener only, `AGENT_WORKSPACE_HEALTH_ADDR`) |
| WebSocket and agent streams, routing by audience | no | yes | no |
| Tail of the log and publish to the live role (in process: a function call) | yes, when live is in the same process | no database at all when separate | yes, holding lease `live.publisher` when live is separate |
| Wake listener, sequencer step | yes (optional) | no | yes |
| Job workers, consumers, outbox side-effect publisher, reply worker, GitHub job loop, indexing, deliveries | no | no | yes |
| Built-in host lease (scheduler, recovery, repositories, provider sign-ins, harness reloader) | no | no | yes, when the host is on |
| Startup maintenance lease (`server.startup`) | no | no | yes |

`api` holds no loop and no per-instance cache it must warm; it answers within a second of
starting (a test starts it cold and times the first request). **The tick endpoint**:
`POST /internal/work/tick` (bearer: `AGENT_WORKSPACE_WORK_TICK_SECRET`, compared in constant
time; refused when unset) runs one bounded round of `work` (due jobs, consumer batches, the
scheduler's due rows, the sequencer, retention) for at most 25 s and answers with what it did
and whether more is due. `AGENT_WORKSPACE_WORK_LOOP=on|off` (default on): with it off, `work`
runs only when ticked, so an outside timer can drive it and nothing has to stay awake (R24
rule 4). Nothing in any role runs per workspace (R24 rule 6): every scheduler queries due
rows across all workspaces.

A new module `apps/server/src/server.rs` assembles everything from config and roles (the
part of `main.rs:323-530` after the CLI), so `main` and the cluster tests start instances the
same way.

### 3.10a The live role as a published contract (ruling R24 rule 5)

The boundary between the database side (`api`, `work`) and the live side is a small published
contract, `docs/contracts/live.md`, so an edge service that sleeps idle sockets can replace
the in-process implementation without the server changing.

- **Ticket.** `GET /api/v2/live/ticket` (session or bearer) returns a ticket signed with a key
  derived from the secrets keyring (`live-ticket`, Ed25519 so a separate live service holds
  only the public key): account, connection kind (`web`, `agent_stream`, `host`), audiences it
  may hear: for a browser, the current workspace (with its spaces and restricted conversation
  roots), the direct scope, and the other workspaces' unread counts only (W1 3.6, C2 3.12); for
  an agent stream, every workspace its credential reaches and the direct scope if its
  subscription includes DMs; plus the account and watched documents (the `SubscriptionSet` of
  3.5), the session id, and an expiry
  of 5 minutes. The client opens the live socket with it and refreshes it before expiry
  through `api`. When access changes, `api` publishes a `ticket.stale` control frame to the
  account's audience; the client fetches a new ticket and the live role replaces the
  connection's audiences.
- **Publish.** The database side sends `{audiences, frame}` to the live role: durable
  envelopes (K3's `LoggedEvent` serialized once, with its position) and ephemeral frames, in
  position order for durable ones. In process this is a function call; separately it is
  `POST <AGENT_WORKSPACE_LIVE_URL>/publish` signed with the same key family.
- **Resume through `api`.** A client that reconnects calls `api` with its cursor; `api` reads
  the missed events with K3's `events::read` (access re-checked) and returns them with a
  fresh ticket whose start position the live role delivers from. The live role keeps no
  history and needs no database.
- **Conformance suite** (`tests/live_contract/`, runnable against any URL with
  `AGENT_WORKSPACE_LIVE_CONTRACT_URL`): ticket verification, expiry, audience routing (a
  frame reaches exactly the audiences named), order, resume without gap or repeat, document
  rooms, `ticket.stale`. It runs in CI against the in-process implementation, which stays
  the default.

**Graceful shutdown** on SIGTERM:

1. Mark the instance draining (`server_instances.draining_at`); `/health/ready` answers 503.
2. Wait `AGENT_WORKSPACE_DRAIN_SECONDS` (default 5) so the balancer stops sending new requests.
3. Send `{"type":"reconnect","afterMs":<random 0-3000>}` to every connection and close with
   1012; they reconnect to another instance and resume from their position.
4. Stop accepting, finish in-flight HTTP (axum's graceful shutdown, as now).
5. Stop loops; release every lease (wakes standbys at once).
6. The built-in host: cloud runs are released for the next holder to follow; local child
   processes are stopped as today (`main.rs:528`) after an optional
   `AGENT_WORKSPACE_RUN_DRAIN_SECONDS` (default 0) in which no new runs are admitted.
7. Mark stopped.

systemd's `TimeoutStopSec=20s` (`deploy/redhat/agent-workspace.service`) must exceed the
drain plus the in-flight time; H1 sets it.

**Deploying with two instances** (H1 packages it; this is the behaviour it relies on): run two
instances behind the reverse proxy, which health-checks `/health/ready`. Restart them one at a
time: stop A (it drains, its sockets move to B, its leases move to B), start A on the new
release, wait for ready, then do B. Migrations must keep the previous release working (the
existing expand, copy, switch, contract rule). Agent runs on the built-in host are still
interrupted when the host's instance restarts, unless they are cloud runs or the run drain is
set; that ends when A1 moves the host out.

**Health endpoints** (`health.rs:22-24`):

- `/health/live`: unchanged.
- `/health/ready`: 200 only when the database answers, the schema is at the version this
  release expects (the newest applied migration in `_sqlx_migrations`), the instance is not
  draining, and for the live role (in process) the tail is less than 10 s behind the head (a
  disconnected wake listener is reported but no longer makes the instance unready: the polls
  keep it correct, R24 rule 3). Body: `{"status":"ready","instance":"0193...","roles":["api","live","work"],
  "draining":false,"tailLagMs":12}`.
- `/health/version`: unchanged.

The operator's Server page payload (`server_status.rs`) gains the list of live instances with
roles, release, heartbeat age and the leases each holds. No new route.

**MCP sessions**: any instance already resumes any session (`mcp.rs:4087-4175`). The
"ended with DELETE" list moves to an unlogged table `mcp_ended_sessions(session_digest BYTEA
PRIMARY KEY, ended_at TIMESTAMPTZ NOT NULL)` pruned after 24 h, so a session ended on one
instance stays ended on all. When K2 moves to the stateless revision, legacy sessions keep
this for their deprecation window and then the table goes.

### 3.11 New settings

| Setting | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_ROLES` | `api,live,work` | This instance's roles |
| `AGENT_WORKSPACE_BUILTIN_HOST` | `on` | Contend for the built-in host lease |
| `AGENT_WORKSPACE_INSTANCE_LABEL` | none | Operator's name for the instance |
| `AGENT_WORKSPACE_DATABASE_LISTEN_URL` | main URL | Session connection for the wake and migrations; `off` runs on polls alone |
| `AGENT_WORKSPACE_WORK_LOOP`, `AGENT_WORKSPACE_WORK_TICK_SECRET` | `on`, none | `work` loops by itself, or only when ticked |
| `AGENT_WORKSPACE_LIVE_URL` | none (in process) | An outside live service implementing the contract |
| `AGENT_WORKSPACE_DATABASE_MAX_CONNECTIONS` | 10 (was 5) | Pool size |
| `AGENT_WORKSPACE_DRAIN_SECONDS`, `AGENT_WORKSPACE_RUN_DRAIN_SECONDS` | 5, 0 | Shutdown |
| `AGENT_WORKSPACE_HEALTH_ADDR` | none | Health listener for `work`-only instances |
| `AGENT_WORKSPACE_TRUSTED_ADDRESS_HEADER` | none | Header carrying the caller's address |
| `AGENT_WORKSPACE_FILE_STORE` and `AGENT_WORKSPACE_S3_*` | `local` | The primary store |

### 3.12 How this maps to the end state

Principle 7 ("all state lives in PostgreSQL and file storage; any server process can serve any
request; hosting adds operations, never features"): every row in 2.2 ends in the database, in
the object store, in a lease, or in the inventory of deliberate per-instance state (section 6).
"Live updates" (END-STATE section 5): events go into the log in the same transaction as the
change, every instance hears about them through the database, and access is decided at
subscribe time. "Files ... behind one seam" (section 3): 3.9. "Background work ... with a role
switch" (section 5): 3.10. The agent event stream (section 4) is A2's, served from K5's
`follow`.

## 4. Migration path

The live server runs one process and will apply every pending migration in one go. Every K5
migration is additive, assumes nothing about K3's or K4's tables, and leaves a one-process
install behaving as before.

1. **Harness first** (K5.1). App assembly moves into `server.rs`; `main` calls it. No behaviour
   change.
2. **Instances and leases** (K5.2). Tables added; each instance registers and heartbeats.
   Nothing is gated on a lease yet.
3. **Built-in host behind the lease** (K5.3). A single instance takes `builtin-host` at once
   and recovers runs as today. After a crash restart it waits up to 15 s for the old lease to
   expire. The old startup code path is deleted in the same slice (it is internal).
4. **The wake and the tail, in shadow** (K5.4). K3's sequencer (K3.3, or built here to K3's
   schema if K3.3 has not started) positions new events; the tail reads `events` and records
   metrics (lag, events seen, and events the in-process hub broadcast that the tail didn't,
   which must stay 0), but sockets are still fed by the hub. Events from before K3.2 are not
   in the log and are never re-broadcast.
5. **Switch the fan-out** (K5.5). Sockets are fed by the tail; the publisher stops
   broadcasting; ephemeral frames move to the bus. The wire stays protocol 1 and the per-event
   authorization stays, so the browser sees no change. After this and K5.3, two instances
   are correct together.
6. **Limits** (K5.6): `RateLimiters` keeps its methods; the in-memory windows are replaced in
   the same slice (internal type, no coexistence needed). The address header becomes a setting
   and the live environment file gets it in the same pull request.
7. **Roles and drain** (K5.7). Default roles are both, so nothing changes until an operator
   splits them. From here the live server may run two instances.
8. **Subscriptions** (K5.8) replace per-event checks behind the same wire. **Protocol 2**
   (K5.9) is added beside protocol 1. W1 switches the web client. **K5.15** deletes protocol 1,
   the old hub type and leftovers once the web client has used protocol 2 for one release.
9. **Files** (K5.11-K5.13). `store_id` defaults to `local`; nothing moves until the operator
   configures an object store and runs `files-move`. Sources are deleted only by a separate
   command.
10. **K3's log**: the tail reads `events` from K5.4 on, so there is no switch; K3.22 retires the
    outbox once K5.5 feeds sockets from the tail.
11. **Hosted-cheap** (K5.17 to K5.21): pooler-safe database use, the tick endpoint, the live
    contract, direct uploads and a workspace's own bucket, document rooms under the contract.
    Each is additive and leaves one process with the default settings behaving as before.

## 5. Slices

The milestone "two instances act as one" is K5.1 to K5.7. A1's host-out work and H1's hosting
work depend on it.

### K5.1 Cluster test harness and app assembly

Move the server's assembly out of `main.rs` into `server.rs` (`Server::build(config, pool,
roles) -> Server`, `Server::spawn(shutdown) -> RunningServer` with the listeners and loops),
and add a test harness that starts two or more full instances against one test schema, each
with its own `AppState`, listener and loops, and can stop one gracefully or abruptly (abort
its tasks without releasing anything). Channel names will use the schema (3.4), so the harness
needs nothing special for them later.

- Changes: `apps/server/src/server.rs` (new), `main.rs`, `lib.rs` (export),
  `tests/integration/cluster_support.rs` (new), `tests/integration/cluster.rs` (new),
  `tests/integration/main.rs` (register).
- Depends on: nothing. P0's test-fixture work if it lands first (reuse its schema helper).
- Acceptance: `cluster.rs::sign_in_on_one_instance_works_on_the_other` (session cookie from A
  reads `/api/v1/session` on B); `cluster.rs::write_on_one_read_on_the_other` (create a task
  on A, read it on B); the existing suite passes unchanged.
- Size: S.

### K5.2 Instances and leases

The `server_instances` and `leases` tables, the `cluster` module (3.2), the heartbeat loop with
self-fencing, the `LeaseLost` error, and `while_held`. K3's consumers move from their per-batch
cursor-row lock to `consumer:<name>` leases if K3.3 has merged. (The loops that stopped the
server on a database error are done by P0.2.)

- Changes: migration `<ts>_server_instances_and_leases.sql`; `cluster/{mod,instance,leases}.rs`;
  `lib.rs` (`AppState.cluster`); `server.rs`; `error.rs` (`LeaseLost`, never shown to clients);
  `retention.rs` (prune stopped instances after 7 days); `events/consumer.rs` (K3) if present.
- Depends on: K5.1; P0.2 (the loop fix it no longer carries).
- Acceptance (`tests/integration/cluster.rs`, lease time to live set to 2 s in tests):
  `lease_is_held_once_under_contention` (two instances, 50 concurrent `try_acquire` each:
  exactly one success); `lease_moves_after_holder_stops` (abort A: B acquires after expiry,
  token is one higher); `released_lease_is_taken_at_once` (B acquires within 500 ms of A's
  release); `fenced_write_fails_with_a_stale_token`; `self_fencing_cancels_work_when_renewal_stops`
  (block A's heartbeat with a held row lock: A's `lost` token fires before B can acquire).
  Unit: lease name format; the instance id is a version 7 UUID.
- Size: M.

### K5.3 The built-in host behind one lease

Section 3.3: the run scheduler, recovery, worktree and approval clean-up, repository
preparation (now a host-scoped job) and the harness reloader run only on the `builtin-host`
holder; admissions and recovery transitions are fenced; `kick` wakes the holder; startup
one-off tasks run under `server.startup`.

- Changes: `recovery.rs` (delete `:33-45`; `reconcile_owned` with fencing), `scheduler.rs`
  (`run` and `claim_one` take the lease and fence; `kick` checks `holds`), `main.rs`/`server.rs`,
  `repositories.rs:448` and `jobs.rs` (`register_host`, host-kind filter in `claim`),
  `harnesses.rs:130`, `provider.rs` (stop local turns on lease loss).
- Depends on: K5.2.
- Acceptance: `cluster.rs::second_instance_does_not_recover_live_runs` (fixture runs in
  `working`; A holds the lease; start B: no run changes state); `cluster.rs::runs_are_recovered_once_after_the_holder_dies`
  (abort A: B moves each run to `recovering` exactly once, counted from `agent_run_events`);
  `cluster.rs::kick_on_a_non_holder_admits_on_the_holder` (queue a run through B's HTTP; the
  demo adapter starts it on A); `cluster.rs::startup_tasks_run_once` (two instances start
  together; the workflow-provisioning count is as for one). Existing
  `m1_recovery_provider_sessions.rs` and `m3_scheduler_http.rs` pass.
- Size: M.

### K5.4 Event positions and the wake (shadow)

The wake listener with reconnect and safety polls (and the polls-only mode with LISTEN off),
running K3's sequencer step (K3 §3.3.1; built here to K3's schema if K3.3 has not started, and
K3.3 is then done for that part), and the tail over `events::tail` in shadow mode with metrics.
Split `realtime.rs` into a `realtime/` folder (`hub.rs` for today's code, `wake.rs`, `tail.rs`).

- Changes: `realtime/`; `events/sequencer.rs` (only if K3.3 has not built it); `config.rs`
  (`DATABASE_LISTEN_URL`, `off`); `metrics.rs` (tail lag, notification queue usage, shadow
  divergence). No migration of its own: K3.2 creates the tables, channel functions and wake
  trigger.
- Depends on: K5.1, K3.2. Can run beside K5.2 and K5.3.
- Acceptance: `cluster.rs::positions_follow_commit_order` (transaction 1 inserts first and
  commits last; the event from transaction 2 gets the lower position; both instances' tails
  see both, positions dense); `cluster.rs::concurrent_writers_leave_no_gaps` (8 tasks × 100
  events across both instances: each tail sees exactly the set written, in strictly increasing
  dense positions); `cluster.rs::tail_survives_losing_the_listener`
  (`pg_terminate_backend` on A's listener: events still arrive within the 1 s safety poll and
  the listener reconnects); `cluster.rs::events_arrive_with_listen_off` (LISTEN off on both
  instances: every event reaches both tails within one poll). Unit: channel name under 63
  bytes for a long schema name.
- Size: M.

### K5.5 Fan-out across instances

Sockets are fed from the tail; the publisher keeps side effects only; ephemeral frames
(`message.delta`, session revocation, cache invalidation) travel on the bus. Protocol 1 and
per-event authorization unchanged.

- Changes: `realtime/` (`hub.rs` fed by the tail, `ephemeral.rs`), `realtime.rs:675-764`
  (publisher no longer broadcasts), `provider.rs:785`, the session revocation callers in 2.2,
  `modules/gate.rs:60` (forget through the bus), `harnesses.rs`.
- Depends on: K5.4.
- Acceptance (the guard rail): `cluster.rs::message_sent_on_one_instance_reaches_a_browser_on_the_other`
  (WebSocket on B, message via A's HTTP, event on B within 2 s); `cluster.rs::private_conversation_events_stay_private_on_both`
  (a non-member's sockets on A and B get nothing); `cluster.rs::logout_on_one_instance_closes_the_socket_on_the_other`
  (within 1 s, not 15); `cluster.rs::streamed_text_crosses_instances` (an ephemeral delta
  published on A reaches a socket on B); `cluster.rs::module_switch_is_seen_everywhere`
  (switch a module off on A; B refuses its route on the next request). Updated
  `m1_realtime_websocket.rs` and `m2_outbox_publisher.rs` pass.
- Size: M.

### K5.6 Shared limits

`rate_limit_counters`, the `Limits` module with exact and chunked tiers (3.8), `LimitSource`
with default numbers, `RateLimiters` methods made async over it, the trusted-address setting,
recent OAuth clients from the token tables, the counter clean-up job. Delete the in-memory
window type.

- Changes: migration `<ts>_rate_limit_counters.sql`; `rate_limit.rs` (rewritten; new
  `limits.rs`); the call sites listed in 3.8; `config.rs`; `docs/self-hosted-red-hat.md`;
  `docs/operations/abuse-controls.md` (counters are shared and survive restarts).
- Depends on: K5.1; P0.13 (the global budgets P0 fixes first: K5.6 moves them into the shared
  store, it does not fix them twice). H1.1 builds on it (`admit`). Can run beside K5.2 to K5.5.
- Acceptance: `cluster.rs::login_limit_holds_across_instances` (11 sign-in attempts for one
  login alternating A and B: exactly 10 reach the password check, the 11th is 429 with
  `Retry-After`); `cluster.rs::write_budget_is_never_exceeded` (300 writes on one session over
  both instances in a minute: at most 240 succeed); `cluster.rs::limits_survive_a_restart`
  (spend on A, restart A, still refused); unit tests for window maths and the chunk bound; the
  existing abuse-control tests pass.
- Size: M.

### K5.7 Process roles, drain and readiness

The three roles `api`, `live`, `work` (3.10) and the built-in host switch, the `work`-only
health listener, the graceful shutdown in
3.10 with the `reconnect` frame (sent on protocol 1 too, as a close with 1012), readiness
rules, instances on the Server page, the pool default, `mcp_ended_sessions`.

- Changes: `config.rs`, `server.rs`, `health.rs`, `server_status.rs`, `mcp.rs:4101-4175`,
  migration `<ts>_mcp_ended_sessions.sql`, `deploy/redhat/agent-workspace.service`
  (`TimeoutStopSec`), `docs/self-hosted-red-hat.md` (two instances, pool sizing, listen URL).
- Depends on: K5.2, K5.5.
- Acceptance: `cluster.rs::draining_instance_moves_its_sockets` (SIGTERM-equivalent on A:
  ready is 503 at once; A's sockets close with 1012; they reconnect to B; the builtin-host
  lease is on B within 1 s); `cluster.rs::work_only_instance_serves_no_app_routes`;
  `cluster.rs::api_only_instance_runs_no_jobs` (a queued job runs only on `work`);
  `cluster.rs::live_only_instance_opens_no_database_connection`;
  `cluster.rs::ended_mcp_session_stays_ended_on_the_other_instance`; unit: readiness is 503
  while the listener is down.
- Size: M.

### K5.8 Subscriptions at subscribe time

Section 3.5's subscription sets, routing by audience, the scope index, recompute on access
events, batched session checks, the 5-minute backstop, the per-instance conversation cache.
The wire stays protocol 1.

- Changes: `realtime/subscriptions.rs`, `realtime/route.rs`, `realtime/hub.rs` (index replaces
  the per-socket broadcast receiver), removal of `event_is_authorized` per event
  (`realtime.rs:506-560`) except the `Thing` exception.
- Depends on: K5.5; K1.20 and K4.2 (soft: `subscriptions::compute` uses K1's access functions
  and K4's `spaces_readable_by` when they exist, today's SQL until then).
- Acceptance: `cluster.rs::removed_member_stops_hearing_at_once` (remove B's user from a project
  through A; the next event in that project doesn't reach them, and an event after it in
  another project does); `cluster.rs::added_to_private_conversation_starts_hearing`;
  `workspace_isolation.rs` realtime cases pass unchanged; a test that counts database queries
  per delivered event (a query-counting pool wrapper): zero for `Space`, `Conversation` and
  `Account` audiences. Unit: every outbox event type maps to an audience (guard rail 4).
- Size: M.

### K5.9 Protocol 2, resume, and `follow`

The `?v=2` wire (3.5), resume with catch-up-then-live, `access` and `resync` frames, the shared
wire fixture, and the `realtime::follow` stream for A2.

- Changes: `realtime/socket.rs`, `realtime/follow.rs`, `tests/fixtures/realtime_wire_v2.json`.
- Depends on: K5.8.
- Acceptance: `cluster.rs::resume_replays_what_was_missed` (socket on B closes at position p;
  three events written on A; reconnect to A with `resumeFrom=p`: exactly those three, then
  `ready{resumed:true}`); `cluster.rs::resume_too_far_back_asks_for_resync`;
  `cluster.rs::resume_replays_only_what_the_connection_may_see`; `follow` unit tests: no gap and
  no repeat across the catch-up-to-live switch under concurrent writes; protocol 1 tests still
  pass.
- Size: M.

### K5.10 Presence, typing, and document watching

`realtime_connections`, presence frames and `presence::online_in`, typing frames, `watch` and
`awareness`, and the `DocumentUpdate` fast path for pages.

- Changes: migration `<ts>_realtime_connections.sql`; `realtime/presence.rs`;
  `realtime/socket.rs`; `page_docs/http.rs` (publish after commit); a sweep job.
- Depends on: K5.9; C1's `documents::can_view`, which C1 now provides (no page-only stand-in
  here).
- Acceptance: `cluster.rs::typing_on_one_instance_shows_on_the_other` (only to sockets that
  can see the conversation); `cluster.rs::presence_follows_connections_across_instances`
  (online when the first socket opens on either, offline after the last closes; stale rows of
  an aborted instance disappear after its heartbeat lapses); `cluster.rs::document_update_reaches_watchers_only`
  (a non-watcher and a watcher without access get nothing).
- Size: M.

### K5.11 Storage seam and streaming uploads

The `ObjectStore` trait and `Stores`, local disk on it with streaming writes, `store_id`
columns, streaming non-image uploads, the orphan sweep job. All callers move to `state.stores`.

- Changes: `files/store.rs`, `files.rs`, `files/http.rs`, `link_previews.rs`,
  `mcp/file_tools.rs`, `mcp/sheet_tools.rs:1314`, `whiteboards/http.rs` (if it stores bytes),
  migration `<ts>_file_store_ids.sql`, `lib.rs`.
- Depends on: nothing in K5 (K5.1 for a cluster test). Can run beside everything above.
- Acceptance: `w20_files.rs` passes; new tests: a 60 MB non-image upload streams without
  the process holding it whole (a counting body wrapper asserts the largest chunk held);
  quota refusal after streaming deletes the object; the orphan sweep deletes an object with no
  row and keeps one with a row; `cluster.rs::file_uploaded_on_one_instance_downloads_on_the_other`
  (both instances share a temporary data root, as two instances on one host do).
- Size: M.

### K5.12 S3-compatible store and signed links

The object store implementation, its settings, presigned redirects for downloads, and
`files::link` with the `/api/v1/files/links/{token}` route.

- Changes: `files/object.rs` (new), `files/links.rs` (new), `files/http.rs`, `config.rs`,
  `Cargo.toml`, `secrets.rs` (derived "file-links" key), isolation and route-guard cases for
  the new route.
- Depends on: K5.11.
- Acceptance: integration tests against an S3-compatible test server started by the test (or
  skipped with the same "never silently" rule the database tests use): put, get, range,
  delete, presign; `files_links.rs`: a link works within its lifetime, fails after it, fails
  for a removed file, fails after the account loses access, fails when tampered; the new route
  has a `workspace_isolation.rs` case.
- Size: M.

### K5.13 Moving files between stores

`files-move` and `files-prune-source` commands, the resumable copy job, the backup guidance.

- Changes: `main.rs` (commands), `files/migrate.rs` (new), `docs/operations/backups.md`.
- Depends on: K5.12.
- Acceptance: moving 250 files from disk to the object store in batches, interrupted after the
  first batch and resumed, ends with every row on `object`, identical SHA-256, and sources still
  present until pruned; a file uploaded during the move is written to the new store and
  served.
- Size: S.

### K5.14 Provider sign-ins through the database

Device-code sign-ins (`provider/registry.rs:109`, `:411-500`) become rows
(`provider_login_ceremonies`: account, requester, session, idempotency key, kind, response,
expires) written by the built-in host holder and read by any instance, started through a
host-scoped job. Skip this slice if A1 has already moved sign-ins into the host program.

- Changes: migration, `provider/registry.rs`, `account.rs:1277-1340`.
- Depends on: K5.3.
- Acceptance: `cluster.rs::provider_sign_in_started_on_one_instance_completes_on_the_other`
  with the demo adapter.
- Size: S.

### K5.15 Remove the old paths

Delete protocol 1 on the server, the in-process `RealtimeHub` broadcast type, the shadow
metrics, and the per-event authorization helpers that remain. Update `AGENTS.md` with the
rules in section 6.

- Depends on: K5.9 and W1.6 (the web client on protocol 2) having shipped for one release.
- Acceptance: the guard tests in section 6 pass with the shrunken allowlist; `m1_realtime_websocket.rs`
  rewritten for protocol 2.
- Size: S.

### K5.16 One owner per document (deferred)

Only when a trigger in 3.7 is hit: `document:<kind>.<id>` leases, in-memory documents with
batched saves, forwarding from non-owners, failover. C1 co-owns. Split into two slices when
started (owner and saves; forwarding and failover).

- Depends on: K5.2, K5.10, C1.30 (C1's live document work).
- Size: L (two M slices when started).

### K5.17 Pooler-safe database use (ruling R24 rule 3)

A guard test and a cluster run that keep the server working through a transaction-mode
pooler: no `pg_advisory_lock(`/`pg_try_advisory_lock(` outside the allowlisted key rotation
(guard 3 below, extended; on the direct connection only, refused without one, ruling R36), no `SET ` without `LOCAL` and no `set_config(..., false)`, no
`PREPARE`/`DEALLOCATE` by hand, no `LISTEN` outside `realtime/wake.rs`; a lint that a
transaction is not held across an `.await` on an HTTP client, provider or process call (a scan
of `begin()`..`commit()` spans for `reqwest`, `ProviderClient`, `Command` uses, with an
allowlist that can only shrink). The scan covers `tenancy/`, `metering/` and
`workspace_lifecycle/` (H1's modules) as well as the rest of the server. Today's long transaction around the agent webhook loop
(`replies.rs:1315-1440`) is split here or listed with an owner (A2). The cluster suite runs
once with `DATABASE_LISTEN_URL=off` and `statement_cache_capacity = 0` (what a pooler needs).

- Changes: `tests/guards/pooler_safe.rs` (new), `server.rs` (pool options from config),
  `config.rs` (`AGENT_WORKSPACE_DATABASE_STATEMENT_CACHE`), `docs/self-hosted-red-hat.md`.
- Depends on: K5.4 (the polls-only mode).
- Acceptance: the guard fails on a planted session lock, a planted plain `SET`, and a planted
  transaction across a `reqwest` call; the whole integration suite passes with the statement
  cache off and LISTEN off.
- Size: S.

### K5.18 The tick endpoint and a cold `api`

`AGENT_WORKSPACE_WORK_LOOP=off` and `POST /internal/work/tick` (3.10); `api` does no start-up
work beyond the pool (migrations and maintenance belong to `work` or the `migrate` command).

- Changes: `server.rs`, `jobs.rs` (`run_round(deadline)`), the consumer runtime and scheduler
  (`run_round` for each), `health.rs`, `config.rs`, route guard and isolation case for the
  internal route (refused without the secret, never listed in OpenAPI).
- Depends on: K5.7.
- Acceptance: `cluster.rs::ticks_drive_work_with_the_loop_off` (a queued job, a consumer batch
  and a due scheduled message all happen after one tick and not before);
  `cluster.rs::api_answers_within_a_second_of_starting`.
- Size: S.

### K5.19 The live contract and its conformance suite

3.10a: tickets, publish, resume through `api`, `ticket.stale`, `docs/contracts/live.md`, the
in-process implementation behind the contract, and `tests/live_contract/`. A `live`-only
instance takes its envelopes from a `work` instance holding `live.publisher`.

- Changes: `realtime/ticket.rs`, `realtime/live_contract.rs`, `realtime/socket.rs`, `secrets.rs`
  (derived `live-ticket` key), `docs/contracts/live.md`, `tests/live_contract/` (new).
- Depends on: K5.9 (resume), K5.7 (roles).
- Acceptance: the conformance suite passes against the in-process implementation and against
  a separate `live`-only instance; a ticket for one workspace hears nothing from another; an
  expired or tampered ticket is refused; resume through `api` replays without gap or repeat.
- Size: M.

### K5.20 Direct upload and download, and a workspace's own bucket

`presign_put`, `file_upload_start`/`file_upload_finish` operations (K2 generates route and
tool), quarantine and processing for images, the pending sweep, `workspace_stores` and
`workspace_store_set`, `Stores::for_workspace`, the per-workspace move.

- Changes: `files/object.rs`, `files/upload.rs` (new), `files/http.rs`, migration
  `<ts>_workspace_stores.sql`, `secrets.rs`, the operation registry (K2).
- Depends on: K5.12; K2.3 (soft: hand-written route and tool until the generator).
- Acceptance: against the S3-compatible test server, a 200 MB upload never passes through the
  server (a byte counter on the server stays under 1 MB); a finish with a wrong length is
  refused and the object deleted; an image is served only after its stripped copy is ready;
  a workspace with its own bucket stores new uploads there and its secret never appears in a
  response, a log line or a row; isolation case for both operations.
- Size: M.

### K5.21 Live document rooms under the contract

Document audiences in tickets (`documents::can_view` at issue), `DocumentUpdate` and awareness
published to `document:<kind>.<id>`, conformance cases for rooms.

- Depends on: K5.10, K5.19.
- Acceptance: conformance suite cases: a watcher hears the room, a non-watcher and a connection
  whose ticket lacks the document hear nothing; after access to the page is removed,
  `ticket.stale` drops the room.
- Size: S.

## 6. Guard rails

1. **The cluster suite** (`tests/integration/cluster.rs`) runs in CI with the rest of the
   integration tests. Its three anchor tests are the end state's promise: a message sent
   through one instance reaches a browser on the other (K5.5); a lease is never held twice
   (K5.2); a rate limit holds across both (K5.6).
2. **Inventory of per-instance state** (`apps/server/src/process_local.rs`, a test like the
   existing guards): it scans `apps/server/src/**/*.rs` for `broadcast::channel`,
   `static ... LazyLock`/`OnceLock` holding a `Mutex` or `RwLock`, `Mutex<HashMap`/`RwLock<HashMap`,
   `tokio::fs::`/`std::fs::`, and `tokio::spawn(`, and fails for any file not listed with a
   reason and a bound ("per machine CPU guard", "cache, 10 s staleness", "built-in host only").
   Entries are per file, not counts, so branches merge in either order.
3. **No session-level advisory locks** outside `cluster/`: a test fails on `pg_advisory_lock(`
   or `pg_try_advisory_lock(` (transaction-level ones are fine), because a pooled connection
   can carry a session lock to the wrong request. The one existing use, key rotation on a
   connection of its own outside the pool (`secrets.rs:1105-1110`), is allowlisted; it is
   already safe across instances. Ruling R36: the allowlisted lock is taken only on the direct
   (non-pooled) database connection, and rotation is refused with a clear error when no direct
   connection is configured, so it never runs through a pooler.
4. **Every event type has an audience**: every `events::emit` sets an `Audience` (K3's type,
   required by the struct), and a unit test over K3's catalogue asserts the router handles
   each audience, so a new event can't silently reach nobody or everybody.
5. **Every rate limit is declared**: all buckets live in one `limits::BUCKETS` list with a tier;
   a test fails if a `Mutex<HashMap` limiter appears again (covered by 2).
6. **Bytes only through the seam**: guard 2 keeps `tokio::fs` out of `files.rs`,
   `link_previews.rs`, `mcp/file_tools.rs` and `mcp/sheet_tools.rs`.
7. **The wire fixture** (`tests/fixtures/realtime_wire_v2.json`) is parsed by server and web
   tests, so the two sides can't drift.
8. **AGENTS.md rules** (K5.15, with P0): "New state shared by requests goes in PostgreSQL or the
   file store; work for exactly one instance takes a lease from `cluster`; live signals go
   through `realtime::publish_ephemeral`; never call the in-process hub; database use is
   pooler-safe (`SET LOCAL` only, no session locks, no hand `PREPARE`, no transaction across a
   network call); nothing runs because a workspace exists."
9. **Pooler-safe** (K5.17): the guard and the LISTEN-off, statement-cache-off suite run in CI.
10. **The live contract** (K5.19): the conformance suite runs in CI against the in-process
    implementation.

## 7. Risks and open questions

| Risk | How we'd notice | Fallback |
| --- | --- | --- |
| The commit-time NOTIFY lock slows busy writes | Commit latency metrics; `pg_notification_queue_usage()`; load test in K5.4 | Wake only from the sequencer (drop the insert trigger and rely on a 100 ms sequencer poll), or move the wake to a broker behind `WakeTransport` |
| A sequencer bug skips or repeats positions | `concurrent_writers_leave_no_gaps`; the tail checks density and logs a gap as an error; shadow divergence metric in K5.4 | Shadow mode stays until divergence is 0 for a week of real traffic before K5.5 merges |
| Stale subscription sets leak event ids after a removal | `removed_member_stops_hearing_at_once`; isolation suites | Recompute is in log order; content still needs HTTP; 5-minute backstop |
| A paused holder (a stalled virtual machine) keeps driving runs after takeover | Fence failures logged as `LeaseLost` | Self-fencing at two thirds of the lease; fencing on admission and recovery writes |
| After a crash restart, runs and scheduling wait up to 15 s | Startup log line with the wait | Acceptable; clean stops release at once |
| Two instances on two hosts with the built-in host on one: repository and worktree paths exist only there | Host-scoped jobs only run on the holder | Documented: until A1, all built-in host work is on one instance |
| Pool exhaustion as instances add listeners | Pool wait metrics | Pool default 10; listener outside the pool; sizing in the guide |
| Unlogged counters reset on a database crash or failover | Documented bound | Use a logged table for the exact tier if an operator needs it (one-line migration) |
| Integration tests wake each other across schemas | Flaky timing | Channels include a schema hash; safety polls keep correctness even if they don't |
| WebSocket memory per connection on live instances | Process memory metric per connection count | Queue bound 512; frames shared as `Arc<str>` |

**Questions only the owner can answer** (each with the plan's assumption):

1. Run two instances on the live server once K5.7 lands, to get deploys without downtime?
   *Recommended: yes, both on the current host behind the existing proxy.* Local agent runs
   are still interrupted when the built-in host's instance restarts, until A1.
2. Depend on PostgreSQL notifications rather than adding a broker now? *Recommended: yes*
   (3.4).
3. How far back may a browser resume before it must refetch everything? *Recommended: 5,000
   events, within the log's retention.*
4. When files are deleted, delete at once (as today) and rely on bucket versioning for
   backups, rather than delaying deletes in the app? *Recommended: delete at once*, because
   removal already promises to scrub.

## 8. Touch points

**Needs from others**

- **K3 (events)**: met in K3 (ruling R8): the one log, envelope (`workspace_id`, `space_id`,
  `Audience` with conversation root and restricted flag), sequencer, channel functions and
  wake trigger (K3.2, K3.3), `events::tail` and `LoggedEvent`; retention of at least the
  browser resume window and A2's cursor window. K3's consumers lock their cursor row per batch
  until K5.2's `consumer:<name>` leases. P0.1 fixes the publisher's lost side effects (G
  report F2).
- **K1 (access)**: events when access changes (`workspace.member.*`, `role.*`,
  `account.disabled`, `session.revoked` as an event or a direct call), and `access` functions that
  return an account's visible spaces and restricted conversations, so `subscriptions::compute`
  uses K1's rule instead of its own SQL. No permission difference by account kind exists in
  K5's area: `realtime_policy_allows` parses the kind (`realtime.rs:562-595`) but allows
  every role; it is deleted in K5.8.
- **K4 (spaces)**: `space.member.*` events; when DMs leave hidden projects they become
  `Conversation` audiences with `restricted: true`, in the direct scope for new ones (K4.23,
  R20); `project_id` in the subscription becomes `space_id`; `spaces_readable_by` (K4.2).
- **K2 (operations)**: the rate-limited error in the one error list with `Retry-After`; one
  `metering::admit` call (H1) per write operation, which uses `limits.check_cost`; the MCP stateless revision (then
  `mcp_ended_sessions` can go); a typing operation if A2 wants agents to show typing.
- **C1 (documents)**: `documents::can_view(account, DocumentRef)` for `watch`; the client side of
  `DocumentUpdate` and awareness; co-ownership of K5.16.
- **M1 (modules)**: a module-switch event, or a call to `realtime::publish_ephemeral(Invalidate)`
  on every switch write.
- **C1, M1 (file sources)**: `files.source_id`/`source_ref` and the source contract (R23); K5's
  storage code skips rows with a source.
- **H1**: `metering::admit` (R13); the shapes document (`docs/hosting/shapes.md`) that names the
  roles, the pooler and the live contract K5 provides; the 10,000-empty-workspaces guard
  (R24 rule 6) covers K5's loops too.
- **P0**: the loop-crash fix (G report F3) is P0.2's; global budgets P0.13's; backups that
  include files.

**Provides to others**

- **A1 (runs, hosts)**: the `builtin-host` lease, host-scoped jobs, fencing; the ephemeral bus
  for streamed run text from any host; signed short-lived file links for host artefacts; any
  instance can serve a host's dial-out connection because `follow` reads the log. When the
  built-in host leaves the server, K5.3's lease moves with it or is dropped.
- **A2 (connecting agents)**: `realtime::follow`, `subscriptions::compute`, access-change
  handling, heartbeats, `realtime_connections` for per-account stream caps, `Limits` with
  `LimitKey::Account`/`Workspace` and `LimitSource` for budgets and loop limits, signed
  download links for `file_read`, `consumer:`/`partition:` leases for ordered delivery.
- **W1 (web)**: protocol 2 and its fixture; resume; `reconnect` and `resync`; ephemeral kinds.
- **C2 (chat)**: typing and presence frames, `presence::online_in`.
- **H1 (hosting)**: the three roles and the tick endpoint, the live contract, pooler-safe use,
  direct uploads and per-workspace buckets, drain, readiness, the instance list for the staff console and Server
  page, the object store, `files-move`, `Limits` and `LimitSource` for plan limits, the hosted
  profile switches (`BUILTIN_HOST=off`), pool and listener settings, the two-instance deploy
  behaviour.

## 9. Changes to the end state

1. **Principle 7**, add one sentence: "A few jobs are done by exactly one server process at a
   time (the built-in host while it is inside the server, ordered consumers); they hold a lease
   in the database, and another process takes over within seconds when the holder stops."
   Without it, "any server process can serve any request" reads as if nothing were ever owned.
2. **Live updates** (section 5), add: "Signals that are never history (typing, presence,
   cursors, streaming text) travel the same way but are not written to the log; losing one is
   harmless." Today the section implies everything is an event in the one log.
3. **Standards table**, "Stream and poll" row: add "Browsers keep a WebSocket, because they also
   send (typing, cursors, which document is open); it resumes from the same cursor." The row's
   "Not WebSocket" is about agents and could be read as covering browsers.
4. **Files** (section 3), add: "Downloads can be handed out as signed links that expire within
   the hour, so an agent or a host can fetch a file without a session."
5. **Process roles** (principle 7 and section 5, from ruling R24): "One program runs as `api`,
   `live` and `work`, together or apart; `work` can be driven by a timer; the live role is a
   published contract with a conformance suite." (Draft 3 carries R24; this is the wording K5
   assumes.)

## Triggers (ruling R16)

K5 adds no trigger. The wake trigger `event_staging_wake` on `event_staging` is K3's (K3.2,
permanent); K5's earlier `outbox_events_wake` trigger is dropped from this plan with the rest
of its position design.

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| K5.1 | Cluster test harness and app assembly | S | none | K5.11 |
| K5.2 | Instances and leases | M | K5.1, P0.2 | K5.4, K5.6, K5.11 |
| K5.3 | The built-in host behind one lease | M | K5.2 | K5.4, K5.5, K5.6, K5.11 |
| K5.4 | Event positions and the wake (shadow) | M | K5.1, K3.2 | K5.2, K5.3, K5.6, K5.11 |
| K5.5 | Fan-out across instances | M | K5.4 | K5.3, K5.6, K5.11 |
| K5.6 | Shared limits | M | K5.1, P0.13 | K5.2 to K5.5, K5.11 |
| K5.7 | Process roles, drain and readiness | M | K5.2, K5.5 | K5.6, K5.8, K5.11 |
| K5.8 | Subscriptions at subscribe time | M | K5.5, K1.20 (soft), K4.2 (soft) | K5.6, K5.7, K5.11 |
| K5.9 | Protocol 2, resume, and `follow` | M | K5.8 | K5.11 to K5.14 |
| K5.10 | Presence, typing, and document watching | M | K5.9 | K5.12 to K5.14 |
| K5.11 | Storage seam and streaming uploads | M | none (K5.1 for its cluster test) | everything up to K5.10 |
| K5.12 | S3-compatible store and signed links | M | K5.11 | K5.2 to K5.10 |
| K5.13 | Moving files between stores | S | K5.12 | K5.8 to K5.10 |
| K5.14 | Provider sign-ins through the database | S | K5.3 | K5.4 to K5.13 |
| K5.15 | Remove the old paths | S | K5.9, W1.6 | K5.10 to K5.14 |
| K5.16 | One owner per document (deferred) | L | K5.2, K5.10, C1.30 | none planned |
| K5.17 | Pooler-safe database use | S | K5.4 | K5.5 onward |
| K5.18 | The tick endpoint and a cold `api` | S | K5.7 | K5.8 onward |
| K5.19 | The live contract and its conformance suite | M | K5.7, K5.9 | K5.10 to K5.14, K5.17, K5.18 |
| K5.20 | Direct upload and download, and a workspace's own bucket | M | K5.12, K2.3 (soft) | K5.13 onward |
| K5.21 | Live document rooms under the contract | S | K5.10, K5.19 | K5.13 onward |

## Review changes

- §3.4 positions: K5's own design (`outbox_events.position`, `event_log_heads`, three-statement stamp, `outbox_events_wake`) replaced by a pointer to K3 §3.3.1, keeping the pre-check and lock timeout K3 adopted (ruling R8: one log, one envelope, one sequencer).
- §3.4 tail: `LogSource`, `OutboxLog`, K5's `LoggedEvent` and `AGENT_WORKSPACE_REALTIME_LOG` removed; the tail uses K3's `events::tail` and `LoggedEvent` (R8).
- §3.5: K5's `Audience` enum removed in favour of K3's; no per-instance conversation cache (the envelope carries root and restricted); `SubscriptionSet` gained `direct` and `unread_only` (R8, R20).
- §3.6: `publish` renamed `realtime::publish_ephemeral` (the name C2 uses); ephemeral frames go through the live contract when LISTEN is unavailable (R24 rules 3, 5).
- §3.2: lease `startup-maintenance` renamed `server.startup` (H1's name); added `builtin_run:<id>` for A1.9; sequencer row points at K3's counter.
- §3.3 item 8 and K5.2: the loop fix is done by P0.2 (R10).
- §3.8: K2's runner calls H1's `metering::admit`, which uses `Limits::check_cost` (R13).
- §3.1, §3.2, §3.10, §3.11, K5.7: roles `web`/`worker` became `api`/`live`/`work`, with the tick endpoint and a cold `api` (R24 rule 4).
- §3.4 wake: LISTEN/NOTIFY is only an optimisation; a polls-only mode; readiness no longer depends on the listener (R24 rule 3).
- New §3.10a: the live role as a published contract (ticket, publish, resume through `api`, conformance suite) (R24 rule 5).
- §3.7: live document rooms are audiences under the contract (R24 rule 5).
- §3.9: `presign_put`, direct upload and download, a workspace's own bucket (`workspace_stores`), and the seam skipping file-source rows (R23, R24 rule 2).
- §4 steps 4, 10, 11 rewritten to match.
- New slices K5.17 (pooler-safe), K5.18 (tick and cold api), K5.19 (live contract), K5.20 (direct upload, own bucket), K5.21 (document rooms).
- Depends added: K5.2 P0.2, K5.4 K3.2, K5.6 P0.13, K5.8 K1.20 and K4.2 (soft), K5.15 W1.6, K5.16 C1.30, K5.20 K2.3 (soft).
- Guard rails 4, 8 updated; 9 and 10 added. §8 touch points updated. §9 change 5 added.
- Added the trigger table (R16: none).
- Content review requests (accepted): the ticket names the current workspace, the direct scope and the other workspaces' unread counts; an agent's stream counts as presence; K5.10, K5.16 and K5.21 call C1's `documents::can_view`; `client-config` is H1's (R30), so K5 adds no such route.

### Second pass

- §3.2: the `builtin_run:<id>` lease and `LeaseName::BuiltinRun` removed; K5.3's `builtin-host` is the only built-in host lease (A1 dropped per-run leases; A1.9 adds only the `fail_run` fix and `SKIP LOCKED`). This supersedes the earlier "added `builtin_run:<id>`" line above.
- K5.17: the transaction-across-await lint also covers `tenancy/`, `metering/` and `workspace_lifecycle/` (agents and hosting review).
- K5.17 and guard 3: ruling R36, the key-rotation session lock runs only on the direct connection, and rotation is refused when there is none.
- No slice added or removed; the slice table is unchanged.
