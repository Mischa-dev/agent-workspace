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
| `AGENT_WORKSPACE_LIVE_ORIGIN` | the public origin | Where browsers open the live connection, when a separate live service answers it, such as `https://live.example.com`. Same rules as the public origin. The web app reads it from `/client-config`. |
| `AGENT_WORKSPACE_ALLOWED_ORIGINS` | the public origin in production; the local dev ports in development | Comma-separated exact origins the browser app may be served from. Writes from any other origin are refused, and only these get CORS headers, so a web app served from a static host on another origin of the same site lists that origin here (see `static-web-app.md`). Required in production when no public origin is set. |
| `AGENT_WORKSPACE_WEB_ROOT` | `apps/web/dist` | The built web app this server serves. |
| `AGENT_WORKSPACE_LOG_FILTER` | `agent_workspace=info,tower_http=info` | Which log lines are written, in `tracing` filter syntax. |
| `AGENT_WORKSPACE_OPERATOR_LOGINS` | none | Login names, comma separated, of the people who run this server, besides whoever set it up from its terminal. Only they see Settings › Workspace › Server. |

## Database and storage

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_DATABASE_URL` | required | The PostgreSQL connection URL. Secret: prefer `AGENT_WORKSPACE_DATABASE_URL_FILE`. |
| `AGENT_WORKSPACE_DATABASE_MAX_CONNECTIONS` | `5` | Connections in the server's pool. |
| `AGENT_WORKSPACE_DATA_ROOT` | `.agent-workspace` | Where uploaded files, artifacts and other data on disk live. |
| `AGENT_WORKSPACE_DATA_QUOTA_BYTES` | `21474836480` | The most the data root may hold (at least 1 GiB). |
| `AGENT_WORKSPACE_MINIMUM_FREE_BYTES` | `5368709120` | Free disk the server keeps; uploads are refused below it (at least 1 GiB). |
| `AGENT_WORKSPACE_SECRETS_KEY_PATH` | none (development: a keyring under `.agent-workspace-secrets/`) | The keyring that seals stored credentials: an owner-only file outside the data root, the web root and any Git work tree. If it names a file that doesn't exist yet in an existing directory, the server creates the keyring there on first start and logs the path (never the key). Back it up apart from database dumps. |

## Background work and limits

| Variable | Default | Meaning |
| --- | --- | --- |
| `AGENT_WORKSPACE_JOB_CONCURRENCY` | `4` | Background jobs run at once in this process (1 to 64). |
| `AGENT_WORKSPACE_LINK_PREVIEWS` | `true` | Whether this server fetches previews of links people post. Off, no workspace gets them, whatever its own setting says. |
| `AGENT_WORKSPACE_ALLOW_PRIVATE_ADDRESSES` | `false` | Lets workspaces point their model provider and mail server at private, loopback and tailnet addresses, and mail at any port. Turn it on only for a local model or mail relay. |
| `AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS` | none | Comma-separated exact origins (such as `http://100.64.0.1:8732`) that agent wakeups and webhooks may reach over http or on a private address. Every other webhook must be public https. |
| `AGENT_WORKSPACE_PUSH_CONTACT` | the public origin when https, else a mailto at its host | Who push services contact about this server's web push: a `mailto:` or `https:` URL. |
| `AGENT_WORKSPACE_METRICS_TOKEN_PATH` | none (no `/metrics`) | An owner-only file holding the bearer token `/metrics` requires. |

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
| `AGENT_WORKSPACE_CODEX_HOME_ROOT` | `<data root>/codex/profiles` | Where harness account homes live; must be inside the data root. |
| `AGENT_WORKSPACE_CODEX_HOST_CONCURRENCY_LIMIT` | `1` | Harness processes at once (1 to 16). |
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
