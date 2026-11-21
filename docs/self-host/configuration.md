# Configuration

Agent Workspace is one program, `agent-workspace-server`, configured entirely by
environment variables that start with `AGENT_WORKSPACE_`. This page lists every one.
A unit test (`config::tests::every_variable_is_documented`) fails when the server reads
a variable this page doesn't name.

## Secrets as files

Any variable can instead be read from a file: set `AGENT_WORKSPACE_<NAME>_FILE` to the
file's path, and the server reads `AGENT_WORKSPACE_<NAME>` from it (a trailing newline is
dropped). Setting both is refused. Use it for anything secret-bearing, such as
`AGENT_WORKSPACE_DATABASE_URL_FILE` in a container with secrets mounted under
`/run/secrets`. Variables whose name ends in `_PATH` already name a file and are read
the same way they always were.

## Profiles

`AGENT_WORKSPACE_PROFILE` picks a set of defaults. A profile only sets defaults: a
variable that is set still wins.

| Profile | Defaults |
| --- | --- |
| `standard` (default) | The defaults in the tables below. |
| `small` | One small machine (about 1 vCPU and 1 GB): `DATABASE_MAX_CONNECTIONS=4`, `JOB_CONCURRENCY=1`, `LINK_PREVIEWS=false`. The numbers are a starting guess, not a measurement. |
| `hosted` | A server many workspaces share: `DATABASE_MAX_CONNECTIONS=20`, `JOB_CONCURRENCY=8`. It refuses to start with `ALLOW_PRIVATE_ADDRESSES=true`, any `WEBHOOK_TRUSTED_ORIGINS` or `DEMO_EXECUTOR=true`. |

## Server and addresses

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_ENV` | `development` | `development`, `test` or `production`. Production uses `__Host-` cookies and refuses development helpers. |
| `AGENT_WORKSPACE_PROFILE` | `standard` | The set of defaults (above). |
| `AGENT_WORKSPACE_SERVER_ADDR` | `127.0.0.1:3100` | Where the app and its API listen. Development must listen on loopback. |
| `AGENT_WORKSPACE_WEBHOOK_ADDR` | `127.0.0.1:3101` | Where GitHub webhooks are received, apart from the app. |
| `AGENT_WORKSPACE_PUBLIC_ORIGIN` | the server address | The address people and agents use, such as `https://workspace.example.com`. It must be https, except on `localhost` or a loopback address, where plain http works in production too (browsers treat localhost as secure, so a laptop trial needs no certificate). OAuth metadata, MCP resource URLs and links in mail are built from it. |
| `AGENT_WORKSPACE_LIVE_ORIGIN` | the public origin | Where browsers open the live connection, when a separate live service answers it, such as `https://workspace.example.com:8443`. Same rules as the public origin, and it must be on the public origin's host (another port is fine): sign-in cookies belong to one host, so the server refuses a live origin on another one when it starts. The web app reads it from `/client-config`, and the server's Content-Security-Policy lets its pages connect there. |
| `AGENT_WORKSPACE_ALLOWED_ORIGINS` | the public origin in production; the local dev ports in development | Comma-separated exact origins the browser app may be served from. Writes from any other origin are refused, and only these get CORS headers, so a web app served from a static host on another origin of the same site lists that origin here (see `static-web-app.md`). Required in production when no public origin is set. |
| `AGENT_WORKSPACE_MCP_HOSTS` | none | Comma-separated extra names (`app` or `app:3100`) the MCP endpoint accepts as the address it is called by, besides loopback and the origins above. A host program that reaches the server by an internal name, such as the container setup's `host` service calling `http://app:3100`, needs that name here. |
| `AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER` | none | The request header that carries the caller's address, as the proxy in front of the server sets it (`cf-connecting-ip` behind Cloudflare). Per-address sign-in and reset limits use it. Unset, no address is read from requests. Set it only when every request comes through that proxy, or callers could name any address. |
| `AGENT_WORKSPACE_WEB_ROOT` | `apps/web/dist` | The built web app this server serves. |
| `AGENT_WORKSPACE_LOG_FILTER` | `agent_workspace=info,tower_http=info` | Which log lines are written, in `tracing` filter syntax. |
| `AGENT_WORKSPACE_OPERATOR_LOGINS` | none | Login names, comma separated, of the people who run this server, besides whoever set it up from its terminal. Only they see Settings › Workspace › Server. |

