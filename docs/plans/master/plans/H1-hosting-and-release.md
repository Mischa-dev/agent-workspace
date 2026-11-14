# H1: Hosting and release

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner's draft, 2026-11-14, against main at fe95a74. Builds on the earlier "Modular, hostable
Workspace" plan (`/home/user/health-check/modular-plan.txt`, its hosting workstreams H1 to H5),
the roadmap's W15 items O4 to O9 (`docs/plans/roadmap.md:698-730`, `:2085-2175`), and the
health check's A, G and H reports. Everything here was read, not run.

**What this plan keeps from the earlier one, and what it changes.** It keeps: one codebase and
one release; hosting adds operations, never features; one shared database per cell with every
row in a workspace; no agent code on shared machines; export and import as the promise behind
the free plan; the free plan's shape (whole product, 10 people, 2 GB, full history); AGPL for
the app and a permissive licence for SDKs, protocols and the package format; the one-time
setup code for first run (roadmap O8). It changes:

- the old H1 to H5 are split into about forty one-PR slices, and the cheap tenancy work (a
  workspace column on every table, per-workspace limits, export declarations) moves to the
  front, before any image or sign-up;
- PostgreSQL row-level security is added as a second net behind the isolation tests (the earlier
  plan relied on the tests alone);
- "Mail is set per workspace" becomes "a server mail transport for account mail, workspace SMTP
  kept as an option";
- two facts the earlier plan and this task's brief state are already out of date: the
  `ai.base_url` address check is done (`ai/gateway.rs:373-418`), and the Red Hat backup already
  archives uploaded files (`deploy/redhat/agent-workspace-backup:28-31`, `:129-155`). What is
  missing is a backup that works anywhere other than that one host;
- the staff console recommends **no impersonation**; support sees content only when a workspace
  invites a support account with a role and an end date;
- the SDK licence becomes "MIT OR Apache-2.0" (the Rust convention, which adds a patent grant)
  rather than MIT alone, and contributions use a sign-off (DCO), not a CLA. Both are owner calls.

Slices marked **[owner now]** are worth building for the owner's own self-hosted server
regardless of when hosting starts.

---

## 1. Goal

When this workstream is done, anyone can run Agent Workspace with one command and a browser,
back it up and restore it, and move a whole workspace to another server and back without losing
anything but credentials. The same code runs a hosted service for many workspaces where every
row is fenced to its workspace twice (by tests and by the database), limits and metering live in
one place and only count what the host pays for, people can sign up, pay and leave, and staff
can run it without reading anyone's content. The public release happens when the owner chooses,
with nothing left in the repository that points at the owner's machines.

---

## 2. Where the code is today

### 2.1 Installing and running

- **One binary, configured by environment.** `AppConfig::from_values` (`config.rs:226-456`)
  reads `AGENT_WORKSPACE_*` variables. Required: `DATABASE_URL` (`:230`) and, outside
  development, `ALLOWED_ORIGINS` (`:825-846`). Defaults suit one small host:
  `DATABASE_MAX_CONNECTIONS=5` (`:231`), `DATA_ROOT=.agent-workspace` (`:232`),
  `WEB_ROOT=apps/web/dist` (`:235`), `JOB_CONCURRENCY=4` (`:409`). Harness settings sit in core
  config (`codex_*`, `claude_binary`, `anthropic_api_url`, `:94-108`, `:236-265`). Production
  refuses an `http` public origin even on loopback (`:748-767`), so a laptop trial of a
  production build needs a change.
- **Startup** (`main.rs:16-421`): connect, two legacy M3 preflights (`:42-61`),
  `sqlx::migrate!` (`:63-66`; sqlx 0.8.6 takes a PostgreSQL advisory lock while migrating),
  default workflow provisioning (`:67-75`), then CLI subcommands (`:77-320`), then run recovery
  of *every* non-terminal run (`:343-348`), plaintext webhook secret migration (`:361-371`), two
  listeners (`:372-388`) and six loops (`:390-421`). Nothing after the migrations is under a lock,
  so a second process starting up interrupts the first one's runs (G report F4).
- **Health**: `/health/live`, `/health/ready` (only `SELECT 1`), `/health/version`
  (`health.rs:17-58`). `preflight` (`preflight.rs:1-60`) checks config, keyring, harness CLIs,
  mail, last backup, disk and pending migrations; the release script runs it before switching.
- **No image.** The only compose file starts PostgreSQL for development
  (`docker-compose.dev.yml:1-21`). Self-hosting means following `docs/self-hosted-red-hat.md`
  (417 lines) with `deploy/redhat/*` scripts that name one user and host
  (`deploy-dev-checkout.sh:16-17`, `agent-workspace.service:12`, G report F11).
  `tools/ci/run.sh:67,93` holds tailnet addresses.
- **Web assets** are served by the server from `web_root` with an SPA fallback
  (`lib.rs:149-150`, `:223-226`), so one image can carry both.
- **Secrets**: envelope encryption with a `KeyEncryption` trait (`secrets.rs:1-21`, `:114-122`);
  each seal binds the secret's id and workspace, so a secret cannot be moved to another
  workspace or server anyway. The keyring file must sit outside the data root, the web root and
  any git work tree (`config.rs:380-395`, `:648-692`). `secret-key-init` and
  `secret-key-rotate` are CLI commands (`main.rs:129-162`). Without a keyring, agent webhook
  secrets stay in plaintext (A report finding 11).

### 2.2 First run, sign-in, email

- **First admin** only from an interactive terminal: `bootstrap-admin` (`bootstrap.rs:290-360`)
  takes an advisory lock and refuses once any user exists (`:298-311`). It also carries an M0
  "adopt an owner" path (`:526-580`).
- **Creating workspaces** happens in three places with three shapes: `bootstrap.rs:337`,
  `seed.rs:42`, and WorkOS self-signup, which hard-codes the name `'My workspace'`
  (`auth/workos.rs:508`). A password-only server cannot create a second workspace.
- **Server operator** is a login listed in `AGENT_WORKSPACE_OPERATOR_LOGINS` or whoever has an
  `identity.bootstrap_admin` audit row (`server_status.rs:43-68`): an audit row is a permission.
- **First-run path in the app** (`onboarding.rs:1-15`): template, project, GitHub, provider
  account, environment, tasks, agent, launch. It assumes the workspace already exists.
- **Sign-in methods**: password with two-factor (`auth/mod.rs:43-45`,
  `account_security.rs:53-86`); WorkOS (Google, sign-up when `allow_signup`) with its session
  checked on every request (`auth/mod.rs:532`, `auth/workos.rs:75`, `:166`, `:477`); WorkOS
  agent tokens (`auth/workos/agents.rs:26-32`). No plain OpenID Connect.
- **Email**: transport and messages in `mail/mod.rs:1-8`; SMTP only (`mail/smtp.rs`), configured
  **per workspace** (`workspace_mail_settings`, `mail/mod.rs:215-245`). Account mail (reset,
  address confirmation, `password_reset.rs:1-13`) borrows the SMTP of one of the person's
  workspaces (`mail/mod.rs:305-337`). A server with no workspace SMTP has no account mail.

### 2.3 Backups

The Red Hat scripts make an encrypted set per day: `pg_dump` plus a tar of `attachments`,
`artifacts`, `files` and `codex/profiles` under the data root, encrypted with `age`, copied
off-host, with the keyring backed up separately and a restore drill
(`docs/operations/backups.md:1-60`, `:172-226`; `deploy/redhat/agent-workspace-backup:28-31`).
This is good and stays as one example. It is tied to systemd, `age`, rsync over ssh and one host
layout, and it backs up local disk only (an object store would need its own story).

### 2.4 What is global that must be per workspace (or must say why not)

| Thing | Where | Today | Needed |
| --- | --- | --- | --- |
| Password sign-in ceiling | `rate_limit.rs:108-116`, called from `auth/mod.rs:190` | 60 attempts/min for the whole server, charged before the per-name bucket | Per address and per name; a server-wide ceiling only on *failures* |
| MCP and public API auth ceiling | `rate_limit.rs:164-172`, `mcp.rs:4336`, `public_api.rs:79` | 900/min for every agent on the server | Per credential and per workspace; global only for failed auth |
| Invitations, bug reports, password resets, OAuth ceremonies, WorkOS starts | 11 `FixedWindowLimiter<()>` in `rate_limit.rs:76-107` | One server-wide bucket each | Per workspace (bug reports, invitations) or per address, global only for junk |
| Client address | `rate_limit.rs:420-425` | Trusts `cf-connecting-ip` only | A configured trusted header, or none |
| OAuth client registrations | `mcp/oauth.rs:188-205` | 1,024 for the server; unused ones pruned after a day | Dynamic registration happens before anyone signs in, so it can't be per workspace: make the cap configurable and add a per-address registration limit |
| Job claiming | `jobs.rs:570-590` | `ORDER BY priority, run_at`; one workspace's flood delays all | A cap on running jobs per workspace |
| File quota | `files.rs:476-520`, setting `files.workspace_quota_mb` (`settings.rs:267-277`) | Set by the workspace's own admins (default 10 GB) | A plan ceiling the workspace can lower but not raise |
| Model spend | `ai.monthly_budget_usd` (`settings.rs:427`, `ai/gateway.rs:277-290`, `:498-504`) | A workspace's own budget over `ai_usage` | Kept as the workspace's own limit; plus a plan meter for calls on the host's key |
| Private addresses | `config.rs:417`, used in `ai/gateway.rs:383`, `mail/mod.rs:379` | One server switch | Stays server-wide; forced off on a hosted server |
| Sign-up | `WorkOsConfig.allow_signup` (`config.rs:146`) | One switch, WorkOS only | A server setting with modes (off, invite only, open) for every sign-in method |
| Mail | `workspace_mail_settings` | Per workspace only | A server transport for account mail |
| Harness concurrency | `CODEX_HOST_CONCURRENCY_LIMIT` (`config.rs:244`) | Server-wide | Leaves with the built-in host (A1) |

### 2.5 Tenancy in the data

- 186 tables created by 130 migrations (one dropped). By a scan of the `CREATE TABLE` bodies:
  **56 carry `organization_id`, 94 carry only `project_id`, 35 carry neither.** The 35 are
  global identity (`actors`, `users`, `sessions`, `login_challenges`, the `user_*` credential
  tables, `password_reset_tokens`, `workos_identities`, `workos_login_attempts`,
  `mcp_oauth_clients`, `organizations`, `secret_master_keys_retired`) and child tables
  (`whiteboard_elements`, `whiteboard_revisions`, `project_page_revisions`, `agent_run_events`,
  `push_deliveries`, `notification_occurrences`, `conversation_entries`, `conversation_pins`,
  `idempotency_keys`, `mcp_oauth_access_tokens`, and others).
- Composite foreign keys stop a row pointing across projects (G report section 1). 32 foreign
  keys to `organizations(id)` have no `ON DELETE CASCADE`, and there is no way to delete a
  workspace (`grep "DELETE FROM organizations"` finds nothing).
- **The isolation nets are strong and must be kept as they are**:
  `tests/integration/workspace_isolation.rs` (every GET route and read-only tool, marker plus
  id sweep, `:273-333`, fixture `TwoWorkspaces` at `:558-750`) and
  `tests/integration/write_isolation.rs` (every write route and write tool, a digest of every
  row naming a B id before and after, `:1-37`, `:549-655`), each with a source-scanning guard
  that fails when a new route has no case.
- No row-level security anywhere. The agent-limits task-local (`policy.rs:777-800`) is a working
  precedent for "a per-call scope every query can see".

### 2.6 Outbound calls a workspace can aim

`outbound.rs:1-90` resolves, checks every address and pins; AI (`ai/gateway.rs:373-418`) and
mail (`mail/smtp.rs:95-98`, `:341-345`) use it. Outgoing webhooks and agent wakeups use a second
guard that pins only the first address (`replies.rs:1638-1680`, used by `webhooks.rs:1311`), and
link previews a third (`link_previews/guard.rs:11-104`). Push endpoints are limited to known push
services (`push.rs:54`, `:215-230`). GitHub, WorkOS and the cloud harness clients call fixed
operator-configured addresses. Future host links (A1) and connected modules (M1) will add more.

### 2.7 Export and import

- Tasks only, per project: JSON or CSV, capped at 20,000 tasks (`work_items/imports.rs:66`,
  `:2206-2290`), and the `project_export` tool truncating at 500 (`mcp/views_data.rs:1585-1632`).
- Single things: a page as Markdown or Word (`page_docs/transfer.rs:85`, `page_docs/docx.rs:61`),
  a sheet (`sheets/http.rs:1048`).
- Structure only: workspace templates (`workspace_templates.rs:1-23`), a precursor of M1's
  packages.
- Module manifests (`modules/*.json`, read by `modules/manifest.rs:76-124`) have no export
  section. Nothing exports conversations, pages in bulk, whiteboards, collections, files, roles,
  agents or history. `docs/operations/abuse-controls.md:43-47` still says no export endpoint
  or upload exists, which is stale.

### 2.8 Usage and capacity

- `ai_usage` logs every model call with tokens and cost, never content
  (`migrations/20261017000000_ai_planning.sql:10-36`); the gateway checks a monthly budget.
- `usage.rs` and `usage/*` are about *provider accounts'* windows (a harness subscription's five
  hours or week), owned by one person (`usage.rs:1-15`, `usage/alerts.rs:1-13`). They are not
  workspace metering and stay with A1.
- `storage_admission.rs` guards the server's disk; `files::require_room` adds the workspace quota.
- Metrics are Prometheus text with a token (`metrics.rs:1-16`); counters are per process, gauges
  are read from the database.

### 2.9 What has the right shape and must be kept

The `FileStore` seam (`files/store.rs:1-70`), `KeyEncryption`, `outbound::resolve`, the job queue
with leases and `SKIP LOCKED`, the isolation nets and their guards, the settings registry
(`settings.rs:1-24`), `ai_usage` as a cost ledger, the backup design (separate key, drill), the
preflight report, the reset and verify-email flows (`password_reset.rs:1-13`), `logged_path`
(route templates in logs), and secrets that can never leave their workspace.

---

## 3. The target design

### 3.1 Self-host in one command

**Image.** One multi-stage `Containerfile` at `deploy/container/Containerfile` builds:

- `agent-workspace-server` (release build), with the web build copied to `/app/web`;
- A1's built-in host program as a **separate image** (`agent-workspace-host`), because it needs
  provider command-line tools and the operator's folders, and the server image must not;
- a small runtime: `git` (the code module still mirrors repositories until A1 moves that out),
  `postgresql-client` (for the backup service), CA certificates, a non-root user.

Published for amd64 and arm64, tagged by release number and commit, with a software bill of
materials and the third-party notices file inside (`/app/THIRD-PARTY-NOTICES`). The server
image runs `agent-workspace-server` with no arguments, which migrates and serves.

**Compose.** `deploy/container/compose.yml`:

