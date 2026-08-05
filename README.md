# Agent Workspace

Agent Workspace is a desktop-first project environment that connects durable
conversation, structured work, hosted execution workers, and GitHub delivery.

The canonical product and engineering plan is
[PROJECT_SOURCE_OF_TRUTH.md](./PROJECT_SOURCE_OF_TRUTH.md).

## Development

Prerequisites:

- Rust 1.97.1
- Node.js 24.18.1
- pnpm 11.18.0 through Corepack
- PostgreSQL 18.4

Common commands:

```bash
corepack pnpm install
corepack pnpm dev
cargo run -p agent-workspace-server
corepack pnpm check
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
```

Copy `.env.example` to `.env` for local development. Never commit `.env`,
Codex credentials, GitHub App keys, webhook secrets, or bearer tokens.

Start PostgreSQL, then apply migrations and create the idempotent M0 seed:

```bash
corepack pnpm db:up
cargo run -p agent-workspace-server -- dev-seed
```

The seed creates the initial Actor, Organization, Project, `#implementation`
Channel, `M0 walking skeleton` Thread, `@agent` execution worker, provider
connection, retained execution profile, and trusted spike repository. Set
`AGENT_WORKSPACE_DEV_REPOSITORY_MIRROR` to the absolute bare-mirror path.
`AGENT_WORKSPACE_DEMO_EXECUTOR=true` enables the development-only deterministic
approval/file-change loop; production configuration rejects this shortcut.

For a new database, create the first browser administrator from an interactive
host terminal. The password is prompted twice and is never accepted in an
argument or environment variable:

```bash
cargo run -p agent-workspace-server -- bootstrap-admin \
  --login-name mischa \
  --display-name "Mischa Nelson" \
  --organization-name "Personal" \
  --organization-slug personal
```

On an upgraded M0 database, the command adopts the single unambiguous active
human Project owner. If more than one ownership pair exists, rerun it with the
non-secret `--adopt-actor <uuid> --organization-id <uuid>` selection. Bootstrap
is permanently refused after the first User commits.

The Rust server serves the built frontend and API on `127.0.0.1:3100`. During
frontend development, Vite proxies HTTP and WebSocket traffic to that port.

The Red Hat build operator uses `deploy/redhat/install-build-rust.sh` for an
isolated pinned Rust toolchain. Its `CARGO_HOME` and `RUSTUP_HOME` live under
the bot account and do not replace Red Hat's `/usr/bin/rustc`.

The supported private-host installation, upgrade, rollback, recovery, and
maintenance procedure is in
[`docs/self-hosted-red-hat.md`](docs/self-hosted-red-hat.md).