## Database and storage

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_DATABASE_URL` | required | The PostgreSQL connection URL. Secret: prefer `AGENT_WORKSPACE_DATABASE_URL_FILE`. |
| `AGENT_WORKSPACE_DATABASE_MAX_CONNECTIONS` | `10` | Connections in this process's pool, not counting the one listening connection. Several processes on one database each open their own: keep their sum (plus one listener each) under PostgreSQL's `max_connections`. |
| `AGENT_WORKSPACE_DATABASE_STATEMENT_CACHE` | `100` | Prepared statements each pooled connection keeps. Set `0` when the database URL goes through a transaction-mode pooler, which can't keep a connection's statements between transactions. |
| `AGENT_WORKSPACE_DATABASE_LISTEN_URL` | the database URL | Where the one listening connection (outside the pool) goes for wakes and live frames. Point it straight at PostgreSQL when the main URL goes through a transaction-mode pooler. `off` runs on polls alone: live updates and lease hand-overs then take up to a second longer, and live text, sign-outs and module switches reach only the server they happen on. |
| `AGENT_WORKSPACE_DATA_ROOT` | `.agent-workspace` | Where uploaded files, artifacts and other data on disk live. |
| `AGENT_WORKSPACE_DATA_QUOTA_BYTES` | `21474836480` | The most the data root may hold (at least 1 GiB). |
| `AGENT_WORKSPACE_MINIMUM_FREE_BYTES` | `5368709120` | Free disk the server keeps; uploads are refused below it (at least 1 GiB). |
| `AGENT_WORKSPACE_FILE_STORE` | `local` | Where new uploads go: `local` (under the data root) or `object` (the S3-compatible store below). Files already stored stay where they are and are still served. |
| `AGENT_WORKSPACE_S3_ENDPOINT` | none | An S3-compatible store's address, such as `https://s3.us-east-1.amazonaws.com` or a MinIO server's. Set it with the bucket and both key paths, or none of them. |
| `AGENT_WORKSPACE_S3_BUCKET` | none | The bucket files go in. |
| `AGENT_WORKSPACE_S3_REGION` | `us-east-1` | The bucket's region (`auto` for stores that ignore it). |
| `AGENT_WORKSPACE_S3_PREFIX` | none | A folder inside the bucket every key goes under. |
| `AGENT_WORKSPACE_S3_PATH_STYLE` | `false` | `true` for stores that want `endpoint/bucket/key` addresses (MinIO and most self-hosted ones). |
| `AGENT_WORKSPACE_S3_ACCESS_KEY_ID_PATH` | none | An owner-only file holding the access key id. |
| `AGENT_WORKSPACE_S3_SECRET_ACCESS_KEY_PATH` | none | An owner-only file holding the secret access key. |
| `AGENT_WORKSPACE_SECRETS_KEY_PATH` | none (development: a keyring under `.agent-workspace-secrets/`) | The keyring that seals stored credentials: an owner-only file outside the data root, the web root and any Git work tree. If it names a file that doesn't exist yet in an existing directory, the server creates the keyring there on first start and logs the path (never the key). Back it up apart from database dumps. |

With an object store, clients can also upload straight to it (`file_upload_start`, then `file_upload_finish`): the server signs a 15-minute `PUT` link fixed to the file's size, type and SHA-256 and checks what arrived, so large files never pass through it. Browsers need the bucket's CORS rules to allow `PUT` from the app's address with the `content-type` and `x-amz-checksum-sha256` headers. A workspace can also keep its uploads in a bucket of its own (`workspace_store_set`, for those who may manage its settings): the server writes, reads and deletes a test object there first, keeps the keys sealed in the keyring, and reaches the bucket only at a public address unless `AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES` is on.

