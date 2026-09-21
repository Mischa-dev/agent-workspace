# Workspace

A private app for talking with AI agents, keeping shared material, planning projects, and reviewing bounded local work. The connected release adds generic agent accounts, MCP/HTTP, shared space boards, channels/threads and planning views. The broader product roadmap is retained in `planning/`; its AgentOS and publishing milestones are not all implemented.

Implementation follows the [launch plan](planning/NEXT_IMPLEMENTATION_PLAN.md) and [six-package backlog](planning/NEXT_BACKLOG.json). See the [release status](planning/CONNECTED_RELEASE.md) for delivered features and the remaining host/client setup. The [longer-term roadmap](planning/LONG_TERM_ROADMAP.md) retains deeper AgentOS management, GitHub automation, memory, and scale work. [ACCEPTANCE.md](ACCEPTANCE.md) remains the evidence for what currently works.

## Open it

Open http://127.0.0.1:4310. The owner token is stored privately in `.local/owner-token`. To copy it without printing it:

```sh
cd /Users/mischanelson/Documents/ChatGPT/workspace
pbcopy < .local/owner-token
```

Paste the token into the sign-in screen. It stays out of the browser bundle, Git, and application logs. Your browser then uses an HttpOnly session cookie.

## Daily use

1. Open **Messages** and talk to Atlas or Ember. Both are separate identities using the qualified local Codex runtime. The workroom contains both. Auto selects one relevant recipient; mentions, replies, and the recipient menu let you override it. Group controls offer independent opinions and a discussion. Stop closes admission until Continue.
2. Save useful notes, pastes, and files as **Resources**. Share deliberately with a conversation or link a project/space. Agent notes use the same persistent records. Attachments retain the chosen revision; later edits do not replace it.
3. Use a **Space** as a shared artifact board: pin/tag notes and files, then attach them to channels or tasks. Create a **Project** with an outcome and use its Outline, Board or Dependencies view. Capture a task with just a title or use **Plan with an agent**. Review a proposed outline before applying it. Add outcomes, acceptance criteria, dependencies, required Resource revisions, blockers, alternatives, and shared locks as work becomes clear.
4. Use **Start work session** to preview exact task and project revisions and approve a bounded scope. Start it immediately or keep the scope for a later webhook. Default limits are two workers, 60 minutes, and a 20% finishing/review reserve. Tasks wait on prerequisites and locks. Results land in Inbox and the project Activity view for review.
5. Code tasks need a local repository and target ref in Project settings. The worker creates a separate worktree and generated local branch, saves a checkpoint, and leaves your target checkout untouched. Review the output and diff. Accepting a code result does not integrate it; after you integrate its commit yourself, **Verify integration** checks actual ancestry before dependencies can run.
6. Add your own identity in **Agents**, select local execution, scheduled inbox checks, or a supported webhook receiver, and open **Connection & access**. Connect any MCP/HTTP client using its own credential or OAuth consent. Configure the actual heartbeat in that client. See [connection instructions](deploy/AGENT_CONNECTIONS.md).
7. **Link pull request** reads a real existing GitHub PR using the locally signed-in `gh` account. Links do not accept tasks or publish code.

## Commands

Run from this repository. `./workspace` selects the installed Node 24.18.1 and the working Command Line Tools Git without changing global settings.

```sh
./workspace start       # built server and background worker, loopback port 4310
./workspace status      # database, server, worker heartbeat, dispatch state
./workspace stop        # stop app and worker; preserve database and files
./workspace dev         # Vite 4311 plus API/worker; stop built app first
./workspace typecheck
./workspace test
./workspace build
```

Development and built modes share the selected database. Stop one before starting the other. Log files are `.local/server.log`, `.local/worker.log`, and `.local/postgres.log`. Never post the private config, credentials, or database contents publicly.

PostgreSQL uses this app's dedicated native cluster `.local/postgres`, bound to loopback port 55439. Stopping the app leaves PostgreSQL running so the data remains available. To stop the dedicated database after stopping the app:

```sh
/opt/homebrew/bin/pg_ctl -D "$PWD/.local/postgres" -m fast -w stop
```

`./workspace start` starts it again. There is no login item, launch daemon, or public deployment.

## Installation and configuration

This host is already set up. A clean installation requires compatible Node 24, pnpm 11.19.0, native PostgreSQL 18 at `/opt/homebrew/bin`, and Codex CLI **0.148.0** authenticated through its supported login. Optional GitHub reads need `gh auth login`. Install dependencies with `pnpm install --frozen-lockfile`, then run `./workspace setup`, `./workspace build`, and `./workspace start`. The control script currently targets this Mac's native paths; other hosts need a qualified launcher and runtime profile.

Private `.local/config.json` records `port`, `databaseUrl`, `dataDir`, owner-token hash, and work-hook token hash. An alternate complete config can be selected with `WORKSPACE_CONFIG_PATH`. Server overrides include `DATABASE_URL`, `DATA_DIR`, `PORT`, and `CODEX_PATH`; the bundled control commands and frontend development proxy use ports 4310/4311. Keep bytes under the configured data directory, outside public assets. The app serves no public file directory.