```yaml
services:
  db:
    image: postgres:18-alpine            # pinned by digest in the file
    environment: { POSTGRES_DB: agent_workspace, POSTGRES_USER: aw_admin,
                   POSTGRES_PASSWORD_FILE: /run/secrets/db_admin_password }
    volumes: [ "db:/var/lib/postgresql", "./initdb:/docker-entrypoint-initdb.d:ro" ]
    healthcheck: { test: ["CMD-SHELL", "pg_isready -U aw_admin -d agent_workspace"] }
  app:
    image: ghcr.io/<published>/agent-workspace:${AW_VERSION:-latest}
    depends_on: { db: { condition: service_healthy } }
    environment:
      AGENT_WORKSPACE_ENV: production
      AGENT_WORKSPACE_SERVER_ADDR: 0.0.0.0:3100
      AGENT_WORKSPACE_PUBLIC_ORIGIN: ${AW_PUBLIC_ORIGIN:-http://localhost:3100}
      AGENT_WORKSPACE_DATABASE_URL_FILE: /run/secrets/db_app_url
      AGENT_WORKSPACE_DATA_ROOT: /var/lib/agent-workspace
      AGENT_WORKSPACE_SECRETS_KEY_PATH: /var/lib/agent-workspace-keys/keyring
      AGENT_WORKSPACE_PROFILE: ${AW_PROFILE:-standard}
    volumes: [ "data:/var/lib/agent-workspace", "keys:/var/lib/agent-workspace-keys" ]
    ports: [ "127.0.0.1:3100:3100" ]
    healthcheck: { test: ["CMD", "agent-workspace-server", "health"] }
  backup:                                  # profile "backup"
    image: ghcr.io/<published>/agent-workspace:${AW_VERSION:-latest}
    command: ["agent-workspace-server", "backup", "schedule"]
    volumes: [ "data:/var/lib/agent-workspace:ro", "keys:/var/lib/agent-workspace-keys:ro", "./backups:/backups" ]
  proxy:                                   # profile "tls": automatic certificates
    image: caddy:2                         # pinned
  host:                                    # profile "host": A1's built-in host
    image: ghcr.io/<published>/agent-workspace-host:${AW_VERSION:-latest}
    # A1.17: the server writes a one-time enrolment code into the shared `hostenrol` volume on
    # first start; the host reads it, enrols through the public host protocol (A1 3.9) and dials
    # out to http://app:3100. It shares nothing else with the server.
    volumes: [ "hostenrol:/run/agent-workspace-enrol:ro", "hostdata:/var/lib/agent-workspace-host",
               "${AW_AGENT_FOLDERS:-./agent-folders}:/work" ]
volumes: { db: {}, data: {}, keys: {}, hostenrol: {}, hostdata: {} }
```

The `host` profile ships A1's built-in host program (`agent-workspace-host`, A1.17) and
nothing else: it is one more host under A1's protocol, with its own store (a schema in the same
database or its own, A1 3.11), in folders the operator allows. Before A1.17 the server image
runs the built-in host embedded (`AGENT_WORKSPACE_BUILTIN_HOST=on`, K5.3), and the profile is
absent.

A one-line `deploy/container/init.sh` writes `.env` and the two secret files with random
values, then `docker compose up -d` (Podman Compose works the same). `initdb/10-app-role.sql`
creates a **non-superuser** role `aw_app` that owns the schema and that the server connects as.
This matters: the official PostgreSQL image's `POSTGRES_USER` is a superuser, and superusers
skip row-level security (3.4).

**Configuration with safe defaults** (H1.12):

- `*_FILE` variants for every secret-bearing variable (`DATABASE_URL_FILE`, etc.).
- `ALLOWED_ORIGINS` defaults to `PUBLIC_ORIGIN` in production when unset (today it is required,
  `config.rs:840-842`).
- A production `PUBLIC_ORIGIN` may be `http://localhost` or a loopback address, so a trial on a
  laptop works; any other host must be `https`. Browsers treat localhost as secure, so the
  `__Host-` cookies still work.
- `AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER` (none by default; `cf-connecting-ip`,
  `x-forwarded-for` with a trusted hop count) replaces the hard-coded header.
- If `SECRETS_KEY_PATH` names a missing file in a writable directory outside the data root,
  first start creates the keyring there (the logic `secret-key-init` already has) and logs the
  path, never the key.
- `AGENT_WORKSPACE_PROFILE`: `small` (pool 4, job concurrency 1, link previews off, metrics off,
  sized for a 1 vCPU, 1 GB machine; the numbers are a guess until measured), `standard` (today's
  defaults), `hosted` (3.9). A profile only sets defaults; any variable still wins.
- `docs/self-host/configuration.md` lists every variable; a unit test parses `config.rs` for
  `optional(values, "...")`, `parse(values, "...")` and `required(values, "...")` keys and fails
  when one is missing from the document.
- Harness variables move out of core config with A1. Until then they keep their defaults and
  the image does not need them.

**First run in the browser** (H1.14), as roadmap O8 describes. On start, when `users` is empty,
the server writes a random setup code to `<data root>/setup-code` (mode 0600) and logs only the
file's path and `docker compose exec app cat /var/lib/agent-workspace/setup-code`. The web app
shows `/setup` while no user exists: enter the code, then a name, login and password (or "sign
in with your provider" when OIDC is configured), a workspace name and address, and **what it
is for**: a shape (R22; M1's packages, installed by M1.17: Friends, which is chat only; Team;
Just me and my agents; Documents; Blank). The shape decides the modules and the starting
layout; it can be changed later and nothing is deleted (today's onboarding picks up from
there, skipping steps for modules that are off). A second tab: **"Import a workspace"** (H1.10). The
code is single use, compared in constant time, rate limited per address, and the file is deleted
once the first admin exists. The endpoint is `POST /api/v1/setup` with
`{ code, person: {...}, workspace: {...} }`; it calls the one `workspace_lifecycle::create`
(3.6) and K1's `accounts::create_person`, inside the same advisory lock `bootstrap-admin` uses
(`bootstrap.rs:298-302`), taken as a transaction-scoped lock (`pg_advisory_xact_lock`) so it
is pooler-safe (R24 rule 3). `bootstrap-admin` stays for terminals and calls the same functions.

**Upgrades and migrations on start** (H1.15). The server takes K5's lease
`server.startup` before the legacy preflights, migrations, workflow provisioning and the
plaintext-secret migration; other processes wait on the lease (bounded, then exit non-zero so the
orchestrator retries). `/health/ready` answers 503 `{"status":"starting","step":"migrating"}`
until startup is done, and 503 when the database's last migration is newer than the build (an
old image started against a new schema). Run recovery is under K5.3's `builtin-host` lease (one
holder), and leaves the server with A1.17.
Releases are numbered (`0.N.0` until the public release), with notes in `CHANGELOG.md`. The
Server page and `preflight` say when a newer release exists, from a public JSON file the operator
can switch off (`AGENT_WORKSPACE_UPDATE_CHECK=off`; on by default, never sends anything but the
running version in the user agent).

**Health endpoints.** `live` unchanged. `ready` adds: migrations applied, keyring opens when one
is configured, the file store answers a `head` on a probe key. `version` unchanged. A new
`agent-workspace-server health` subcommand calls `ready` on loopback for container health checks.

**Backups and restore covering database and files** (H1.16). A built-in command, so it works in
any container or host:

- `agent-workspace-server backup create --to <dir>` writes the same set format the Red Hat
  scripts write (`database.dump`, `data.tar.gz`, `backup-info`, `SHA256SUMS`), optionally
  encrypted to an age recipient (`--recipient-file`), using `pg_dump` from the image. For a
  local file store it archives `files` (and the other unrebuildable paths); for an object store
  (K5) it records the bucket and the newest object version instead and the docs say to switch
  on the bucket's own versioning.
- `backup schedule` runs `create` daily at a configured time, keeps `BACKUP_KEEP_DAYS`, and
  writes the status file that `preflight` and the Server page already read
  (`server_status.rs`, `backup_status_path`).
- `backup key` exports the keyring, encrypted to a *different* recipient, to a different place,
  as the Red Hat design insists.
- `restore --from <set> --database-url <empty db>` restores into an empty database and data
  root and refuses a non-empty one; `restore drill` restores into a spare database and checks
  counts, migration version and that the keyring opens every secret (porting
  `agent-workspace-restore-drill`).
- The Red Hat scripts stay and keep working; their documentation points to the built-in command
  as the portable path.

**Running without any outside service.** With no WorkOS, GitHub, mail, object store, model key,
metrics or push configured, every module works: sign-in by password, invitations by copied link,
password reset by `user-password-rotate`, files on disk, no link previews of outside pages unless
switched on. The web bundle loads nothing from another origin. Two checks hold this (section 6).

### 3.2 Export and import of a whole workspace

**Who and how.** Operation `workspace_export` (K2 operation, agent tool, route
`POST /api/v1/workspaces/{id}/exports`), permission `workspace.export` (new in K1's catalogue;
owners hold it by default), result: a transfer id. A job writes the archive to the file store
under `exports/<workspace>/<transfer>.awx`; the person is notified with a download link that
needs their session and expires after 7 days. `workspace_import` takes an uploaded archive (web,
up to the upload limit) or a path on the server (`agent-workspace-server workspace-import
--file`, for large archives and for first run).

**Archive format, version 1.** A zip file (ZIP64), extension `.awx`. Zip, not tar: an import that
stops must resume at an entry without reading everything before it, and zip has a central
directory for that; files inside are stored without recompression.

```
manifest.json
workspace/workspace.json          name, address, settings, modules on/off, layouts
accounts/people.jsonl             profile, handle, emails (with verified flag), avatar file id
accounts/agents.jsonl             profile, managers, outside reference, subscriptions; no credentials
accounts/apps.jsonl               connected apps: name, scopes, who approved; no tokens
accounts/memberships.jsonl        workspace and space memberships with roles
roles/roles.jsonl
spaces/spaces.jsonl               projects and hidden kinds (until K4's spaces)
things/<module>/<kind>.jsonl      in the order the modules declare
links/links.jsonl                 K3's one link store (until then each module's link tables)
history/events-000001.jsonl       K3's event log for this workspace (the audit trail)
files/index.jsonl                 id, sha256, size, content type, owning thing
files/blobs/<sha256>              one per distinct content
needs/secrets.jsonl               names and owners of secrets to re-enter; never values
needs/connections.jsonl           agents, apps, hosts and code hosts to reconnect
```

Every line is `{"kind":"task","v":4,"id":"0193...","data":{...}}`. `v` is the module's export
version for that kind; a module keeps an upgrade function from each older version it still
reads (the current and the previous two).

```json
{
  "format": "agent-workspace-export",
  "formatVersion": 1,
  "exportedAt": "2026-11-14T10:00:00Z",
  "source": { "serverVersion": "0.9.0", "schemaVersion": 20261113203000, "origin": "https://example.org" },
  "workspace": { "id": "0193a...", "slug": "acme", "name": "Acme" },
  "idMode": "preserve",
  "modules": { "tasks": { "version": 4, "kinds": { "task": 1234, "task_comment": 5300 } } },
  "files": { "count": 812, "bytes": 1503238553 },
  "excluded": ["credentials", "sessions", "secret values", "delivery logs", "search index"]
}
```

**Ids are preserved, and remapped only when they would collide.** Ids are UUIDv7 made by the
server, so they don't collide between servers in practice. Preserving them keeps every link
inside text working (references are parsed from URLs that contain ids,
`20261113060000_thing_references.sql:37-60`), keeps an agent's own notes and outside systems
pointing at the right task, and is what moving a workspace between cells will need. The importer
first loads every top-level id into a temporary table and checks it against the target; if any
exists (importing a copy onto the server it came from), it switches to **copy mode**: every id
gets a fresh UUIDv7 recorded in `workspace_import_ids`, and every declared reference, including
ids inside text and URLs, is rewritten through the map. Task keys (`AWD-142`) are kept in both
modes, so people's habits survive.

**Each module declares how its things export** (H1.8), through M1's registrar: each module
calls `Registrar::export(&'static dyn ExportPart)` (M1 3.2, M1.3) with an `ExportPart` that
carries its declaration. There is no `export` section in `modules/*.json`. The declaration's
shape, written here as JSON for reading (in code it is a `const ExportDecl`):

```json
"export": {
  "version": 4,
  "after": ["projects", "fields"],
  "kinds": [
    { "kind": "task", "table": "work_items",
      "references": { "parent_id": "task", "assignee_actor_id": "account", "status_id": "task_status" },
      "textReferences": ["description"],
      "files": [],
      "exclude": [] },
    { "kind": "task_comment", "table": "work_item_comments", "references": { "work_item_id": "task" } }
  ],
  "notExported": { "work_item_worker_invocations": "runtime state of a run on this server" }
}
```

The trait is named `ExportPart`, the slot M1's registry declares
(`Registrar::export(&'static dyn ExportPart)`, M1 3.2 and M1.3); H1.8 writes the parts for the
core modules and registers them there.

```rust
pub trait ExportPart: Send + Sync {
    fn kind(&self) -> &'static str;
    fn version(&self) -> u32;
    async fn export(&self, cx: &mut ExportCx<'_>, out: &mut KindWriter) -> Result<(), ExportError>;
    async fn import(&self, cx: &mut ImportCx<'_>, batch: Vec<Line>) -> Result<(), ImportError>;
    fn upgrade(&self, from: u32, line: Line) -> Result<Line, ImportError> { /* default: refuse */ }
}
```

