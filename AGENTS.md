# Repository guidance

The current direction is in `docs/plans/roadmap.md`: the workstreams, in
waves, that make this app the platform for whole projects and for agents of
any make, including your own harnesses. Each workstream is one agent session
and one pull request; follow its "How an agent runs a workstream" section.
It builds on
`docs/plans/workspace-merge-plan.md`, which made this app the one workspace
(DMs, groups, agents answering on their own, plus projects, work items, Codex
runs and GitHub). `PROJECT_SOURCE_OF_TRUTH.md` is history and a reference for
how existing code behaves; don't add to it for ordinary changes.

## Skills for working here

Step-by-step guides for the jobs that keep coming up are in
`.claude/skills/<name>/SKILL.md`; read the one that fits before you start:
`add-mcp-tool` (a tool on the app's MCP server), `add-migration` (any
database change), `check-in-browser` (looking at a web change in a real
browser) and `steward` (the checks before pushing, and driving a pull
request to green). Claude Code loads them by itself.

## Commands

- Install frontend dependencies: `corepack pnpm install`
- Run frontend: `corepack pnpm dev:web`
- Run backend: `corepack pnpm dev:server`
- Start PostgreSQL with Docker: `corepack pnpm db:up`
- Start PostgreSQL on Red Hat: `podman-compose -f docker-compose.dev.yml up -d`
- Bootstrap the first browser administrator: `cargo run -p agent-workspace-server -- bootstrap-admin --login-name <name> --display-name <name> --organization-name <name> --organization-slug <slug>` (interactive terminal only)
- Rotate one User password and revoke their sessions: `cargo run -p agent-workspace-server -- user-password-rotate --login-name <name>` (interactive terminal only)

In a cloud session (a Claude Code cloud session or a Codex Cloud task),
`tools/cloud/setup.sh` starts PostgreSQL, creates a test database, saves
`AGENT_WORKSPACE_DATABASE_URL` for later commands, installs the Node in
`.node-version` and the pinned pnpm, installs the web dependencies and warms
the cargo build. Use it as the Codex Cloud environment's setup script. Run it
again if integration tests skip or can't reach the database. Where Node can't
be downloaded the image's Node 22 is used, so keep the web checks passing on
it (stub `fetch` and `WebSocket` in tests instead of letting jsdom connect).

Builds and tests don't run on the Mac (it's short on disk). `tools/ci/run.sh`
runs the full CI on the best machine that's free: Mischa's NixOS laptop when it
is idle (on mains power, no game, GPU and CPU quiet), otherwise the Red Hat
host. It checks the working tree as it is, so run it before pushing; add
`--areas rust` or `--areas web` to run one side. Release builds for a deploy
always happen on the Red Hat host, which serves the app. Cap build memory so a
build can never push other services into the OOM killer.

## How much to check

- While building: `cargo check`, plus the tests for the code you changed.
- Before pushing or deploying: `tools/ci/run.sh` (format, clippy, all tests,
  web lint/typecheck/test/build; the same checks GitHub runs). Then, for a
  deploy, try the change once on the live server.
- Write tests where a bug would be silent and costly: permissions and private
  conversations, claims and leases, reply rules, credentials. Don't write test
  matrices for UI details, and don't produce acceptance-evidence documents.
- Database changes get a migration; integration tests use real PostgreSQL via
  `AGENT_WORKSPACE_DATABASE_URL`.

## Working alongside other branches

Several agents build at once, so changes must merge in either order.

- Name new migrations by UTC timestamp, such as
  `apps/server/migrations/20261020143000_settings.sql`. The numbered ones stop
  at 0054; a test refuses new numbered ones.
- Don't pin counts other branches change: tests check that every MCP tool is
  well formed instead of counting them, and the web budget holds the initial
  load and each lazy chunk, not a total.
- In shared registries (the tool list in `mcp.rs`, routes in `api.rs` and
  `App.tsx`, `policy_inventory.rs`, `ui/index.ts`) add lines next to related
  ones rather than at the end, and don't reorder.

## Architecture rules

- Keep one Rust server crate until module boundaries are proven.
- Keep provider-specific JSON inside provider modules.
- Resolve Actor identity server-side; clients cannot choose authorship.
- Conversation access goes through `conversation_visible_to` (migration 0037).
  Never add a conversation query that skips it.
- MCP calls are checked against the caller's current memberships every time
  (`actor_can_access_project`, migration 0042). Apps connected by a person act
  as that person; agents act as themselves. See
  `docs/plans/agent-interface.md` for the tools.
- Keep application Threads, Agent Runs, and provider sessions separate.
- Use argument arrays for Git and process execution; never interpolate a shell
  command from user input.
- Never expose secrets to the browser, logs, database records, provider child
  environments, or repository files.