The Codex adapter rejects unqualified CLI versions. Upgrade the adapter and rerun its catalog, privacy, tool, interruption, and recovery tests before changing the pinned runtime expectation. It uses the runtime's advertised default model rather than assuming the model available to this coding session is available to the CLI.

## Trigger an approved scope

The UI shows the scope ID after approval. Both the button and webhook use the same checked `session.start` operation.

```sh
./workspace trigger <scope-id> <stable-delivery-id>
```

This reads `.local/work-hook-token` locally and posts to `POST /api/hooks/work` with a bearer credential and `Idempotency-Key`. Do not put the token in a URL. Retries with the same ID and input return the same session. Reusing the ID with different input rejects. A predicted usage reset does not grant capacity or new scope. No webhook is exposed beyond loopback; AgentOS will need a deliberately configured authenticated connection later.

Usage telemetry is queried through Codex when work needs admission, at most once per minute. Missing windows remain unknown. A known exhausted observation continues to hold new work until fresh telemetry confirms recovery. Every admitted run still has finite turn, tool, time, and worker bounds. Idle startup makes no model calls.

## Backup and restore

Back up both the database and bytes together:

```sh
./workspace backup
./workspace start
```

Backup first stops the app and worker, then writes a PostgreSQL custom dump, file copy, and manifest in `.local/backup-<timestamp>`. The backup contains private workspace content in plaintext inside a private directory. It intentionally excludes the owner token and GitHub/Codex credentials. Preserve the local owner-token file separately if recovering on another machine.

```sh
./workspace restore /absolute/path/to/backup-directory
./workspace start
```

Restore creates a **new database and file directory** and saves the previous config. Original data is preserved. It clears browser sessions and scoped runtime credentials, revokes restored agent credentials and OAuth grants, cancels old pending requests, marks uncertain work for reconciliation, and leaves dispatch **disabled**. Sign in again, review Inbox and execution records, confirm old runtimes are stopped, then deliberately enable dispatch in Workspace settings. Restoring never grants permission to replay earlier work.

A real dump/restore of the browser qualification database retained every record count and uploaded-file checksum. The restored built server and worker stayed idle with dispatch off. Evidence is in `evidence/backup-restore.json`; the connected release is verified in `evidence/connected-backup-restore.json`.

Interrupted uploads and unreferenced files older than one hour move to `dataDir/quarantine` on startup; they are not deleted. Missing bytes and checksum mismatches appear in storage health. Preserve damaged data and restore the matching immutable bytes from backup.

## Failure recovery

A database claim is not proof of exactly-once external execution. Each run records preparation, dispatch intent, runtime IDs, interruption receipts, and terminal outcome. If the worker exits without a terminal receipt, restart holds that run for reconciliation. Inspect preserved output/worktree and confirm the old runtime is stopped before resolving it or preparing another attempt. Retrying a task changes its attempt generation and requires a new scope.

No remote push, merge, PR publication, deployment, billing purchase, or reset-credit operation is enabled. Unknown usage remains visible. GitHub links are verified reads taken when linked; webhook synchronization and commit-specific PR checks are future work.

## Current limits

- Local Codex has prior live qualification; generic MCP/HTTP has protocol and persistence qualification. Actual Muse, ChatGPT Work and Hermes/OpenClaw connections still need host/account setup. See the release status for the distinction.
- The qualified execution profile offers bounded repository list/read/write and fixed patch/JSON checks. It cannot run arbitrary shell commands, package installation, or a project's test suite. Repositories with checkout filters are rejected. Review changes and run project tests in your normal environment.
- Groups support bounded discussion, channels, threads and explicit agent handoffs. Ambiguous routing asks for a recipient. Automatic invitations and a model-assisted speaker selector remain deferred. Strict independent opinions require managed agents with isolated context.
- Context contains recent verbatim messages, exact inputs, revision pins, omission counts, and paginated permission-checked older-message retrieval. Generated conversation briefs and dedicated decision/constraint records are not implemented.
- The private v0 store loads records into a transaction for simple consistent checks. It has not been qualified for massive roadmaps or long-term multi-user scale. Search currently filters the loaded workspace in the UI.
- Markdown/pastes use a lightweight safe renderer. Connected agents can download permitted binary files; the managed local text context includes UTF-8 files up to 100 KB. Files upload up to 10 MB; larger required context is rejected visibly.

See `ACCEPTANCE.md` for the actual tested boundary and evidence. `ARCHITECTURE.md` describes module and trust ownership. The original product plan remains in `planning/PRODUCT_PLAN.md`.

Portable Linux deployment and private backup instructions are in [deploy/README.md](deploy/README.md). No public deployment has been activated; the selected host and cloud-client reachability still need qualification.