Most kinds need no hand code: a generic exporter reads the declared table filtered by its
tenant column (from H1.3's per-table list: `organization_id` on the tables that already had
one, `workspace_id` on the tables 3.3 adds, per ruling R1), writes every column except
excluded ones, and the generic importer inserts
in order with references mapped. Hand-written `ExportPart`s are for things with derived state
(live page documents, sheets' cells, whiteboard elements) or files.

**What goes in, what never does.**

- Accounts and roles: a **profile stub** for each member account of any kind (people with
  profile, handles and email addresses; agents; apps), the memberships, roles and the
  new-agent default role (R19: accounts are global, so the archive carries what this
  workspace knows of them, never their other workspaces).
- Agents: profile, owner (as "this workspace" or "a person", never another workspace's id),
  managers, roles, memberships, this workspace's subscriptions, the **outside reference**
  (issuer, outside id, lineage). Never credentials, tokens, OAuth grants or webhook secrets.
  After import each agent shows "Needs connecting" and its managers reconnect it through A2's
  reconnect path (A2.18); the outside reference lets the host re-attach.
- Conversations: only those that carry this workspace's id (R20). DMs and groups in the
  direct scope belong to their participants, not to the workspace, so a workspace export never
  holds them; in a `kept` workspace (`direct_messages = kept`) its members' DMs carry its id
  and are exported with it.
- History in full: message and comment revisions, page revisions, task activity and timeline,
  record history, sheet revisions, the event log. Never cut.
- Files: every stored blob, deduplicated by sha256, with thumbnails regenerated on import.
- Never: password hashes, two-factor secrets, recovery codes, sessions, secret values (they are
  sealed to their workspace id and server key anyway, `secrets.rs:5-9`), delivery and job logs,
  search vectors, caches, rate-limit state. `needs/secrets.jsonl` lists what to re-enter.
- Code: repositories and pull request records as history; GitHub installations are listed under
  `needs/connections.jsonl` because an installation id means nothing to another server's app.

**Accounts on the target server.** Accounts are server-global (K1, R19). Before import
starts, the importing admin sees a mapping screen: each person in the archive is matched by
verified email, and each agent or app by its outside reference (issuer and outside id), to an
existing account on the target; otherwise it is created (a person as a new **unclaimed**
account: no password, no sign-in until claimed; an agent or app owned by the new workspace,
"Needs connecting"). Matching an existing agent makes it a member here only after a manager of
that agent agrees, as any join does (R19). The mapping is applied as an id map while inserting, in both id
modes, so history stays attributed. The importing admin is always mapped to themselves. On
completion, unclaimed people can be sent a claim email (if the server has mail) or the admin
copies claim links; claiming sets a password or links a sign-in provider. A matched existing
account gets a notice that it was added.

**Import is a resumable job.**

```sql
CREATE TABLE workspace_transfers (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),   -- the new workspace, for imports
    direction TEXT NOT NULL CHECK (direction IN ('export','import')),
    state TEXT NOT NULL CHECK (state IN ('queued','mapping','running','paused','succeeded','failed','canceled')),
    requested_by_actor_id UUID NOT NULL REFERENCES actors(id),
    file_key TEXT CHECK (file_key IS NULL OR char_length(file_key) <= 200),
    id_mode TEXT CHECK (id_mode IN ('preserve','copy')),
    cursor JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(cursor::text) <= 4096),
    counts JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(counts::text) <= 65536),
    people_map JSONB CHECK (people_map IS NULL OR octet_length(people_map::text) <= 1048576),
    error TEXT CHECK (error IS NULL OR char_length(error) <= 1000),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ
);
CREATE INDEX workspace_transfers_workspace ON workspace_transfers (workspace_id, created_at DESC);

CREATE TABLE workspace_import_ids (
    transfer_id UUID NOT NULL REFERENCES workspace_transfers(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (char_length(kind) <= 80),
    source_id UUID NOT NULL,
    target_id UUID NOT NULL,
    PRIMARY KEY (transfer_id, kind, source_id)
);
```

The job (`workspace.import`, on the job queue) creates the workspace in state `importing`
(3.6), then walks the manifest's order. Each batch (500 lines or 8 MB) is inserted in one
transaction together with the cursor update (`{"entry":"things/tasks/task.jsonl","line":12500}`),
so a crash resumes exactly. The job runs under the new workspace's tenancy scope (3.4), so a
forged archive cannot touch another workspace. Imports write no outbox events, notifications,
webhooks or automation firings; one `workspace.imported` event is written at the end, then a
search reindex job and counter recomputation (`project_counters`). A failed import can be resumed
or abandoned; abandoning runs workspace deletion.

**Safety of an archive.** Entry names are checked (no `..`, no absolute paths), total
uncompressed size and ratio are bounded, every line is size-bounded (16 MB), every file's sha256
and size must match the index, every reference must resolve inside the archive or to a mapped
person, and unknown kinds are refused with the module that would read them named.

**Limits.** Self-hosted: none beyond disk. Hosted: one export per workspace per day, kept 7
days, not counted against storage; an import's files count against the plan's storage, and the
archive may be at most the plan's storage plus 5 GB.

**Moving.** Hosted to self-hosted: export, `init.sh`, then "Import a workspace" at first run.
Self-hosted to hosted: sign up, then "Import" in the new workspace's settings (or at sign-up).
Agents move host by their host's own means and their outside reference re-attaches (A2).

**A person's own DMs, and the whole server (R20).** Operation `direct_export` (any signed-in
person, for themselves) writes an archive of the DMs and groups they take part in, with the
messages and files in them, in the same format with `"scope": "direct"` and only the
participants' profile stubs; it is the person's copy and imports nowhere automatically. The
operator's **full-server export** (`agent-workspace-server server-export`, terminal only)
writes every workspace plus the direct scope and every global table except credential
secrets, for moving a whole server; `server-import` restores it into an empty server with ids
preserved.

### 3.3 Every table carries its workspace

Before row security, export by workspace or deletion can be simple, every workspace-owned table
needs the workspace column. About 129 tables lack it (94 project-only, 21 child tables of the 35;
the remaining 14 are global, listed with reasons in 6.2). They get a column **named
`workspace_id`** (ruling R1), whose foreign key points at `organizations(id)` until K4.22. The
56 tables that already carry `organization_id` keep that name (only K4's plan renames existing
columns). So every policy, export query, isolation guard and generator reads the tenant column
name from H1.3's per-table list (`tenancy/tables.rs`: table → `organization_id` or
`workspace_id`), and both names work. Per batch of tables (H1.4 to H1.7):

1. **Expand**: `ALTER TABLE t ADD COLUMN workspace_id UUID REFERENCES organizations(id)`
   (nullable, no default; the foreign key added `NOT VALID`, so the statement is instant).
   A `BEFORE INSERT` trigger `fill_workspace_from_project()` (or from the parent row for child
   tables) sets it when the writer didn't. Rust writers are not changed in this step.
2. **Copy**: a job `tenancy.backfill` with payload `{table, after_id}` updates 5,000 rows at a
   time (`UPDATE t SET workspace_id = p.organization_id FROM projects p WHERE ... AND t.id >
   $after ORDER BY id LIMIT 5000`), re-enqueuing itself with the last id, so it resumes after a
   restart. It is queued by the migration (`INSERT INTO background_jobs ... ON CONFLICT DO
   NOTHING` on a unique key), so production's batch of pending migrations is safe: the column
   exists and the trigger fills new rows even before the job runs.
3. **Switch**: once a table's backfill job reports zero rows left, a later migration adds
   `CHECK (workspace_id IS NOT NULL) NOT VALID`, then `VALIDATE CONSTRAINT` (which only takes
   a light lock) and validates the foreign key, plus an index `(workspace_id)` where the table
   has no index leading with it.
   That migration must tolerate a backfill that hasn't finished: it runs the remaining backfill
   inline in batches first.
4. **Contract**: none needed; the trigger stays as a convenience and writers may set the column.

**Triggers this plan adds, per table (ruling R16).**

| Table(s) | Trigger | Writes | Removed |
| --- | --- | --- | --- |
| Every table in H1.4 to H1.7's batches (about 129) | `<table>_fill_workspace`, `BEFORE INSERT`, calls `fill_workspace_from_project()` or `fill_workspace_from_parent()` | Only the new row's own `workspace_id`, and only when it is NULL; never another table | Not removed: it is the writer-independent fill that keeps a missed writer correct. K4.22 keeps it on the renamed base tables. Because it writes no other table, it cannot double up with K3's thing sync, T1's task and record sync, A1's run mirror or C1's freeze triggers on the same tables (all `AFTER` triggers writing other tables) |
| `organizations` | none | | |

