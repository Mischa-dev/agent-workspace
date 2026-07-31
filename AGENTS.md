# Repository guidance

Read `PROJECT_SOURCE_OF_TRUTH.md` before changing product behavior or
architecture. It is canonical.

## Commands

- Install frontend dependencies: `corepack pnpm install`
- Run frontend: `corepack pnpm dev:web`
- Run backend: `corepack pnpm dev:server`
- Run all required local checks: `corepack pnpm check`
- Start PostgreSQL with Docker: `corepack pnpm db:up`
- Start PostgreSQL on Red Hat: `podman-compose -f docker-compose.dev.yml up -d`

## Verification

Run checks relevant to every changed subsystem. Before a pull request, run:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
corepack pnpm lint
corepack pnpm typecheck
corepack pnpm test
corepack pnpm build
```

Database changes require migration tests against real PostgreSQL. Provider
changes require contract fixtures and the compatibility note for the pinned
provider version.

## Architecture rules

- Keep one Rust server crate until module boundaries are proven.
- Keep provider-specific JSON inside provider modules.
- Resolve Actor identity server-side; clients cannot choose authorship.
- Keep application Threads, Agent Runs, and provider sessions separate.
- Use argument arrays for Git and process execution; never interpolate a shell
  command from user input.
- Never expose secrets to the browser, logs, database records, provider child
  environments, or repository files.
- Update the Decision Log in `PROJECT_SOURCE_OF_TRUTH.md` in the same pull
  request as any architectural change.
