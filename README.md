# Agent Workspace

Agent Workspace is a desktop-first project environment that connects durable
conversation, structured work, hosted execution workers, and GitHub delivery.

The canonical product and engineering plan is
[PROJECT_SOURCE_OF_TRUTH.md](./PROJECT_SOURCE_OF_TRUTH.md).

## M0 development

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

The Red Hat build operator uses `deploy/redhat/install-build-rust.sh` for an
isolated pinned Rust toolchain. Its `CARGO_HOME` and `RUSTUP_HOME` live under
the bot account and do not replace Red Hat's `/usr/bin/rustc`.