No H1 trigger writes another table. Tables born after this plan (A1's `runs`, `requests`,
`host_commands`; A2's per-workspace tables; K3's `events`, `links`, `things`; T1's and C1's
new tables) are created with `workspace_id NOT NULL` and get no fill trigger. Global tables
born after it (A1's `hosts`, `host_agents`, `host_enrolments`; A2's `agent_connections`;
K1's account and credential tables) carry no tenant column and are on H1.3's global list.

### 3.4 Row-level security as a second net

**The scope.** A tokio task-local `TENANCY` (beside `AGENT_LIMITS`, `policy.rs:777`) holds one
of:

```rust
pub enum Tenancy {
    /// A signed-in person, agent or app: the workspaces it belongs to now.
    Member { workspaces: SmallVec<[Uuid; 4]> },
    /// One workspace: a job, an import, a public form or bug report link resolved to its owner.
    Workspace(Uuid),
    /// Server-wide work that must read across workspaces (claiming jobs, retention, metrics
    /// gauges, the staff console). Each use names a registered reason.
    System(&'static str),
    /// Before authentication: only global identity tables are readable.
    Anonymous,
}
pub async fn scoped<F: Future>(tenancy: Tenancy, f: F) -> F::Output;
pub fn system<F: Future>(reason: &'static str, f: F) -> impl Future<Output = F::Output>;
```

Authentication middleware (browser session, MCP principal, public API) wraps the rest of the
request in `scoped(Tenancy::Member { workspaces })` from the account's enabled memberships. Job
workers wrap each job in `Workspace(job.organization_id)` (the column exists,
`background_jobs.organization_id`), or `System` for the few cross-workspace kinds.

**Narrowing to one workspace per request (ruling R4).** Requests to K2's generated API carry
the workspace in the path (`/api/v2/w/{workspace}/...`, id or slug). K2's runner resolves it,
refuses a path workspace the caller isn't a member of, and runs the dispatch inside
`scoped(Tenancy::Workspace(id))`; inside its transaction it also runs
`SELECT set_config('app.tenancy', 'w:<id>', true)` (transaction-local), so a connection
acquired earlier under the member scope is narrowed too. This is the one key the row rules
read; there is no separate `app.workspace_id` setting. MCP stays one endpoint where the
credential decides the workspace: any account, of any kind, may be in several workspaces
(R19), so a credential that reaches several gets `Member` over the workspaces it reaches,
narrowed to `Workspace` by the optional `workspace` argument that every operation call
carries; a credential narrowed to one workspace (K1's narrow tokens) gets `Workspace`; the
alias `/w/<slug>/mcp` sets `Workspace` and is refused when it disagrees with the credential.
Today's v1 routes keep the `Member` scope until retired.

**Setting it, pooler-safe (R24 rule 3).** The scope is set only by
`set_config('app.tenancy', $1, true)` (the same as `SET LOCAL`) as the first statement of a
transaction, with the current task-local serialised (`m:<account>|<uuid>,<uuid>`,
`w:<account>|<uuid>`, `s:<reason>`, or `a`); `SET LOCAL statement_timeout` rides in the same
statement. Never a session setting and never `before_acquire`: a transaction-mode pooler
hands a connection to someone else after each transaction, and a session setting would leak
one request's scope into the next. One helper, `db::scoped_tx(&pool)`, begins the
transaction and sets both; K2's runner uses it for every operation, and jobs use it per unit
of work. Transactions stay short and are never held across a network call (the pusher, A1's
host calls and model calls commit before they send). Most of today's queries run on the pool
outside a transaction (about 2,400 call sites, G report F9). They see no scope; in observe
mode that is allowed and logged (below), and H1.45 to H1.62 move the call sites into
`scoped_tx` or onto K2's runner, one module directory each, behind H1.44's ratchet, before
H1.63 turns strict mode on (R42). A single-statement
read can instead use `db::scoped_one(&pool, query)`, which sends `BEGIN`, the setting, the
query and `COMMIT` as one pipelined batch.

**The policy.** One SQL function and one generated policy per table:

```sql
CREATE FUNCTION tenancy_allows(workspace UUID) RETURNS boolean
LANGUAGE sql STABLE PARALLEL SAFE AS $$
  SELECT CASE split_part(current_setting('app.tenancy', true), ':', 1)
    WHEN 's' THEN true
    WHEN 'w' THEN workspace = split_part(current_setting('app.tenancy', true), ':', 2)::uuid
    WHEN 'm' THEN workspace = ANY (string_to_array(split_part(current_setting('app.tenancy', true), ':', 2), ',')::uuid[])
    WHEN 'a' THEN false
    ELSE current_setting('app.tenancy_strict', true) IS DISTINCT FROM 'on'   -- unset: observe mode
  END $$;

ALTER TABLE work_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE work_items FORCE ROW LEVEL SECURITY;
CREATE POLICY tenancy ON work_items USING (tenancy_allows(workspace_id))
  WITH CHECK (tenancy_allows(workspace_id));
```

The generator reads each table's tenant column from H1.3's list, so a table that already had
`organization_id` gets `tenancy_allows(organization_id)` and one that got the column here gets
`tenancy_allows(workspace_id)` (ruling R1).

**The direct scope (R20).** The reserved `organizations` row with `kind = 'direct'` is never
in a `Member` or `Workspace` scope (no membership row may reference it; H1.3's guard checks
this). Tables that may hold direct-scope rows (conversations and what hangs off them: messages,
files, reactions, reply requests, A1's runs and requests raised from a DM) get a second
generated policy branch: `OR (workspace_id = direct_scope_id() AND
conversation_member_visible(conversation_id, tenancy_account()))`, where `tenancy_account()`
reads the account part of `app.tenancy`. So row rules treat the direct scope as participants
only: no role, admin, workspace owner or `System` reason other than `retention`, `jobs.claim`
and the operator's full-server export reads it, and none of those is reachable from a
workspace's own screens. Global tables (H1.3's list) need no branch.

`FORCE` makes the table owner subject too, so self-hosters running one database role get the net
as long as that role isn't a superuser (the compose init creates `aw_app` for this). The
database's server-wide strictness is `ALTER DATABASE ... SET app.tenancy_strict = 'on'`, set by
H1.63 once the call-site ratchet reads zero (R42); until then an unset scope is allowed and logged.

**Rollout, table by table.** H1.20 adds the task-local, `scoped_tx` and `scoped_one`, the log
span and an *observe* mode: when a query on a tenant table runs with no scope set (detected by
`tenancy_allows` taking its `ELSE` branch, which calls a logging function once per statement
fingerprint), it logs once per call site
(`tracing` target `agent_workspace::tenancy`, with the route template and the job kind) and
counts it in metrics. H1.21 enables policies on the first batch (chat tables) and fixes every
unscoped path the observe log shows for them; H1.22 adds the rest of the policies in observe
mode. The call sites are scoped by H1.45 to H1.62, one module directory each, in any order,
behind H1.44's ratchet (a count of unscoped call sites that may only go down); H1.63 turns
strict mode on when it reaches zero (R42). Each batch runs both isolation nets with the test pool connecting as a
non-superuser role (`SET ROLE aw_test_app` in the test pool's `after_connect`), so the nets prove
the policies and the policies back the nets.

**Cost.** One extra statement per transaction for `set_config`, pipelined with `BEGIN` (well
under a millisecond on a local database; H1.20 measures it with the existing performance
notes, `docs/operations/performance.md`). The policy adds an equality or `= ANY` filter on a
column that is indexed or sits beside an indexed project or id filter. Because nothing is set
on the session, a transaction-mode pooler in front of PostgreSQL is safe, which the "edge"
shape needs (`docs/hosting/shapes.md`); K5's lint for session settings, session advisory locks
and hand-written `PREPARE` covers H1's code too.

**The escape for cross-workspace work.** `tenancy::system(reason, ...)` with a reason from a
fixed list in `tenancy.rs` (`"jobs.claim"`, `"outbox.claim"`, `"retention"`, `"metrics.gauges"`,
`"staff.console"`, `"auth.lookup"`, ...). A guard test fails on a `system(` call whose reason isn't
in the list or that appears outside the files the list names. Authentication lookups (tokens,
sessions, OAuth grants) run under `auth.lookup` because they happen before the workspaces are
known.

### 3.5 Per-workspace limits, noisy neighbours, outbound calls, logging

- **Limits** (H1.1): every limiter is declared with its scope: `Address`, `Login`,
  `Credential`, `Account`, `Workspace`, or `ServerFailuresOnly`. Successful sign-ins and valid
  MCP calls never touch a server-wide bucket; server-wide ceilings count only failures and junk.
  New per-workspace ceilings: MCP and public API calls (3,000/min per workspace, beside 600/min
  per credential), invitations, bug reports and public form submissions. K5 makes the store
  shared across processes; H1.1 only changes keys and scopes, so it lands on today's in-memory
  limiter or on K5's, whichever is there.
- **Noisy neighbours** (H1.23): job claiming skips workspaces with more than
  `JOB_RUNNING_PER_WORKSPACE` (default 4) running jobs (a `NOT IN (SELECT organization_id ...
  GROUP BY ... HAVING count(*) >= $n)` filter in the claim CTE, `jobs.rs:570-580`, with an index
  on `(state, organization_id)`); a per-workspace cap on concurrent outbound deliveries, exports,
  imports and search queries; `statement_timeout` of 15 s on request transactions (`SET
  LOCAL` in `scoped_tx`) and longer only under `System("export")`; per-workspace caps on open realtime
  connections. A2's agent caps (open event streams per connection and per workspace, webhook
  pushes in flight per workspace, `event_list` calls per connection; A2 3.6) take their numbers
  from the plan's `limits` through K5's `LimitSource`, with A2's defaults on `self_hosted`.
- **Outbound calls** (H1.2): one module, `outbound`, builds every client a workspace can aim:
  webhooks, agent wakeups, link previews, model provider, mail, and later host links and
  connected modules. Every resolved address is checked and pinned (not only the first,
  `replies.rs:1672-1677`), redirects are off, proxies are off, and a per-workspace concurrency
  permit is taken. A hosted profile forces `ALLOW_PRIVATE_ADDRESSES=false` and ignores
  `WEBHOOK_TRUSTED_ORIGINS`.
- **Tenant-aware logging**: the tenancy span carries `workspace_id` (a UUID, never a name) and
  the request id; logs never carry message, page, task or file content (already the rule; a
  guard scans `tracing::` macros for field names such as `body`, `content`, `text`, `title`,
  `message_text`).

### 3.6 Workspace lifecycle

`organizations` gains `lifecycle TEXT NOT NULL DEFAULT 'active' CHECK (lifecycle IN
('active','importing','suspended','deleting'))`, `lifecycle_reason TEXT`, `delete_after
TIMESTAMPTZ`. One module, `workspace_lifecycle`:

```rust
pub async fn create(tx: &mut PgConnection, owner: AccountId, input: NewWorkspace) -> Result<WorkspaceId, AppError>;
pub async fn suspend(tx: &mut PgConnection, ws: WorkspaceId, reason: SuspendReason, by: Actor) -> Result<(), AppError>;
pub async fn resume(tx: &mut PgConnection, ws: WorkspaceId, by: Actor) -> Result<(), AppError>;
pub async fn request_deletion(tx: &mut PgConnection, ws: WorkspaceId, by: Actor) -> Result<DateTime<Utc>, AppError>;
pub async fn cancel_deletion(tx: &mut PgConnection, ws: WorkspaceId, by: Actor) -> Result<(), AppError>;
```

`create` replaces the three inserts (`bootstrap.rs:337`, `seed.rs:42`, `auth/workos.rs:508`) and
is used by first run, sign-up, import and an "add workspace" action allowed by the server setting
`workspaces.creation` (`operators` by default; `anyone` on hosted, capped per account by plan).
Suspended: sign-in shows a notice, MCP and API refuse with `workspace_suspended`, owners can still
export unless the reason is abuse. Deleting: owners get 14 days to cancel and an export link;
then the `workspace.delete` job removes rows module by module in reverse export order under
`Tenancy::Workspace`, deletes file blobs and exports, cancels billing, and finally deletes the
`organizations` row. Accounts stay (they are global, R19); memberships here end. Agent and
app accounts **owned** by the workspace are retired: their credentials are revoked and their
memberships in other workspaces end, which those workspaces' admins are told when deletion is
requested (R39); during the 14 days a person who manages such an account may take ownership of it to keep
it. DMs in the direct scope are untouched (they belong to their participants). A small
`deleted_workspaces(id, deleted_at)` record remains for billing and staff.

### 3.7 Getting in

- **Server-wide sign-up setting** `signup.mode`: `off` (self-hosted default), `invite`
  (invitation links only), `open` (hosted). Applies to every sign-in method.
- **Open sign-up** (H1.28): email → a six-digit code or link by mail (reusing the verification
  tokens of `password_reset.rs`) → set a password or continue with a provider → create workspace
  (name, address checked by K4's slug rules, and what it is for: one of M1's shapes, M1.17,
  including chat only, R22) → onboarding. A person can also sign up
  through an invitation and never create a workspace.
- **Sign-in with common providers** (H1.30): Google, GitHub and Microsoft as OIDC or OAuth
  implementations of K1's `IdentityProvider` seam, account linking only by verified email.
  **Plain OIDC** for self-hosters (H1.18): one configured issuer (Keycloak, Authentik, a company
  Google), with optional automatic account creation for a verified domain.
- **Company single sign-on and directory sync** (H1.31, H1.32) are per-workspace plan features:
  a workspace verifies a domain (DNS TXT), configures OIDC or SAML (through WorkOS where
  configured, which stays one implementation of the seam, or plain OIDC), and may require it for
  that domain. Directory sync is SCIM 2.0 at `/scim/v2/{workspace}` with a workspace token,
  creating, updating and disabling memberships; it never deletes history.
- **Abuse controls** (H1.29): per-address and per-email-domain sign-up limits (on K5's shared
  limiter); a disposable-domain list shipped as data and updatable; a proof-of-work challenge
  that needs no outside service (a third-party challenge can be a hosted option later); new
  hosted workspaces start in `review` standing for 7 days or until staff clear them (invites and
  public forms limited to 5 a day, no outbound webhooks), and flagged ones go to the staff
  review queue. Public forms and bug report links get per-workspace caps.
- **Accounts owned outside may join (R19).** K1's workspace setting `access.outside_accounts`
  (`yes`, `ask_admin`, `no`; K1 3.11) is set to `ask_admin` for every workspace created on the
  hosted service (`lifecycle::create` under the `hosted` profile) and `yes` on self-hosted;
  admins may change it.
- **People are server-global** and belong to several workspaces (K1). The workspace switcher and
  per-workspace roles are K1's and W1's; H1 only needs `accounts::create_person`, the
  memberships, and a person's list of workspaces for the tenancy scope.

### 3.8 Plans and metering

**Plans are data.**

```sql
CREATE TABLE plans (
    id TEXT PRIMARY KEY CHECK (id ~ '^[a-z][a-z0-9_]{0,40}$'),   -- 'self_hosted', 'free', 'team', 'business'
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    version INTEGER NOT NULL DEFAULT 1,
    limits JSONB NOT NULL CHECK (jsonb_typeof(limits) = 'object' AND octet_length(limits::text) <= 8192),
    operations JSONB NOT NULL DEFAULT '{}'::jsonb,   -- sso, scim, audit_retention_days, support
    public BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- seeded: ('self_hosted', 'Self-hosted', 1, '{}', '{"sso":true,"scim":true}', false)

CREATE TABLE workspace_plans (
    workspace_id UUID PRIMARY KEY REFERENCES organizations(id) ON DELETE CASCADE,
    plan_id TEXT NOT NULL REFERENCES plans(id),
    status TEXT NOT NULL CHECK (status IN ('active','trialing','past_due','grace','over_limit')),
    overrides JSONB NOT NULL DEFAULT '{}'::jsonb,     -- staff grants, such as the non-profit programme
    trial_ends_at TIMESTAMPTZ, grace_ends_at TIMESTAMPTZ,
    source TEXT NOT NULL CHECK (source IN ('default','billing','staff','import')),
    version BIGINT NOT NULL DEFAULT 1,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE usage_events (                          -- append-only ledger for counted meters
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    meter TEXT NOT NULL CHECK (meter ~ '^[a-z][a-z0-9_.]{0,63}$'),
    quantity BIGINT NOT NULL CHECK (quantity >= 0),
    account_id UUID REFERENCES actors(id),
    space_id UUID, conversation_id UUID,
    source_kind TEXT NOT NULL CHECK (char_length(source_kind) <= 64),
    source_id UUID,
    dedupe_key TEXT NOT NULL CHECK (char_length(dedupe_key) <= 200),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (workspace_id, dedupe_key)
);
CREATE INDEX usage_events_meter_month ON usage_events (workspace_id, meter, created_at);

CREATE TABLE usage_counters (                        -- per period, per scope; updated with the ledger
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    meter TEXT NOT NULL,
    period_start DATE NOT NULL,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace','account','space','conversation')),
    scope_id UUID NOT NULL,                          -- the workspace id for scope 'workspace'
    quantity BIGINT NOT NULL DEFAULT 0,
    PRIMARY KEY (workspace_id, meter, period_start, scope_kind, scope_id)
);

CREATE TABLE spend_limits (                          -- also A2's budgets (ruling R13)
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace','account','space','conversation')),
    scope_id UUID NOT NULL,
    -- true: the limit applies to each account inside the scope on its own ("30 messages a
    -- minute per account in this conversation"); only with period minute or hour.
    per_account BOOLEAN NOT NULL DEFAULT false,
    meter TEXT NOT NULL,
    -- minute and hour are counted in K5's shared limiter store (short, lossy);
    -- day and month in usage_counters (durable). admit hides the difference.
    period TEXT NOT NULL CHECK (period IN ('minute','hour','day','month')),
    soft_limit BIGINT CHECK (soft_limit IS NULL OR soft_limit >= 0),
    hard_limit BIGINT CHECK (hard_limit IS NULL OR hard_limit >= 0),
    set_by_actor_id UUID NOT NULL REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1,
    UNIQUE (workspace_id, scope_kind, scope_id, per_account, meter, period),
    CHECK (NOT per_account OR period IN ('minute','hour'))
);

CREATE TABLE limit_notices (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    meter TEXT NOT NULL, scope_kind TEXT NOT NULL, scope_id UUID NOT NULL,
    period_start DATE NOT NULL,
    level TEXT NOT NULL CHECK (level IN ('soft','hard')),
    notified_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (workspace_id, meter, scope_kind, scope_id, period_start, level)
);
```

**Meters.** Two kinds:

| Meter | Kind | What counts | Source |
| --- | --- | --- | --- |
| `people` | gauge | Enabled person memberships (agents and apps never) | memberships, live |
| `guests` | gauge | Guest memberships (C2's guests) | memberships, live |
| `storage_bytes` | gauge | Files plus link preview images, as `files::require_room` counts | files, live |
| `hosted_model_micros` | counter | Cost of model calls made with the **host's** key | `ai_usage` rows with `key_source = 'host'` |
| `agent_service_micros` | counter | Usage reported by a host staff marked as the hosted agent service | A1's run usage reports |
| `automation_heavy_runs` | counter | Automation actions that call a model or the hosted agent service | automations |
| `emails` | counter | Notification and digest mail (account mail never counts) | mail |
| `form_submissions` | counter | Public form and bug report submissions | forms, bug reports |
| `workspaces_owned` | gauge (per account) | Workspaces a person owns, checked at creation | lifecycle |
| A2's budget meters: `operations`, `writes`, `messages`, `deliveries`, `reported_cost_micros`, `reported_tokens` | counter | Operations and writes (K2's runner), messages (send pipeline), deliveries (A2's delivery), usage that any account or host reports for runs (A1's `run_usage_report`, recorded by A2.14) | A2, K2, A1; never billed, only limited by `spend_limits` rows a workspace sets |
| `rows` | gauge | Rows the workspace holds across tenant tables (estimated nightly from H1.3's list, exact on demand) | cost counter (R24 rule 8) |
| `pushes` | counter | Phone and browser push notifications sent | C2's push sender |

**Every cost has a counter (R24 rule 8).** Before the hosted service opens, every cost we pay
has a per-workspace counter: stored bytes (`storage_bytes`), rows (`rows`), emails (`emails`),
pushes (`pushes`), deliveries (`deliveries`) and model calls we pay for (`hosted_model_micros`,
`agent_service_micros`). The `hosted` profile refuses to start if any of these meters is not
registered (H1.37), so a new cost can't ship uncounted. Email and push are the metered
outbound costs; digests and plan limits are how they are held down.

**The direct scope (R20).** No plan, seat count, limit or export of a workspace touches the
direct scope. Its storage counts against the **uploader's own allowance** (R40): a per-account
`storage_bytes` gauge whose limit is one server-wide value the operator sets
(`files.personal_allowance_bytes`: unlimited by default when self-hosted, the free plan's file
allowance when hosted). It never counts against a workspace and is not derived from the
uploader's workspaces' plans. Messages and deliveries in DMs count against the account
that acts, under `scope_kind = 'account'`.

**One enforcement function, one place.**

```rust
pub struct Ask<'a> {
    pub workspace_id: Uuid,
    pub meter: Meter,
    pub quantity: i64,
    pub account_id: Option<Uuid>,
    pub space_id: Option<Uuid>,
    pub conversation_id: Option<Uuid>,
    pub dedupe_key: Option<&'a str>,   // Some: record usage in the same call
}
pub enum Admission { Allowed, AllowedNearLimit { notice: LimitNotice } }
pub struct LimitRefusal { pub meter: Meter, pub scope: Scope, pub limit: i64, pub used: i64,
                          pub resets_at: Option<DateTime<Utc>>, pub message: String, pub who_can_change: Who }

pub async fn admit(tx: &mut PgConnection, ask: Ask<'_>) -> Result<Admission, LimitRefusal>;
pub async fn record(tx: &mut PgConnection, usage: Usage<'_>) -> Result<(), AppError>; // idempotent by dedupe_key
pub async fn usage_for(conn: &mut PgConnection, ws: Uuid, period: Period) -> Result<UsageReport, AppError>;
```

`admit` reads the effective limit for each scope that applies (the plan's limit with staff
overrides, the workspace's own setting where one exists such as `files.workspace_quota_mb` or
`ai.monthly_budget_usd`, and any `spend_limits` row for the account, space or conversation), takes
the smallest, and compares it with the gauge or the counter row, locking the counter row
(`SELECT ... FOR UPDATE`) only for hard counted limits so concurrent calls can't both pass the
last unit. Soft-then-hard: at 80% it allows and returns a notice (one inbox notice per scope per
period, through `limit_notices`); at 100% of a soft limit it allows during the plan's grace and
says so; at the hard limit it refuses with `LimitRefusal`, which K2's error list turns into one
`limit_reached` error carrying the meter, the numbers, when it resets and who can raise it.

**Where it's called.** Accepting an invitation or adding a member (`people`, `guests`);
uploads and imports (`storage_bytes`, replacing the quota half of `files::require_room`); the AI
gateway (`hosted_model_micros` and the workspace budget, replacing `ai/gateway.rs:498-504`);
starting a run on the hosted agent service (`agent_service_micros`, A1); automation actions that
call a model or a hosted host; notification mail; public forms; workspace creation. K2's
operation runner calls it once per write (`writes`, `operations`; ruling R13), the send pipeline
for `messages` and A2's delivery for `deliveries`; A2's budgets are `spend_limits` rows, not a
second mechanism. For `minute` and `hour` periods `admit` calls K5's `Limits::check_cost`
for the rate bucket (`LimitKey::Account`/`Workspace` with `LimitSource` reading `spend_limits`); for `day` and
`month`, in `usage_counters`. A refusal for `deliveries` holds the delivery (the caller waits)
instead of dropping it. Nothing else ever checks a plan or a budget.

**Self-hosted.** The `self_hosted` plan has no limits, so only the workspace's own settings and
spend limits apply. Usage is still recorded and shown to admins (Settings › Usage). Plans,
billing and staff screens don't appear unless `AGENT_WORKSPACE_PROFILE=hosted`.

**Never charged, never cut.** Agents are accounts but never `people`. No meter counts messages,
tasks, pages or history; no retention job deletes content (guard in 6.2). Over a limit, nothing
is deleted or hidden: new invitations or uploads are refused, everything else works, export
always works.

**The free plan, proposed numbers.**

| Limit | Free | Why |
| --- | --- | --- |
| Modules, projects, messages, tasks, pages, history | Unlimited, never cut | Costs little per workspace (text rows); history limits are the most resented cut in the category (R3 section 5) |
| People | 10 | The category's small-team line; the product's reason to exist is agents, which are free |
| Guests | 10, not counted as people | Lets a small team invite clients or players without paying |
| Agents | Unlimited; fair use of 600 calls/min per credential and 3,000/min per workspace | Agents are accounts, never seats (R3 recommendation 2) |
| Files | 2 GB, 25 MB a file | Bounded and cheap in object storage; a per-file cap keeps upload abuse down |
| Hosted model calls | None included; bring a key at no charge | The cost that grows without limit; never meter what the host doesn't pay for |
| Heavy automation runs | 1,000 a month | Only actions that cost compute count; a rule that posts a message is free |
| Notification email | 3,000 a month | Account mail (sign-in, reset, invitations) never counts |
| Public form submissions | 1,000 a month | Abuse control more than cost |
| Free workspaces a person may own | 3 | Abuse control |
| Export | Any time (one a day) | The promise |

**Paid plans add operations, never features**: more people (priced per person, in the category's
$6 to $10 band, R3 section 5), more storage, included model credits with a default model that
works without setup, the hosted agent service at usage prices, company single sign-on and
directory sync, longer audit retention, support with response times. Open-source, education and
non-profit groups get the first paid plan free on application (a staff override with a reason
and an end date).

### 3.9 Billing

A `billing` module, active only when `AGENT_WORKSPACE_BILLING_PROVIDER` is set (hosted profile).
The merchant of record handles tax, invoices and card data; Workspace never sees a card. The
provider is picked when H1.33 starts (the earlier plan named one; a short comparison of
merchant-of-record services is part of that slice). Provider-specific JSON stays in its module
(`billing/<provider>.rs`).

```rust
pub trait MerchantOfRecord: Send + Sync {
    async fn checkout(&self, ws: &BillingWorkspace, plan: &PlanPrice, seats: u32) -> Result<Url, BillingError>;
    async fn portal(&self, customer_ref: &str) -> Result<Url, BillingError>;
    async fn set_seats(&self, subscription_ref: &str, seats: u32) -> Result<(), BillingError>;
    fn verify_webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<BillingEvent, BillingError>;
}
pub enum BillingEvent {
    SubscriptionStarted { ws: Uuid, plan: String, subscription_ref: String, customer_ref: String, period_end: DateTime<Utc> },
    SubscriptionChanged { subscription_ref: String, plan: String, seats: u32, period_end: DateTime<Utc> },
    PaymentFailed { subscription_ref: String, retry_at: Option<DateTime<Utc>> },
    PaymentSucceeded { subscription_ref: String, invoice_url: Option<String> },
    SubscriptionEnded { subscription_ref: String, at: DateTime<Utc> },
}
```

- **In**: `POST /api/v1/billing/webhook` (excluded from session auth, signature checked), stored
  in `billing_events (provider_event_id UNIQUE, workspace_id, kind, received_at, applied_at)`
  for idempotency, then applied by one function `plans::apply_billing_state`, which is the only
  writer of `workspace_plans` besides staff overrides. Out-of-order events are resolved by
  fetching the subscription's current state from the provider.
- **Out**: seat count changes (debounced job, at most hourly) and plan changes.
- **Trials**: 14 days of the first paid plan, no card, then back to free.
- **Upgrades** take effect at once; **downgrades** at the period end. A workspace over the new
  plan's limits gets `over_limit`: nothing deleted, new invitations and uploads refused until it
  is under.
- **Failed payments**: `past_due` while the provider retries; then 14 days of `grace`; then the
  free plan, never suspension and never deletion.
- **Invoices** are the provider's; the billing page links to its portal.
- **Self-hosted** never reaches this code: no provider configured, no routes mounted, no screens
  (checked in 6.2).

### 3.10 The staff console

A separate program, `apps/staff` (`agent-workspace-staff`), in the same repository and Cargo
workspace, sharing the server crate as a library. It listens only on a private address, has its
own staff accounts (signed in by the operator company's single sign-on plus a security key), and
uses a database role with read access and a small set of `SECURITY DEFINER` functions for its
writes. Server-rendered pages, no SPA.

- **Find a workspace** by id, address, owner email or billing reference; see plan, usage, member
  and agent counts, lifecycle, flags, recent staff actions. Never message, page or file content.
- **Plan overrides** (with a reason and an end date), **suspend and resume** (abuse, legal),
  **the review queue** of new workspaces and reported content (a report carries the reporter's
  quote, not a view into the workspace), **metrics** (workspaces, active, sign-ups, plan mix,
  storage, meters, abuse flags).
- Every action writes `staff_actions` and also an event into the workspace's own audit trail
  ("Staff suspended this workspace: reason"), so workspace owners can see what staff did.
- **Impersonation: none (recommended).** When a customer wants help with their content, a
  workspace admin invites a support account as a guest with a role and an end date, through the
  ordinary invitation and roles, and every action is in the ordinary audit trail. This uses the
  product's own access model and gives staff no hidden power. The alternative (time-boxed,
  consented, read-only impersonation) is listed as an owner question.

### 3.11 Operations (hosted)

- **Profile `hosted`** forces: private addresses off, built-in host off, sign-up open, metering
  enforced, object storage required, server mail required, update check off, staff routes off in
  the app.
- **Mail**: a managed provider through the server transport (H1.17), with bounces and complaints
  fed back to suppress addresses.
- **Object storage**: K5's `FileStore` variant; bucket versioning on; exports in the same store.
- **Monitoring and alerting**: the existing Prometheus metrics and `AlertSink`
  (`metrics.rs:13-16`) to a pager; per-workspace gauges for the noisy-neighbour caps.
- **Error tracking without personal content**: errors are reported with category, route
  template, request id and workspace id only (the shape `AppError` already logs); browser error
  reports already arrive at `/api/v1/client-errors` (`metrics.rs:205-208`) and are scrubbed the
  same way.
- **Status page** on separate infrastructure, components: app, agent API, realtime, mail, agent
  service.
- **Backups**: managed PostgreSQL with point-in-time recovery, plus the built-in daily set; a
  monthly automated restore drill into a scratch database.
- **Deploys with zero downtime**: K5 (two processes, startup lease, expand-contract migrations).
- **Regions and cells later.** What must be true now so cells are possible: every row carries its
  workspace (3.3); no request path joins across workspaces except global identity (row security
  enforces it); ids are UUIDv7 made by the server (true today); global tables are small and
  identity-only (6.2's list), so a later directory can route a person to the cells of their
  workspaces; the workspace address is in every URL (K4), which is what a router keys on; and
  export and import move a workspace between cells.

### 3.12 The hosted agent service (seam only)

It is another host that enrols with a workspace through A1's protocol (A1.11, A1.12), signs in
as an app account like every host (ruling R7) and gets no other rights. Because it enrols
separately with each workspace that subscribes (or each person who links it for their own
agents, R19), each enrolment makes its own `hosts` row owned by that workspace or person; the operator
configures the service's enrolment signing key (`AGENT_WORKSPACE_BILLED_HOST_KEYS`), and an
enrol call carrying a valid signature from it is marked in a server-level table
`host_billing (host_id PRIMARY KEY, billing_class TEXT CHECK (billing_class IN
('hosted_service')), marked_by, marked_at)` (staff can also mark or unmark by hand in the
console). Before A1's launch path dispatches a `start` command to such a host (A1.13), it calls
`metering::admit(meter: agent_service_micros, account, conversation)` in the run's workspace
(for a DM run in the direct scope, against the launching account's allowance, R20); usage the host reports
with `run_usage_report` is recorded with `dedupe_key = usage_report:<usage_reports.id>` (the
same key A2.14 uses for `reported_cost_micros`, so a report is never counted twice). Usage
self-reported by any other host is shown and counts toward A2's budgets but is never billed.
Nothing in the protocol or the run record differs for this host.

### 3.13 The public release

**Licence (R25).** Deferred: the licence is **chosen at release**. Until then every plan
assumes one fully open project with no closed edition, and no plan may depend on a paid-only
feature (so the "Open core" row below is out). The options are kept for that choice.

**Licence options.**

| Option | For | Against |
| --- | --- | --- |
| AGPL-3.0 for the app, MIT OR Apache-2.0 for SDKs, protocols, package format (recommended) | Anyone can self-host and change it; a competing hosted copy must publish its changes; the pieces others build on stay easy to adopt | Some companies avoid AGPL dependencies (they use the SDKs, which are permissive) |
| Apache-2.0 everywhere | Widest adoption | Anyone can host it against us with no obligation |
| Open core | Revenue from closed features | Contradicts "same product hosted and self-hosted" (R3 section 4) |
| Source-available (BSL, FSL, ELv2) | Most protection | Not open source; the category has seen trust lost by switching (R3 section 4) |

Before choosing, these must be true (H1.39): every dependency's licence is compatible (cargo-deny
covers Rust, `deny.toml:14-36`; nothing checks the web's npm packages today, so add a check;
fonts, icons and images are listed); `THIRD-PARTY-NOTICES` is generated for the image and the
web's About page (`NOTICE` already credits IJG); machine-specific details are out of the tree
(`tools/ci/run.sh:67,93`, `deploy/redhat/*` user and paths, `link_previews/guard.rs:16`'s user
agent, the dev seed that points at the owner's repository); a history scan for secrets and
personal or infrastructure details, with the owner choosing to publish as is or rewrite (6.3);
contributor rules (DCO sign-off recommended, so no CLA, with the consequence that relicensing
later needs every contributor); `SECURITY.md` with private reporting and supported versions;
`CONTRIBUTING.md`; a README for strangers; documentation; a neutral demo workspace (seeded by
`dev-seed`) shown on a public demo server that resets nightly.

**Trust documents** (H1.40, hosted): terms of service, privacy policy, data processing terms and
the sub-processor list, an acceptable use policy, the **free-plan promise** (the limits won't
shrink without 12 months' notice; a free workspace is never deleted for inactivity; export and
self-hosting always work), and the **export promise** (any workspace exports in the documented
format at any time, including after a downgrade or a failed payment).

### 3.14 Cheap to run (R24, R23, R30)

**The web app is static files (R24 rule 1).** `apps/web` builds to `dist/` with no origin
baked in; it can be served by the server (self-host default, as today), by any static host or
by a CDN. At start it fetches **`GET /client-config`** (H1's route, R30; public, cacheable for
60 s): `{apiOrigin, liveOrigin, signIn: {methods, oidc?}, signup: mode, features, version}`,
built from the server's configuration and never holding a secret. The server sets CORS for
the configured web origins only. No page is rendered by the server. The one exception that is
not part of the app shell, `/panel-frame/{launch}` (M1's frame host page), is served by the
`api` role because it needs a response header per launch (R31).

**Nothing runs per workspace (R24 rule 6).** No timer, loop, connection, cache warm-up or
in-memory state exists because a workspace exists: H1's jobs (backfill, metering rollups,
deletion, digests) query due rows. Guard (H1.42), which covers K5's loops too (lease renewal,
the wake, fan-out, the scheduler, the live role) as well as H1's: a test starts the server with 10,000 empty
workspaces (created in one SQL batch) and asserts that over five idle minutes CPU time and
resident memory do not grow beyond the same server with one workspace, by a margin set in the
test, and that no query runs per workspace per tick (counted with `pg_stat_statements`).

**Shapes and what they cost idle.** `docs/hosting/shapes.md` (H1.42) names K5's pieces each
shape relies on: the three roles `api`, `live`, `work` (K5.7), the tick endpoint that lets an
outside timer drive `work` (K5.18), the pooler rules (K5.17 and 3.4: `SET LOCAL` only, no
session advisory locks, no hand-written `PREPARE`, short transactions, `LISTEN`/`NOTIFY` only
as an optimisation), and the live contract with its conformance suite (K5.19; in the edge
shape one `work` instance holding a short `live.publisher` lease tails the log and posts to
the live service, R35). It describes three reference shapes and what each costs while idle: **one box** (self-host: one process in all three K5
roles, PostgreSQL, disk); **small hosted** (two small machines, managed PostgreSQL, object
storage, a CDN for the static app); **edge** (static app on a CDN, files in zero-egress object
storage with signed links, K5's live role on edge objects that hibernate idle sockets, `api`
in containers that stop when idle and answer within a second of starting, PostgreSQL through a
transaction pooler, `work` driven by an outside timer calling K5's tick endpoint). The edge
shape is designed and its contracts tested (pooler-safe scope, 3.4; K5's live-role suite); it
is built only when the numbers say so. No provider is named in the code; the document may
name the one whose current documentation it was checked against.

**A workspace's own bucket (R23).** On the hosted service a workspace may bring its own
S3-compatible bucket for its uploads. The mechanism is K5.20's (`workspace_stores`,
`workspace_store_set`, the per-workspace move); H1's part (H1.43) is the Settings › Storage
screen, the plan rule that bytes in the workspace's own bucket do not count against
`storage_bytes`, and listing the bucket under `needs/connections.jsonl` in an export. Exports read from either store. File *sources*
(outside stores whose files appear by reference) are C1's and M1's, not this.

---

## 4. Migration path

The live server runs a 2026-10-31 commit with every later migration pending, and many branches
merge in any order. Each step below is additive until its last slice.

1. **Limits** (H1.1): new keys and scopes beside the old buckets; old global buckets become
   failure-only in the same slice. No data.
2. **Outbound** (H1.2): `replies::webhook_client` and `link_previews::guard` become thin wrappers
   over `outbound`, then their callers move and the wrappers are deleted in the same slice.
   `public_ipv4/6` move from `replies.rs:1682-1710` to `outbound.rs`.
3. **Workspace column** (H1.3 to H1.7): expand, copy by a resumable job, switch (3.3). Each batch
   is independent and touches only its tables' migration and trigger, so batches merge in any
   order. Writers are unchanged. Coordinate with T1 (tasks as records adds tables: they are born
   with the column), K3 (new event and link tables are born with it) and K4 (the rename comes
   after all batches).
4. **Export declarations** (H1.8) are new JSON fields; the coverage guard starts with an
   allow-list of undeclared tables that only shrinks.
5. **Export and import** (H1.9, H1.10) are new operations and tables; the old task export stays
   as it is (it is a different, smaller thing).
6. **Self-host** (H1.12 to H1.19): config changes keep every existing variable meaning the same;
   the Red Hat env file keeps working unchanged. `bootstrap-admin` keeps working. The Red Hat
   backup scripts keep working.
7. **Tenancy scope and row security** (H1.20 to H1.22): observe mode first (nothing refused),
   then policies per batch while unset scopes are still allowed, then strict mode. Rollback for a
   batch is `ALTER TABLE ... DISABLE ROW LEVEL SECURITY` in a new migration.
8. **Lifecycle** (H1.24): new columns with defaults; the three workspace inserts move to
   `create` in the same slice.
9. **Metering** (H1.25 to H1.27): `files.workspace_quota_mb` and `ai.monthly_budget_usd` keep
   their meaning as the workspace's own limits; `require_room`'s quota check and the gateway's
   budget check move into `admit` in H1.26, with their tests kept. The `self_hosted` plan is
   seeded and every existing workspace gets a `workspace_plans` row by migration
   (`INSERT ... SELECT id ... ON CONFLICT DO NOTHING`), plus `lifecycle::create` writes one.
10. **Sign-up, billing, console** add new routes behind settings that default off.

---

## 5. Slices

Sizes: S about a day, M a few days, L about a week. **[owner now]** marks slices worth building
for the owner's own server now. Phase order: A (cheap tenancy groundwork), B (export), C
(self-host), D (row security and lifecycle), E (plans and getting in), F (billing, console,
operations), G (release).

### Phase A: groundwork that makes hosting cheap

**H1.1 Limit scopes, no global bucket for good callers** [owner now] — M
Declare every limiter with a scope (`rate_limit.rs:76-107`); charge server-wide ceilings only for
failed sign-ins, failed MCP authentication and junk refreshes; add per-workspace ceilings for MCP
and public API calls, invitations, bug reports and public forms; make the client address header
configurable (`rate_limit.rs:420-425`); make the OAuth client cap a setting and add a per-address
registration limit (`mcp/oauth.rs:188-205`). If P0 or K5 has already fixed the login bucket,
this slice shrinks to the rest.
Changes: `rate_limit.rs`, `auth/mod.rs:186-195`, `mcp.rs:4336`, `public_api.rs:79`,
`mcp/oauth.rs`, `config.rs`, `docs/operations/abuse-controls.md`.
Depends on: nothing (K5's shared store when present).
Acceptance: 100 junk sign-ins from one address do not stop a correct sign-in from another
(`tests/integration/roles_and_access.rs` or a new `hosting_limits.rs`); one workspace's agents
exhausting their ceiling don't refuse another workspace's agent; unit test that every limiter has
a declared scope; the "no new global limit" guard (6.2).

**H1.2 One outbound guard** [owner now] — S
Move `public_ipv4/6` into `outbound.rs`; webhooks, agent wakeups and link previews use
`outbound::resolve` and one pinned-client builder that pins every checked address; add a
per-workspace outbound permit. A guard test fails on `reqwest::Client::builder` outside
`outbound.rs` and an allow-list of fixed-address clients (GitHub, WorkOS, push, provider cloud
harnesses).
Changes: `outbound.rs`, `replies.rs:1600-1710`, `webhooks.rs:1311`, `link_previews/guard.rs`.
Depends on: nothing. Can run beside H1.1.
Acceptance: unit tests for a name resolving to one public and one private address being refused
for webhooks and previews (today only the first address is checked); the builder guard.

**H1.3 Tenant table registry and guard** — S
`apps/server/src/tenancy/tables.rs`: the list of global tables with a reason each (R19:
accounts of every kind, `actors` today, with their owner columns; every credential table,
K1's `account_credentials` and today's `user_*`, token and OAuth tables; A1's `hosts`,
`host_agents`, `host_enrolments`; A2's `agent_connections` and `connection_signing_secrets`;
K1.7's `agent_outside_refs`; sessions and sign-in tables), the tenant column of every
workspace table (`organization_id` where it exists today,
`workspace_id` where H1.4 to H1.7 add it or a new table is born with it; ruling R1), and the
allow-list of tables still missing the workspace column (shrinks with H1.4 to H1.7). Row
rules, export, the isolation guards and the round-trip test read the column name from here. Integration
test `every_table_belongs_to_a_workspace_or_says_why` reads `information_schema.columns`.
It also lists the tables that may hold direct-scope rows (R20) and checks that no membership
or role row references the direct scope.
Depends on: nothing. Can run beside H1.1, H1.2.
Acceptance: the test passes today with the allow-list, and fails for a new table with neither;
an account table, a credential table or a host table outside the global list fails it.

**H1.4 Workspace column: chat** — M
`conversations`, `conversation_members`, `conversation_read_states`,
`conversation_notification_levels`, `conversation_entries`, `conversation_pins`,
`message_mentions`, `message_reactions`, `message_revisions`, `message_link_previews`,
`message_work_item_references`, `notifications`, `notification_occurrences`,
`notification_project_mutes`, `reply_requests`, `person_questions`, `push_deliveries`,
`outbox_events`, `thing_references`, `project_decisions`. Expand, trigger, backfill job, switch
(3.3), remove them from H1.3's allow-list.
Depends on: H1.3. Can run beside H1.5 to H1.7.
Acceptance: `tests/integration/tenancy_backfill.rs`: a row inserted without the column gets it
from its project; the backfill job resumes from its cursor after being stopped; the switch
migration completes when the job hasn't run.

**H1.5 Workspace column: tasks and planning** — M
`work_items` and its 30 satellite tables, `project_labels`, `project_milestones`,
`project_counters`, `recurring_task*`, `task_*`, `saved_views`, `saved_view_pins`,
`field_definitions`, `project_task_settings`, `project_navigation_states`. Same shape as H1.4.
Coordinate with T1 (tasks as records).
Depends on: H1.3.

**H1.6 Workspace column: documents and records** — M
Pages (`project_pages`, `project_page_documents`, `project_page_comments`,
`project_page_suggestions`, `project_page_revisions`, `project_instructions`), sheets, whiteboards
(`whiteboard_elements`, `whiteboard_files`, `whiteboard_links`, `whiteboard_revisions`),
collections and record history. Same shape.
Depends on: H1.3.

**H1.7 Workspace column: agents, runs, code, MCP** — M
`external_agent_*`, `execution_*`, `agent_run_*`, `agent_approvals`,
`agent_webhook_deliveries`, `agent_setup_links`, `automation_runs`, `automation_rule_matches`,
`github_*`, `repositories`, `git_worktrees`, `mcp_client_credentials`, `mcp_oauth_grants`,
`mcp_oauth_authorization_codes`, `mcp_oauth_access_tokens`, `mcp_oauth_refresh_tokens`,
`provider_connections`, `capacity_alert_fires`, `idempotency_keys`, `ai_plan_import_items`,
`project_memberships`. Same shape. Coordinate with A1 (run tables) and K1 (accounts): A1's
new tables (`runs`, `run_activities`, `run_usage`, `usage_reports`, `requests`, `hosts`,
`host_enrolments`, `host_agents`, `host_commands`) and A2's (`agent_connections`,
`event_subscriptions`, `deliveries`, `follows`, ...) are born with `workspace_id NOT NULL` and
are not in this batch. A1.22 turns `agent_approvals`, `person_questions` (H1.4) and
`agent_run_usage` into views over `requests` and `run_usage`; whichever lands second keeps a
`workspace_id` column on the view (from `requests.workspace_id`, `run_usage.workspace_id`), and
H1.3's list marks them as views so the row-security guard skips them (the base tables carry
the policy). `external_agent_connections` is copied by A2.1 into `agent_connections` with the
same ids; it still gets the column here because legacy credentials point at it until K1.31.
Depends on: H1.3.
Acceptance (for H1.4 to H1.7 together): H1.3's allow-list is empty.

**H1.8 Export declarations** — M
An `ExportPart` with its `ExportDecl` for every core module, registered through M1's
`Registrar::export`; the generic table exporter/importer (no archive yet). Guard
`every_workspace_table_is_exported_or_says_why` (6.2) reads the registrations.
Depends on: H1.3; M1.3 (soft, R41: until M1.3 lands, H1.8 keeps an in-code list of
`ExportDecl`s in `export/registry.rs`, which M1.3 then moves into `Registrar::export`
registrations; no JSON section in manifests either way); easier after H1.4 to H1.7.
Acceptance: the guard passes with every module declaring; a table added to a module with no
declaration fails it.

### Phase B: export and import

**H1.9 Workspace export** [owner now] — L
`workspace_transfers`, the `workspace.export` job, the archive writer (format v1, 3.2), file
blobs, `needs/*`, notification with download link, operation and agent tool `workspace_export`,
route, Settings › Workspace › Export page (lazy, registered in the layout registry), permission
`workspace.export` in K1's catalogue. Isolation cases for the new route and tool.
Depends on: H1.8; K1.9 for the permission (until then, the owner and admin check through
`authorize` with a new `Action`); K2.3 for the operation form (until then an ordinary route plus a
tool, as `agent_parity` requires).
Acceptance: `tests/integration/workspace_export.rs`: the archive of the isolation fixture's
workspace A holds every declared kind and none of B's ids or markers, and none of the
direct-scope DMs between A's members (R20), while a `kept` workspace's DMs are in it; account
profile stubs never name another workspace (R19); `direct_export` gives a person their own
DMs and nobody else's; no line contains a secret
value, password hash, token or session id (scan for the fixture's known secrets); the download
needs the requester's session; isolation nets cover the route and tool.

**H1.10 Workspace import** — L
The import job (resumable, batches with cursor), id modes and `workspace_import_ids`, the account
mapping step (people by verified email, agents and apps by outside reference, R19), unclaimed accounts and claim links (with K1), agents and apps marked "needs
connecting", archive safety checks, `workspace-import` CLI command, web upload in Settings and in
first run (after H1.14).
Depends on: H1.9, H1.24 (lifecycle `importing`); K1.29 (`accounts::create_person`, unclaimed
accounts). Agents arrive "needs connecting" and are reconnected through A2.18's reconnect path
(soft: until then their managers create a new credential on the agent page).
Acceptance: `tests/integration/workspace_import.rs`: an import killed mid-way and resumed gives
the same result as one uninterrupted; importing onto the source server switches to copy mode and
every reference (including ids in message text) points into the copy; an archive naming another
workspace's id is refused and changes nothing (digest as in `write_isolation`); a zip with `../`
or a size bomb is refused; no outbox event, notification or webhook fires during import; an
agent whose outside reference exists on the target is matched, and joins only after a manager
of it agrees.

**H1.11 Lossless round-trip guard** — M
The guard test of 6.1: export workspace A of the full fixture, import it into a fresh schema,
compare canonical digests table by table through the id map.
Depends on: H1.10. Acceptance: the test itself; it fails when a column is added to an exported
table without export coverage.

### Phase C: self-host in one command

**H1.12 Container-ready configuration** [owner now] — M
`*_FILE` variables, `ALLOWED_ORIGINS` from `PUBLIC_ORIGIN`, loopback `http` public origin in
production, keyring created on first start, profiles (`small`, `standard`, `hosted`),
`docs/self-host/configuration.md` and its guard test.
Changes: `config.rs`, `main.rs`, `secrets.rs`, docs.
Depends on: nothing. Can run beside phase A.
Acceptance: unit tests in `config.rs` for each new rule; the documentation guard.

**H1.13 Image and compose** — M
`deploy/container/Containerfile`, `compose.yml`, `init.sh`, `initdb/10-app-role.sql`, the
`health` subcommand, extended `/health/ready`, a CI workflow that builds the image for both
architectures on tags and on demand (not on every pull request), notices file in the image.
Depends on: H1.12. A1.17 for the host image (the `host` profile is added when A1's host program
exists; until then the server image runs the built-in host embedded, K5.3).
Acceptance: on a clean machine, `init.sh && docker compose up -d` gives a ready server (manual
check recorded in the pull request, and the CI workflow's smoke step); `/health/ready` is 503
while migrating.

**H1.14 First run in the browser** — M
Setup code file, `POST /api/v1/setup`, `/setup` page (lazy), `bootstrap-admin` calling the same
functions, the M0 adoption path deleted (A report finding 8).
Depends on: H1.24 (`lifecycle::create`), K1.29 (`accounts::create_person`); M1.17 (soft:
"Team" only until the shapes exist).
Acceptance: `tests/integration/first_run.rs`: choosing "Friends" gives a workspace with only
Chat, Files and Search on; setup without the code, or with a wrong one, is
refused and rate limited; the code works once; it is never logged (capture the log); after the
first admin exists `/setup` refuses even with the code.

**H1.15 Startup under a lease, upgrades and releases** — S
K5's `server.startup` lease around everything before the listeners; readiness reports the step;
refuse to serve when the schema is newer than the build; `CHANGELOG.md` and numbered releases;
the update check with its off switch.
Depends on: K5.2 (leases). Acceptance: two processes started at once against one database: one
migrates, the other waits and then serves; the second never runs recovery for the first's runs
(with K5.3's `builtin-host` lease).

**H1.16 Portable backup and restore** [owner now] — M
`backup create|schedule|key`, `restore`, `restore drill`, status file, docs; the compose
`backup` profile.
Depends on: H1.12; K5 for the object-store note. Can run beside H1.13.
Acceptance: `tests/integration/backup_restore.rs` (skipped without `pg_dump` on the path, but
CI has it): create a set from a seeded database and data root, restore into a new database and
directory, and the restored workspace reads the same messages and files; restore refuses a
non-empty target.

**H1.17 Server mail transport** — S
`AGENT_WORKSPACE_MAIL_URL` (`smtp://`, `smtps://`) or an API provider module, used for account
mail; workspace SMTP stays for notification mail and overrides only that. `mail::transport_for_actor`
(`mail/mod.rs:305-337`) prefers the server transport.
Depends on: nothing. Acceptance: `w19_self_hosting.rs` gains: a reset email goes out through the
server transport when no workspace has SMTP; the server SMTP host goes through `outbound`.
From M1.17 (soft), the email-words guard of 6.2 runs over every mail template.

**H1.18 Plain OpenID Connect sign-in** — M
One configured issuer as an `IdentityProvider` implementation, discovery, PKCE, account linking
by verified email, optional automatic membership for a verified domain.
Depends on: K1.30 (`IdentityProvider` seam, provider-neutral session columns); K5.14 (sign-in
state across processes).
Acceptance: an integration test against a stub issuer on loopback: sign-in creates or links the
account; an unverified email never links; state and nonce are single use.

**H1.19 Self-host documentation and the offline check** — M
`docs/self-host/*` (install, configuration, TLS, backups, upgrades, moving a workspace, small
machines), the README section, and the offline checks of 6.2. The Red Hat guide stays as one
example.
Depends on: H1.13, H1.16. Acceptance: the offline checks run in CI; the docs guard from H1.12.

### Phase D: row security and lifecycle

**H1.20 Tenancy scope** — M
`tenancy.rs`: the task-local, `scoped`, `system` with registered reasons, `db::scoped_tx` and
`db::scoped_one` (the scope set by `SET LOCAL` only, R24 rule 3; no pool hook), the observe log
and metric, the tenancy span on requests and jobs, and wrapping in the auth middleware, MCP
principal and job runner.
Depends on: nothing for the mechanism; K2.3 (soft) for the per-request narrowing from the
`/api/v2/w/{workspace}` path (ruling R4, 3.4); until then requests keep the member scope.
Acceptance: unit tests for serialisation; an integration test that a job sees
`w:<its workspace>`, a request sees its memberships, a v2 request sees only its path's
workspace, an MCP call by an agent in two workspaces sees both and, with the `workspace`
argument, only that one; a test runs the integration suite's scoped paths through a
transaction-mode pooler (PgBouncer in the test compose) and finds no scope carried from one
transaction to the next; a measured cost note in the pull request.

**H1.21 Row security: framework and chat** — M
`tenancy_allows()`, the policy generator migration for H1.4's tables, the test pool's
non-superuser role, fixes for every unscoped path the observe log shows on those tables.
Depends on: H1.4, H1.20.
Acceptance: both isolation nets pass with the test role; a deliberately unscoped query against a
chat table, in a test with strict mode on, returns nothing; the row-security guard (6.2) covers
the batch.

**H1.22 Row security: every other table's policies, observe mode** — M
Policies for H1.5 to H1.7's tables and the 56 that already had the column, in observe mode (an
unset scope is allowed and logged). No call-site fixes here: those are H1.45 to H1.62 (R42).
Depends on: H1.5 to H1.7, H1.21.
Acceptance: the row-security guard allows no tenant table without its policy; with a scope set,
a direct-scope DM's messages are invisible under every `Member` and `Workspace` scope,
including its participants' workspaces' admins, and visible to its participants (R20).

**Scoping the call sites (R42).** About 2,400 pool call sites run outside a scoped
transaction today. They are moved into `db::scoped_tx`, `db::scoped_one` or K2's runner by
mechanical slices, one per server module directory, mergeable in any order behind one ratchet.
Strict row rules switch on only when the ratchet reads zero.

**H1.44 Unscoped call-site ratchet** — S
`tests/tenancy_ratchet.rs`: a source scan counts, per directory, call sites that run a query
on the pool outside `scoped_tx`, `scoped_one` or K2's runner, and compares against
`tenancy/ratchet.json`. A count above the stored one fails; a count below it fails until the
file is lowered in the same pull request, so the number only goes down. The observe log (H1.20)
backs it at run time.
Depends on: H1.20. Acceptance: adding an unscoped query fails the test; scoping one and not
lowering the file fails it too.

**H1.45 to H1.62 Scope call sites, one module directory each** — S each
Mechanical: wrap each listed call site in `scoped_tx` or `scoped_one` (or move it onto K2's
runner where the operation already exists), lower that directory's entry in `ratchet.json`, no
behaviour change. One slice per directory under `apps/server/src/`, and the files directly in
`src/` in four alphabetical groups:
H1.45 `mcp/`; H1.46 `work_items/`; H1.47 `ai/`; H1.48 `auth/`; H1.49 `collections/`;
H1.50 `github/`; H1.51 `mail/`; H1.52 `modules/`; H1.53 `page_docs/`; H1.54 `provider/`;
H1.55 `push/`; H1.56 `sheets/`; H1.57 `usage/`; H1.58 `whiteboards/`; H1.59 `src/*.rs` a to c;
H1.60 `src/*.rs` d to l; H1.61 `src/*.rs` m to r; H1.62 `src/*.rs` s to z. A directory created
later gets its entry in the ratchet file from birth and is scoped from the start.
Depends on: H1.44 each; none on each other. Acceptance per slice: that directory's ratchet
entry is zero; the integration tests touching it pass with strict mode on in a test database.

**H1.63 Strict row rules** — S
`ALTER DATABASE ... SET app.tenancy_strict = 'on'` and the same in the compose init, only when
`ratchet.json` totals zero.
Depends on: H1.22, H1.44 to H1.62. Acceptance: the ratchet total is zero; the observe metric shows
zero unscoped queries over the full integration suite; a deliberately unscoped query returns
nothing.

**H1.23 Noisy-neighbour controls** — M
Per-workspace running-job cap in the claim query, per-workspace permits for outbound deliveries,
exports, imports, search and realtime connections, request `statement_timeout`.
Depends on: H1.1, H1.2; K5.6 for permits across processes.
Acceptance: `tests/integration/hosting_limits.rs`: 50 queued jobs of workspace A don't delay
workspace B's single job past one claim cycle.

**H1.24 Workspace lifecycle** — M
`lifecycle` columns, `workspace_lifecycle::{create, suspend, resume, request_deletion,
cancel_deletion}`, the three existing workspace inserts moved to `create`, suspension enforced in
auth and MCP, the `workspace.delete` job, operations and tools `workspace_delete` and
`workspace_delete_cancel`, isolation cases.
Depends on: H1.8 (reverse order for deletion); K4.5 (address rules).
Acceptance: `tests/integration/workspace_lifecycle.rs`: a deleted workspace leaves no row naming
its id in any tenant table and no blob, and workspace B is untouched (digest); a suspended
workspace's agent gets `workspace_suspended`; deletion can be cancelled within the grace period.

### Phase E: plans, metering, getting in

**H1.25 Metering core** — M
Tables of 3.8, `metering::{admit, record, usage_for}`, the `self_hosted` plan and rows for every
workspace, Settings › Usage (lazy, registered), operation and tool `usage_report`.
Depends on: H1.24 (soft: until `lifecycle::create` writes the plan row, `admit` treats a
workspace with no `workspace_plans` row as on `self_hosted`, so this slice and A2.14 need not wait
for phase D); K2.2 error list (`limit_reached`); K5.6 (soft) for minute and hour periods.
Acceptance: unit tests for picking the smallest limit across scopes; an integration test that two
concurrent `admit` calls for the last unit of a hard counted limit let exactly one through.

**H1.26 Meters wired in** — M
People, guests, storage (replacing the quota half of `files::require_room`), host-key model calls
(new server AI key setting and `ai_usage.key_source`), heavy automations, notification email,
pushes, rows, form submissions, workspaces owned, and the uploader's own allowance for
direct-scope files (R20); soft notices; the plan limits are read-only to workspace
admins, and `files.workspace_quota_mb` can only lower them.
Depends on: H1.25. Acceptance: existing quota and budget tests still pass; a free-plan workspace
at 10 people refuses an 11th person and accepts an agent; over storage, uploads are refused and
messages still send; export works over every limit; a file sent in a DM counts against the
sender's own allowance and against no workspace.

**H1.27 Spend limits per account, space and conversation** — S
`spend_limits` operations and tools (`spend_limit_set`, `spend_limit_list`), the settings screen,
the inbox notices, `per_account` and the `minute`/`hour` periods counted in K5's store. A2's
budgets are rows in this table checked by the same `admit` (ruling R13); A2.14 adds its meters,
the agent page's budget section and the delivery hold, not a table.
Depends on: H1.25; K5.6 (soft). Acceptance: a limit on one agent's hosted model spend
refuses that agent's next call and not another's; a `per_account` minute limit on a conversation
refuses a person and an agent alike.

**H1.28 Open sign-up with verified email** — M
`signup.mode`, the sign-up flow, workspace creation through `lifecycle::create`, invitation
sign-up without a workspace.
Depends on: H1.17, H1.24, K1.29, K4.5; M1.17 (soft, as in H1.14). Acceptance:
`tests/integration/signup.rs`: sign-up asks for a shape and installs it; no account is usable
before the email is confirmed; `signup.mode = off` refuses every path, including provider
sign-in for a new identity.

**H1.29 Abuse controls and the review queue** — M
Per-address and per-domain sign-up limits, disposable-domain list, proof-of-work challenge,
`review` standing for new workspaces with its restrictions, reports from people.
Depends on: H1.28, H1.1. Acceptance: a disposable domain is refused; a workspace in review cannot
send more than 5 invitations a day.

**H1.30 Sign-in with common providers** — S
Google, GitHub, Microsoft as `IdentityProvider` implementations on H1.18's code.
Depends on: H1.18.

**H1.31 Company single sign-on** — M
Domain verification, per-workspace OIDC or SAML, "require for this domain", as a plan operation.
Depends on: H1.18, H1.25.

**H1.32 Directory sync** — M
SCIM 2.0 users and groups per workspace, token, disabling memberships without deleting history.
Depends on: H1.31; K1.27 (groups, if C2's user groups land as K1 data). Isolation cases for every
SCIM route.

### Phase F: billing, console, operations

**H1.33 Billing seam and plan state** — M
`billing` module, `MerchantOfRecord`, the webhook route, `billing_events`,
`plans::apply_billing_state`, seat sync job; one provider implementation.
Depends on: H1.25, H1.26. Acceptance: replaying the same webhook twice changes nothing the second
time; events out of order end in the provider's current state; with no provider configured no
billing route exists (route table test).

**H1.34 Checkout, trials, downgrades, dunning** — M
Billing page (lazy), checkout and portal links, trials, `over_limit`, `past_due`, `grace`, back
to free.
Depends on: H1.33. Acceptance: a failed payment ends on the free plan after grace with nothing
deleted.

**H1.35 Staff console: core** — M
`apps/staff`, staff sign-in, find a workspace, plan overrides, suspend and resume, `staff_actions`
and the workspace-visible audit event.
Depends on: H1.24, H1.25, H1.20 (`System("staff.console")`).

**H1.36 Staff console: review queue, metrics, support access** — M
Review queue, reports, metrics pages, the support-account guest invitation flow.
Depends on: H1.35, H1.29.

**H1.37 Hosted operations profile** — M
`hosted` profile enforcement, error reporting scrubber, status page component checks, runbook
(`docs/operations/hosted.md`) including the restore drill on managed PostgreSQL; the profile
refuses to start unless every cost meter of 3.8 is registered (R24 rule 8).
Depends on: H1.12, H1.17, H1.26, K5.7, K5.12 (two processes, object storage).

**H1.38 Hosted agent service seam** — S
`host_billing`, marking by signed enrolment, metering calls at dispatch and on usage reports.
Depends on: A1.11 (hosts), A1.3 (`run_usage_report`), A1.13 (one launch path, where the dispatch
check goes), H1.25.

### Phase G: the public release

**H1.39 Release readiness** — M (parts [owner now])
Machine-specific details to configuration or a private repository (with P0 where it overlaps),
neutral demo seed, npm licence check in CI, `THIRD-PARTY-NOTICES` generation, `SECURITY.md`,
`CONTRIBUTING.md` with DCO, README for strangers, docs index, a history scan report for the
owner.
Depends on: H1.19.

**H1.40 Trust documents and publication** — M
Licence files (the licence chosen at release, R25), terms, privacy, DPA, acceptable use,
free-plan and export promises, the history
rewrite if the owner chooses it, the public demo, the announcement checklist.
Depends on: H1.39 and the owner's calls.

**H1.41 Static web app and client config** — S
`GET /client-config` (R30), CORS for configured web origins, the web app reading its API and
live origins from it, a build with no origin baked in, docs for serving `dist/` from a static
host; `/panel-frame/{launch}` stays on `api` (R31).
Depends on: H1.12 (configuration). Can run beside anything in phase C.
Acceptance: the web build served from a different origin signs in and works against the
server; `client-config` holds no secret (a test greps it for every configured secret); the
offline check (6.2 (2)) still passes.

**H1.42 Idle-cost guard and hosting shapes** — M
The 10,000-workspace idle test (3.14), `docs/hosting/shapes.md`, and the fixes the test finds.
Depends on: H1.24 (lifecycle, so empty workspaces are made the real way); K5.7 and K5.18 (the
three roles and the tick endpoint; soft: the test runs the one-process shape until then).
Acceptance: the test passes in CI's nightly run with all three K5 roles in one process; the
document lists each shape's idle cost with how it was measured and names K5's roles, tick
endpoint, pooler rules and live contract.

**H1.43 A workspace's own bucket: screen and plan rule** — S
Settings › Storage over K5.20's `workspace_store_set` (endpoint, bucket, region, key, test
write and read before saving, the move started from here), `storage_bytes` excluding bytes in
the workspace's own bucket, the export listing it as a connection to re-enter.
Depends on: K5.20 (the mechanism), H1.26.
Acceptance: a workspace with its own bucket can upload past the plan's storage limit; the key
never appears in the screen after saving, in logs or in an export.

**Parallel lanes.** H1.1, H1.2, H1.3, H1.12 and H1.20 can start at once. H1.41 runs once H1.12
is in; H1.42 and H1.43 in phase E. H1.4 to H1.7 run beside
each other. H1.13, H1.16 and H1.17 run beside each other once H1.12 is in. Phase E waits for
H1.24; phase F for phase E; phase G for the owner.

---

## 6. Guard rails

### 6.1 The lossless export test

`tests/integration/workspace_roundtrip.rs::export_then_import_loses_nothing`: build the full
two-workspace fixture (`workspace_isolation.rs:584-750`, which already makes one of every kind),
export A, import into a fresh schema, then for every tenant table (by its listed tenant column) compute a
canonical digest of A's rows (columns sorted, ids passed through the import's map, timestamps
kept) and compare. Columns may be skipped only through a module's `exclude` list or a table's
`notExported` reason. A companion test,
`every_workspace_table_is_exported_or_says_why`, reads `information_schema` and the
declarations, so a new table or column with no export decision fails before the round trip does.

### 6.2 Other guards

- **Row security**: `every_tenant_table_has_its_policy` checks `pg_class.relrowsecurity`,
  `relforcerowsecurity` and a policy named `tenancy` using `tenancy_allows(<its tenant column>)` on
  every table not in the global list; the global list in `tenancy/tables.rs` needs a reason per
  entry. The isolation nets run as a non-superuser role.
- **System scope**: a source scan fails on `tenancy::system(` with an unregistered reason or
  outside the files the reason names.
- **Emails use no word of a module that is off (R22).** W1's browser walk can't see mail, so
  H1 checks it: a test renders every mail template (invitations, digests, notification mail,
  sign-up, billing) for a workspace in each of M1's shapes and fails on a word or link that
  belongs to a module that is off there ("project", "task", "agent", "run" for "Friends"). The
  only allowed exception is account information (R31): an agent member's name may carry the
  "agent" badge.
- **Session settings (R24 rule 3)**: K5's lint for session `SET`, session advisory locks and
  hand-written `PREPARE` covers `tenancy/`, `metering/` and `workspace_lifecycle/`.
- **No new global limit**: a source scan of the limiter registry fails on a limiter whose scope
  is server-wide and not `ServerFailuresOnly` unless it is in a short allow-list with a reason.
  A second scan fails on `SELECT count(*) FROM <tenant table>` without the table's tenant column in the
  same statement inside request code (the OAuth client count, `mcp/oauth.rs:197`, is listed as
  global identity).
- **No plan checks outside metering**: a scan fails on reading `workspace_plans` or `plans`
  outside `metering/` and `billing/`.
- **History is never cut**: a scan fails on `DELETE FROM` any content table (messages, revisions,
  pages, tasks, records, events) outside workspace deletion, the owning module's own delete
  operation and `retention.rs`'s allow-listed tables (worktrees, external events).
- **The self-hosted path needs no outside service**: (1) an integration test boots the router
  with a config that has no WorkOS, GitHub, mail, object store, model key or metrics token, with
  `outbound` in a test mode that fails on any connection, and walks first run, sign-in, a
  message, a task, a page, an upload and an export; (2) a web build check fails when the built
  `dist/` references an absolute `http(s)://` origin for scripts, styles, fonts or images; (3)
  the image smoke step runs the compose stack on a network with no internet.
- **No billing on the self-hosted path**: the route table built from a self-hosted config has
  no `/billing` or `/scim` route unless configured, and no `plans` screen is registered.
- **Logs carry no content**: the tracing field-name scan in 3.5.
- **Configuration is documented**: the H1.12 test.

---

## 7. Risks and open questions

### 7.1 Risks

1. **Row security returns empty results silently.** A path that runs without a scope sees no
   rows in strict mode and looks like "nothing here". Notice it: the observe metric must be zero
   across the full integration suite and a week of the live server before strict mode; strict is
   per database setting, so the fallback is turning it off without a deploy.
2. **The workspace column touches about 130 tables while other branches change them.** Merge
   conflicts and missed writers. Notice it: H1.3's guard and the insert trigger mean a missed
   writer still gets the column. Fallback: smaller batches; tables T1 or K3 are replacing are
   left for those workstreams to create with the column.
3. **Backfill on the live server's large tables** (messages, events). The job runs in 5,000-row
   batches at low priority; the switch migration finishes any remainder in batches before
   validating. Watch the job's progress and database load on the Server page.
4. **Export goes stale as modules change.** The round-trip test and the coverage guard fail in
   the pull request that adds the column.
5. **A hostile archive.** Checks in 3.2, the import under its own workspace scope, and strict
   row security; the archive never names paths, only ids and content hashes.
6. **Metering races or double counts.** Hard counted limits lock the counter row; the ledger is
   idempotent by `dedupe_key`.
7. **Superuser databases make row security inert** for self-hosters who don't use the compose
   init. Acceptable (the net matters most for hosted), documented, and `preflight` warns when
   the server's role is a superuser.
8. **Scope settings and a connection pooler.** A session setting would leak scopes between
   requests behind a transaction-mode pooler, so there are none: the scope is `SET LOCAL` in
   each transaction (3.4), and K5's lint fails on a session `SET`. Risk left: a call site
   that forgets `scoped_tx` sees no scope; observe mode finds it before strict mode, and
   strict mode then refuses rather than leaks.
9. **The built-in host is still inside the server** until A1 moves it out; a hosted server must
   switch it off (the `hosted` profile does).

### 7.2 Questions only the owner can answer

1. **The free plan's numbers.** Recommended: 3.8's table (10 people, 10 guests, 2 GB and 25 MB a
   file, unlimited agents with fair use, 1,000 heavy automation runs, no included model calls,
   3 free workspaces per person, full history).
2. **Licence and contributions.** Recommended: AGPL-3.0-or-later for the app, MIT OR Apache-2.0
   for SDKs, protocols and the package format, DCO sign-off, no closed edition.
3. **Git history at release.** Recommended: run a secret and personal-detail scan over the whole
   history first; if it finds only infrastructure addresses and names, rewrite those strings
   once with a history filter at release (keeps authorship and history, changes commit ids).
   The alternative is publishing as is.
4. **Staff impersonation.** Recommended: none; support through an invited guest account.
5. **Who may create more workspaces on a self-hosted server** (A report question 4).
   Recommended: a server setting, operators only by default.
6. **Included model calls on the free plan.** Recommended: none; paid plans include credits.
7. **Hosted infrastructure separate from the owner's server.** Recommended: yes, chosen when
   phase F starts.

---

## 8. Touch points

**H1 needs from:**

- **K1**: `accounts::create_person`, unclaimed accounts and claim links, the `IdentityProvider`
  seam with WorkOS as one implementation and provider-neutral session columns, permission
  `workspace.export` and `workspace.delete` in the catalogue, a server-level operator role
  (replacing the audit-row rule, `server_status.rs:43-68`), a person's current workspaces for the
  tenancy scope, guests as a role; accounts of every kind global with an owner (R19), the
  add-member rule for owned accounts, and matching by outside reference for import.
- **K2**: the operation form for `workspace_export`, `workspace_import`, `workspace_delete`,
  `usage_report`, `spend_limit_set`; the `limit_reached` and `workspace_suspended` errors; the
  runner resolving the path's workspace (ruling R4) and running dispatch inside
  `tenancy::scoped(Tenancy::Workspace(id))` with `app.tenancy = 'w:<id>'` set
  transaction-locally (3.4), not a separate `app.workspace_id`; the runner calling
  `metering::admit` once per write (ruling R13).
- **K3**: the event log exportable per workspace; `workspace.imported` and lifecycle events;
  the links store exportable; events table born with the workspace column.
- **K4**: workspace address rules and uniqueness for `lifecycle::create`; the workspace in every
  URL (to narrow the tenancy scope); the `organization_id` → `workspace_id` rename of the
  existing columns (the columns H1 adds are already named `workspace_id`, ruling R1, with
  foreign keys to `organizations(id)` until K4.22).
- **K5**: shared limiter store (H1.1 supplies keys and scopes), leases (`server.startup`,
  per-workspace permits), the object-store `FileStore` variant, sign-in ceremony state across
  processes, two processes behind one address; the three roles and the tick endpoint (K5.7,
  K5.18), the pooler-safe lint (K5.17), a workspace's own bucket (K5.20), the live contract
  (K5.19) for the edge shape.
- **T1**: new task and record tables born with the workspace column and an export declaration.
- **A1**: the built-in host as its own program and image (A1.17), with the one-time enrolment
  code written where the compose `host` service can read it; host records (A1.11); run usage
  reports (A1.3); calling `metering::admit` at dispatch for a billed host (A1.13, wired by
  H1.38); new tables born with `workspace_id`.
- **K3**: thing-sync triggers that never write `workspace_id` of another table (the R16 table
  in 3.3).
- **A2**: budgets built on `spend_limits` and `admit`; the reconnect flow after import; the
  outside reference re-attach.
- **M1**: `Registrar::export` (M1.3), which H1.8 fills (no `export` section in manifests);
  the shapes (M1.17) that sign-up and first run install; `/panel-frame/{launch}` served by
  `api`; packages sharing the archive's per-kind JSON Lines idea (a package is structure
  without content).
- **K4**: also the direct scope row (`organizations.kind = 'direct'`, K4.23) that row rules,
  metering and export treat specially (R20).
- **C1, C2**: their new tables (proposals, guests, user groups) born with the column and
  declared for export.
- **P0**: machine-specific details out of `tools/ci/run.sh` and `deploy/redhat` if P0 takes it;
  the stale abuse-controls note.

**H1 provides to:**

- everyone: `tenancy::{scoped, system}`, the global-table list and the two guards; the
  workspace column on every table; `outbound` as the only way to call an address a workspace
  chose; `metering::admit` as the only plan or budget check; `workspace_lifecycle::create`.
- **A2**: the spend-limit mechanism for budgets (`spend_limits` with `per_account` and
  `minute`/`hour` periods, `admit`, `record` with dedupe keys); `outbound` (H1.2) for its
  webhook pusher; plan values for stream, push and rate caps (3.5, 3.11).
- **A1**: the compose `host` profile and image for the built-in host; `host_billing` and the
  `agent_service_micros` meter for the hosted agent service, which is otherwise just a host.
- **M1**: the archive format ideas and the `ExportPart` implementations for its registry slot.
- **W1**: `GET /client-config` (R30), so the static web app can be served from anywhere.
- **C2**: the email-words guard over mail templates (W1.39's walk can't see mail).
- **K5**: the per-workspace keys and limits that its shared store holds.

---

## 9. Changes to the end state

1. **Section 6, Hosted, "Tenancy" row.** Add: "Every table carries its workspace column,
   child tables included, except a short published list of server-global identity tables."
   (Today about 129 tables lack it; row rules and per-workspace export need it.)
2. **Section 6, Moving.** Replace "An agent's account and history move with the workspace" with
   "An agent's account, roles and history move with the workspace; its credentials never do, so
   it is reconnected after a move. People are matched by verified email or claim their account;
   agents and apps are matched by their outside reference (R19)."
3. **Section 6, the free plan.** Add the per-file cap and guests: "every module, full history, 10
   people plus 10 guests, 2 GB of files at up to 25 MB each, agents free with fair-use limits".
   (Owner call 5 is otherwise unchanged.)
4. **Section 6, Operations row.** Add: "Staff never impersonate; a workspace that wants help
   invites a support account with a role and an end date."
5. **Section 6, Self-hosted.** Add "First run is protected by a one-time code from the host", so
   a fresh install can't be taken by whoever reaches it first.
6. **Section 10, call 1.** Add the contribution rule to the licence call: "with contributions
   under a sign-off (DCO), not a contributor licence agreement". (R25 defers the licence
   itself: chosen at release, assumed fully open; the DCO rule still applies.)
7. **Section 6, Hosted.** Add: "A workspace's own export holds only conversations that carry its
   id; a person exports their own DMs; the operator's full-server export holds everything"
   (R20).

---

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| H1.1 | Limit scopes, no global bucket for good callers [owner now] | M | (K5 store if present) | H1.2, H1.3, H1.12, H1.20 |
| H1.2 | One outbound guard [owner now] | S | — | H1.1, H1.3, H1.12 |
| H1.3 | Tenant table registry and guard | S | — | H1.1, H1.2, H1.12 |
| H1.4 | Workspace column: chat | M | H1.3 | H1.5, H1.6, H1.7 |
| H1.5 | Workspace column: tasks and planning | M | H1.3; T1 coordination | H1.4, H1.6, H1.7 |
| H1.6 | Workspace column: documents and records | M | H1.3 | H1.4, H1.5, H1.7 |
| H1.7 | Workspace column: agents, runs, code, MCP | M | H1.3; A1.1, A1.22, A2.1, K1.6 coordination | H1.4, H1.5, H1.6 |
| H1.8 | Export declarations | M | H1.3; M1.3 (soft, in-code list until then, R41) | H1.4 to H1.7, H1.12 |
| H1.9 | Workspace export [owner now] | L | H1.8; K1.9, K2.3 (soft: stopgaps named) | H1.12 to H1.17 |
| H1.10 | Workspace import | L | H1.9, H1.24; K1.29; A2.18 (soft) | H1.13, H1.16 |
| H1.11 | Lossless round-trip guard | M | H1.10 | H1.13 to H1.19 |
| H1.12 | Container-ready configuration [owner now] | M | — | Phase A |
| H1.13 | Image and compose | M | H1.12; A1.17 for the host image | H1.16, H1.17 |
| H1.14 | First run in the browser | M | H1.24; K1.29; M1.17 (soft) | H1.16, H1.17 |
| H1.15 | Startup under a lease, upgrades, releases | S | K5.2 | H1.13, H1.16 |
| H1.16 | Portable backup and restore [owner now] | M | H1.12 | H1.13, H1.17 |
| H1.17 | Server mail transport | S | — | H1.13, H1.16 |
| H1.18 | Plain OpenID Connect sign-in | M | K1.30, K5.14 | H1.13 to H1.17 |
| H1.19 | Self-host documentation and offline check | M | H1.13, H1.16 | H1.20 |
| H1.20 | Tenancy scope | M | — (K2.3 soft, for the path scope) | Phase A, C |
| H1.21 | Row security: framework and chat | M | H1.4, H1.20 | H1.23, H1.24 |
| H1.22 | Row security: every other table's policies, observe mode | M | H1.5 to H1.7, H1.21 | H1.25, H1.45 to H1.62 |
| H1.23 | Noisy-neighbour controls | M | H1.1, H1.2; K5.6 | H1.21, H1.24 |
| H1.24 | Workspace lifecycle | M | H1.8; K4.5 | H1.21, H1.23 |
| H1.25 | Metering core | M | H1.24 (soft); K2.2; K5.6 (soft) | H1.22 |
| H1.26 | Meters wired in | M | H1.25 | H1.27, H1.28 |
| H1.27 | Spend limits per account, space, conversation | S | H1.25; K5.6 (soft) | H1.26 |
| H1.28 | Open sign-up with verified email | M | H1.17, H1.24; K1.29, K4.5; M1.17 (soft) | H1.26 |
| H1.29 | Abuse controls and review queue | M | H1.28, H1.1 | H1.30 |
| H1.30 | Sign-in with common providers | S | H1.18 | H1.29 |
| H1.31 | Company single sign-on | M | H1.18, H1.25 | H1.29, H1.30 |
| H1.32 | Directory sync | M | H1.31; K1.27 | H1.33 |
| H1.33 | Billing seam and plan state | M | H1.25, H1.26 | H1.32, H1.35 |
| H1.34 | Checkout, trials, downgrades, dunning | M | H1.33 | H1.35 |
| H1.35 | Staff console: core | M | H1.20, H1.24, H1.25 | H1.33, H1.34 |
| H1.36 | Staff console: review queue, metrics, support access | M | H1.35, H1.29 | H1.37 |
| H1.37 | Hosted operations profile | M | H1.12, H1.17, H1.26; K5.7, K5.12 | H1.35, H1.36 |
| H1.38 | Hosted agent service seam | S | H1.25; A1.3, A1.11, A1.13 | H1.33 to H1.37 |
| H1.39 | Release readiness (parts [owner now]) | M | H1.19 | Phase E, F |
| H1.40 | Trust documents and publication (licence chosen at release, R25) | M | H1.39; owner's calls | — |
| H1.41 | Static web app and client config | S | H1.12 | Phase C |
| H1.42 | Idle-cost guard and hosting shapes | M | H1.24; K5.7, K5.18 (soft) | Phase E |
| H1.43 | A workspace's own bucket: screen and plan rule | S | K5.20, H1.26 | Phase E |
| H1.44 | Unscoped call-site ratchet | S | H1.20 | H1.21, H1.22 |
| H1.45 to H1.62 | Scope call sites, one module directory each (18 slices, listed under H1.45) | S each | H1.44 | each other, Phase D onward |
| H1.63 | Strict row rules | S | H1.22, H1.44 to H1.62 | Phase E |

## Review changes

- 3.2: the generic exporter filters by the tenant column from H1.3's list; `ExportKind` renamed `ExportPart` to match M1's `Registrar::export`, and exports are registered there with no `export` section in manifests. Why: R1, M1's slot; the content and web review's accepted request.
- 3.2: a workspace export carries profile stubs for member accounts of any kind and only conversations with its id; added `direct_export` for a person's own DMs and the operator's full-server export; import matches people by verified email and agents and apps by outside reference. Why: R19, R20.
- 3.3: the column is named `workspace_id` (foreign key to `organizations` until K4.22); existing tables keep `organization_id`; guards read the column from H1.3's list; added the R16 trigger table; global tables born later listed. Why: R1, R16, R19.
- 3.4: per-request narrowing by K2's runner with a transaction-local `app.tenancy`, no `app.workspace_id`; MCP scope for multi-workspace credentials; the scope is now set only by `SET LOCAL` in a transaction (`scoped_tx`, `scoped_one`), replacing the session-level `before_acquire` hook; direct-scope rows get a participants-only policy branch. Why: R4, R19, R24 rule 3 (the hook broke behind a transaction pooler), R20.
- 3.5: A2's caps take plan values through K5's `LimitSource`; `statement_timeout` is `SET LOCAL`. Why: R13, R24.
- 3.6: deleting a workspace keeps global accounts, retires accounts it owns (with notice to other workspaces and a chance to take ownership), and leaves DMs alone. Why: R19, R20.
- 3.1, 3.7: first run and sign-up ask what the workspace is for and install one of M1's shapes (M1.17), chat only included; the setup lock is transaction-scoped. Why: R22, the review's accepted request, R24 rule 3.
- 3.8: `spend_limits` gains `per_account` and minute and hour periods; A2's meters listed; one counting mechanism (K2's runner once per write, K5 store for short periods); `rows` and `pushes` meters and the rule that every cost has a counter before hosted opens; direct-scope storage counts against the uploader's own allowance and no workspace plan touches it. Why: R13, R24 rule 8, R20.
- 3.12: the hosted agent service is an app account and an ordinary host, one `hosts` row per enrolment owned by a workspace or a person, billing marked by a signed enrolment; `admit` at A1.13 dispatch with the usage-report dedupe key. Why: R7, R19, R13.
- 3.13: licence deferred and "chosen at release", assumed fully open. Why: R25.
- 3.14 (new): static web app with H1's `client-config` route; `/panel-frame/{launch}` on `api`; the 10,000-workspace idle guard; `docs/hosting/shapes.md` with three shapes; a workspace's own bucket (mechanism K5.20, screen and plan rule here). Why: R24 rules 1 and 6, R23, R30, R31.
- Compose: the `host` profile ships A1.17's `agent-workspace-host`; before that the built-in host runs embedded under K5.3. Why: self-hosting ships A1's host program.
- 6.2: added the email-words guard and the session-settings lint. Why: R22 (W1's walk can't see mail; accepted request), R24 rule 3.
- 7.1 risk 8 rewritten for `SET LOCAL`. Why: R24 rule 3.
- Slices: dependencies given with slice ids (H1.8 on M1.3; H1.9 on K1.9 and K2.3; H1.10 on K1.29 and A2.18; H1.13 on A1.17; H1.14 and H1.28 on M1.17; H1.15 on K5.2; H1.18 on K1.30 and K5.14; H1.20 on K2.3; H1.23 on K5.6; H1.24 on K4.5; H1.25 on H1.24, K2.2, K5.6; H1.27 no longer on A2; H1.32 on K1.27; H1.37 on H1.26, K5.7, K5.12; H1.38 on A1.3, A1.11, A1.13); H1.20, H1.22, H1.26, H1.9, H1.10 acceptance extended for the above. Why: real dependencies and the rulings.
- New slices H1.41 (static web app and client config), H1.42 (idle-cost guard and shapes), H1.43 (own bucket screen and plan rule). Why: R24, R30, R23.
- Touch points and changes to the end state updated. Why: kept in step with the above.
- 3.7, 3.8, 3.14, H1.42 (kernel review, accepted): `access.outside_accounts` set to `ask_admin` for hosted workspaces; `admit` calls K5's `Limits::check_cost` for rate buckets; the idle guard covers K5's loops; `shapes.md` names K5's three roles, tick endpoint, pooler rules and live contract (with R35's `live.publisher` lease in the edge shape). New columns were already all named `workspace_id` (R1). Why: accepted requests, R35.
- Third pass (R40 to R42): DM files count against a personal allowance per account, one server-wide value the operator sets (unlimited self-hosted, the free plan's file allowance hosted), never a workspace's; H1.8 keeps an in-code `ExportDecl` list until M1.3 (H1.8's dependency on M1.3 is soft again); H1.22 split: H1.22 adds the remaining policies in observe mode, new H1.44 (ratchet of unscoped call sites, may only go down), H1.45 to H1.62 (one mechanical slice per server module directory, plus four alphabetical groups of `src/*.rs`, any order), H1.63 (strict only at zero). Why: R40, R41, R42.