## Background work and limits

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_JOB_CONCURRENCY` | `4` | Background jobs run at once in this process (1 to 64). |
| `AGENT_WORKSPACE_LEASE_SECONDS` | `15` | How long a server's hold on single-holder work (the built-in host, the event router) lasts unless renewed (2 to 300). A server that stops without letting go hands its work on after this long. |
| `AGENT_WORKSPACE_INSTANCE_LABEL` | none | A name for this server process when several share one database, shown with its instances (at most 80 characters). |
| `AGENT_WORKSPACE_ROLES` | `api,live,work` | What this process does, any of the three, comma separated. `api` serves the app, its API, MCP and GitHub webhooks; `live` holds browsers' live connections; `work` runs background jobs, the event log's sequencer and consumers, replies and the one-off startup work. A process with neither `api` nor `live` answers health checks only. Every role needs at least one process somewhere. Only `work` applies database migrations as it starts; an `api` or `live` process starts cold and answers `/health/ready` with 503 until the schema is its release's, or run `agent-workspace-server migrate` first. With the work loop off, `live` processes run the log's sequencer. |
| `AGENT_WORKSPACE_BUILTIN_HOST` | `on` | Whether a `work` process may run agents on this machine (the built-in host: runs, their recovery, repositories on its disk). One process at a time holds it. `off` on processes whose machine shouldn't. |
| `AGENT_WORKSPACE_HOST_SERVER_URL` | none | Read by `agent-workspace-host`, the built-in host as its own program: the server's address, such as `http://app:3100`. Run it with the same database, data folder and keyring as the server, and the server with `AGENT_WORKSPACE_BUILTIN_HOST=off`; then restarting the server leaves running agents alone, and restarting the host recovers its runs. With the built-in host off, a `work` server writes a one-time code to `<data root>/builtin-host/code` until a host has enrolled; the host exchanges it once and keeps its token beside it. `agent-workspace-server enrol-builtin-host --code-file <path>` writes a new code and cuts off the host before it. |
| `AGENT_WORKSPACE_HOST_CODE_PATH` | `<data root>/builtin-host/code` | Where the server writes, and the host program reads, that one-time code, when they don't share the data folder's path. |
| `AGENT_WORKSPACE_DRAIN_SECONDS` | `5` | When asked to stop, how long this process answers `/health/ready` with 503 before it closes its live connections (browsers reconnect to another process) and stops taking requests (0 to 120). The service manager's stop timeout must be longer than this plus the longest request. |
| `AGENT_WORKSPACE_STARTUP_WAIT_SECONDS` | `600` | How long a starting `work` process waits while another one does the startup work (migrations and the like; one process at a time) before it exits non-zero so the service manager starts it again (10 to 86400). |
| `AGENT_WORKSPACE_UPDATE_CHECK_URL` | none | A release feed (https) read once a day: a JSON document `{"latest": "0.2.0", "notesUrl": "https://…"}`. When it names a newer release than the running one, the Server page and `preflight` say so. Nothing is sent but the running version in the user agent. |
| `AGENT_WORKSPACE_UPDATE_CHECK` | `on` | `off` stops the update check even when a feed is set. |
| `AGENT_WORKSPACE_HEALTH_ADDR` | the server address | Where a process with neither `api` nor `live` answers `/health/live`, `/health/ready` and `/health/version`. |
| `AGENT_WORKSPACE_WORK_LOOP` | `on` | Whether a `work` process runs its queue work (jobs, the log's consumers, agents' wake-ups, GitHub jobs) by itself. `off`: only when something calls `POST /internal/work/tick`, so an outside timer can drive it and nothing has to stay awake. The built-in host, when on, keeps running either way. |
| `AGENT_WORKSPACE_WORK_TICK_SECRET_PATH` | none (no tick) | An owner-only file holding the bearer token `POST /internal/work/tick` requires. Each tick runs one round of work for at most 25 seconds and answers with what it did and `"more": true` when more is due; call it again then. |
| `AGENT_WORKSPACE_LIVE_URL` | none (this server's own live role) | Where an outside live service implementing `docs/contracts/live.md` takes publishes, such as `http://10.0.0.7:3200`. Set on every process: one `work` process publishes the log there, `api` processes send it presence, typing and streamed text, and a `live` process of this server then takes its frames only from those publishes. |
| `AGENT_WORKSPACE_LINK_PREVIEWS` | `true` | Whether this server fetches previews of links people post. Off, no workspace gets them, whatever its own setting says. |
| `AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES` | `false` | Lets workspaces point their model provider and mail server at private, loopback and tailnet addresses, and mail at any port. Turn it on only for a local model or mail relay. |
| `AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS` | none | Comma-separated exact origins (such as `http://wake.internal:8732` for a listener on a private network) that agent wakeups and webhooks may reach over http or on a private address. Every other webhook must be public https. |
| `AGENT_WORKSPACE_PUSH_CONTACT` | the public origin when https, else a mailto at its host | Who push services contact about this server's web push: a `mailto:` or `https:` URL. |
| `AGENT_WORKSPACE_DIRECTORY_DIR` | none | An absolute path to a folder of packages (`.json`, `.zip`, or folders holding a `package.json`) every workspace on this server can install from Settings › Modules › Available. Read again every hour. |
| `AGENT_WORKSPACE_DIRECTORY_URLS` | none | Package lists (`agent-workspace-directory/1` JSON over https), comma or space separated, every workspace can install from. Read again every hour. |
| `AGENT_WORKSPACE_DIRECTORY_WORKSPACE_SOURCES` | `true` (`false` in the hosted profile) | Whether a workspace may add its own package lists. |
| `AGENT_WORKSPACE_METRICS_TOKEN_PATH` | none (no `/metrics`) | An owner-only file holding the bearer token `/metrics` requires. |
| `AGENT_WORKSPACE_WORKSPACE_CALLS_PER_MINUTE` | `3000` | MCP and public API calls one workspace's agents and apps may make together each minute, beside each credential's own 600. See `docs/operations/abuse-controls.md`. |
| `AGENT_WORKSPACE_OAUTH_MAX_UNCLAIMED_CLIENTS` | `1024` | Apps that registered but never finished connecting, kept before new registrations are refused. |

## Mail (optional)

Without these, account mail (password resets, address confirmations) goes out through a workspace's own mail settings, and only to people who own that workspace or belong to no other.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_MAIL_URL` | none | The server's own mail transport, used for every account email: `smtps://user:password@host` (TLS from the first byte, port 465 unless given), `smtp://user:password@host` (STARTTLS, required; port 587 unless given) or `smtp+plain://localhost:25` (a relay on this machine only). Percent-encode `@` and `:` in the user name and password. The mail server must be a public address on a mail port unless private addresses are allowed. Workspaces' own mail settings still send their notification email. |
| `AGENT_WORKSPACE_MAIL_FROM` | none (required with the URL) | The address account email comes from. |
| `AGENT_WORKSPACE_MAIL_FROM_NAME` | none | The name shown with it. |

## Sign-in through WorkOS (optional)

Unset, people sign in with a password and two-factor codes.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_WORKOS_CLIENT_ID` | none | The WorkOS client id (`client_...`). Set with the key file. |
| `AGENT_WORKSPACE_WORKOS_API_KEY_PATH` | none | An owner-only file holding the WorkOS API key (`sk_...`). |
| `AGENT_WORKSPACE_WORKOS_ALLOW_SIGNUP` | `false` | Whether people may sign up through WorkOS. |
| `AGENT_WORKSPACE_WORKOS_AUTHKIT_DOMAIN` | none | The AuthKit domain, needed for agent registration. |
| `AGENT_WORKSPACE_WORKOS_GOOGLE_ENABLED` | `false` | Offer "Sign in with Google". |
| `AGENT_WORKSPACE_WORKOS_AGENT_REGISTRATION_ENABLED` | `false` | Let agents register through WorkOS (needs the AuthKit domain). |
| `AGENT_WORKSPACE_WORKOS_API_BASE` | `https://api.workos.com` | Development only: a loopback stand-in for browser tests. |

## Sign-in with OpenID Connect (optional)

One issuer people sign in with: a company identity service, or any provider
that speaks OpenID Connect. Register this server with it as a confidential
web client whose redirect address is `<public origin>/auth/oidc/callback`,
asking for the `openid email profile` scopes. Set all three of the first
variables together.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_OIDC_ISSUER` | none | The issuer's identifier, an `https` URL; its discovery document is read from `<issuer>/.well-known/openid-configuration` on each sign-in. |
| `AGENT_WORKSPACE_OIDC_CLIENT_ID` | none | This server's client id at the issuer. |
| `AGENT_WORKSPACE_OIDC_CLIENT_SECRET_PATH` | none | An owner-only file holding the client secret. |
| `AGENT_WORKSPACE_OIDC_NAME` | `single sign-on` | What the sign-in button says: "Continue with ...". |
| `AGENT_WORKSPACE_OIDC_ALLOW_SIGNUP` | `false` | Whether someone the issuer vouches for, with a verified email and no account here, gets one. Off, they need an account first (an invitation, or the operator). |
| `AGENT_WORKSPACE_OIDC_JOIN` | none | `domain=workspace-address`, such as `example.com=acme`: people whose verified email is in the domain join that workspace as members when they first sign in. |

The first sign-in links the issuer's identity to the account whose verified
email matches, or to the signed-in account when someone links it from
Account settings. An email the issuer doesn't say it verified never links
an account and never makes one.

## GitHub (optional)

The App id, private key and webhook secret are set together or not at all.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_GITHUB_APP_ID` | none | The GitHub App's id. |
| `AGENT_WORKSPACE_GITHUB_PRIVATE_KEY_PATH` | none | An owner-only file holding the App's private key. |
| `AGENT_WORKSPACE_GITHUB_WEBHOOK_SECRET_PATH` | none | An owner-only file holding the webhook secret. |
| `AGENT_WORKSPACE_GITHUB_WEBHOOK_PREVIOUS_SECRET_PATH` | none | The previous webhook secret while rotating it. |
| `AGENT_WORKSPACE_GITHUB_APP_SLUG` | none | The App's URL name, to connect GitHub from settings. Set with the next two. |
| `AGENT_WORKSPACE_GITHUB_CLIENT_ID` | none | The App's OAuth client id. |
| `AGENT_WORKSPACE_GITHUB_CLIENT_SECRET_PATH` | none | An owner-only file holding the App's OAuth client secret. |
| `AGENT_WORKSPACE_GITHUB_API_BASE_URL` | `https://api.github.com` | Only this value is supported. |
| `AGENT_WORKSPACE_GITHUB_API_VERSION` | the pinned version | Only the pinned version is supported. |

## The built-in host (until it is its own program)

These configure the harnesses the server can run agents with on its own machine. A
container image does not need them.

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_CODEX_BINARY` | `codex` | The Codex command line. |
| `AGENT_WORKSPACE_PROVIDER_HOME_ROOT` | `<data root>/codex/profiles` | Where harness account homes live; must be inside the data root. |
| `AGENT_WORKSPACE_HARNESS_HOST_CONCURRENCY_LIMIT` | `1` | Harness processes at once (1 to 16). |
| `AGENT_WORKSPACE_CODEX_MAX_LINE_BYTES` | `1048576` | The longest line read from a harness. |
| `AGENT_WORKSPACE_CODEX_MAX_STDERR_BYTES` | `262144` | Error output kept from a harness. |
| `AGENT_WORKSPACE_CODEX_MAX_BUFFERED_EVENTS` | `64` | Events buffered from a harness. |
| `AGENT_WORKSPACE_CODEX_REQUEST_TIMEOUT_SECONDS` | `120` | How long a harness request may take. |
| `AGENT_WORKSPACE_CODEX_SHUTDOWN_TIMEOUT_SECONDS` | `5` | How long a harness gets to stop. |
| `AGENT_WORKSPACE_CODEX_CLOUD_POLL_MILLISECONDS` | `15000` | How often a cloud task's status is read. |
| `AGENT_WORKSPACE_CLAUDE_BINARY` | `claude` | The Claude Code command line. |
| `AGENT_WORKSPACE_ANTHROPIC_API_URL` | `https://api.anthropic.com` | The provider's API for its cloud runs (https, or loopback http in tests). |
| `AGENT_WORKSPACE_COMMAND_HARNESS_DIRS` | none | Absolute directories, colon separated, that a registered command-line harness's program must sit in. |
| `AGENT_WORKSPACE_DEMO_EXECUTOR` | `false` | Development only: a stand-in harness. |
