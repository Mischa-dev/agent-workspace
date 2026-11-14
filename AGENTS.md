# Repository guidance

The plan is in `docs/plans/master/`. Before you start a slice, read, in this
order: `docs/plans/master/END-STATE.md` (what the product is becoming; its
words are used the same way everywhere), `docs/plans/master/LEAD-RULINGS.md`
(binding where a plan disagrees), your workstream's plan in
`docs/plans/master/plans/` (at least its goal, target design, guard rails and
touch points), and your slice's section in it, which is the specification.
`docs/plans/master/BUILD-ORDER.md` says which build the slice is in, what must
have merged first and how a build runs (its section 3); if a slice it depends
on hasn't merged, stop and say so in the task. Work is tracked as tasks in the
app's AWD project; a task's title and the pull request's title start with the
slice id (`K1.3: …`). The definition of done is section 7 of
`docs/plans/master/plans/P0-steady-ground.md`. If your slice changes something
another plan relies on, record it in `docs/plans/master/interfaces.md`. Older
plans in `docs/plans/` are references for how existing pieces were built;
where they disagree with `docs/plans/master/`, the master plan wins.
First-build documents are in `docs/history/`; don't follow them.

## Skills for working here

Step-by-step guides for the jobs that keep coming up are in
`.claude/skills/<name>/SKILL.md`; read the one that fits before you start:
`add-operation` (anything new the app can do: a write, or a read agents
and the web share), `add-mcp-tool` (changing a hand-written tool on the
app's MCP server), `add-migration` (any
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
runs the full CI on the best machine that's free: the NixOS laptop when it
is on mains power with its GPU quiet, otherwise the Red Hat host. On the laptop
each step also waits its turn behind agents' builds, then checks CPU and memory
before it starts. It checks the working tree as it is, so run it before
pushing; add `--areas rust` or `--areas web` to run one side. Release builds for
a deploy always happen on the Red Hat host, which serves the app. Cap build
memory: a cap limits the job it's on, not the whole machine.

On the NixOS laptop, run finite heavy commands (cargo build, check, clippy
and test; `pnpm test` and `pnpm build`; Playwright runs) as
`~/.local/bin/build-run <command>`. Jobs run one at a time inside T3's capped
agent slice, with one Cargo job unless `CARGO_BUILD_JOBS` is set, and take turns
with laptop CI on `/run/build-admission.lock`, so waiting is expected. Keep them
finite and in the foreground, with no dev server or detached build inside. Run
`tools/ci/run.sh` outside it, since it refuses from inside. If the cap, queue or
lock isn't set up, `build-run` refuses with exit 125: report that rather than
running the build another way, and don't use plain `systemd-run --user
--scope`, which escapes the cap.

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
- New user-facing parts (a sidebar row, a tab, a task field or section, a
  composer extra) register in the layout registry,
  `apps/web/src/experience/registry.json`, and say which presets show them;
  pages check `shows(part)` before rendering them. Layouts are visual only.
  `experience/registry.test.tsx` fails on a sidebar row or tab that isn't
  registered.

## Rules while the foundations are rebuilt

Fourteen workstreams are replacing five patterns that grew by copying. Don't
add to them. Each rule has a guard test (`apps/server/src/structure_guards.rs`,
`apps/web/src/structure.test.ts`) with an allowance per file for today's code.
The allowances only go down: when you remove an offender, lower its number;
never raise one. If you move code between files, move its allowance in the
same pull request. If a rule blocks real work, ask in the task instead of
raising a number.

1. **Access is decided in one place.** Don't write a new role check by name:
   no SQL `role IN ('owner','admin')`, `role = 'admin'` or `role <> 'viewer'`,
   no new `custom_role_allows(…)`, no new `WorkspaceRole` or `.manages()`, and
   in the web no `role === 'owner'`. Ask the policy: `policy::authorize`
   through `work_item_commands::lock_membership` and `authorize_mutation`,
   `roles::standing` or `roles::project_holding`, and on the web the
   permissions the server returns. (K1 replaces these with one `access`
   module; use it once it lands.) No role name compared in Rust
   (`== "owner"`), no hand-built `Subject` or direct `authorize(` call, no
   agent limit applied by hand. Every allowance for these rules names the K1
   slice that removes it in `structure_guards/removals.json`.
2. **No permission depends on the kind of account.** People, agents and apps
   differ only in how they sign in and how they're told about things. Don't
   refuse or allow anything because `actor_kind` is or isn't `Human`. If
   something is for people only, it is a permission marked people-only in
   `policy.rs`, where roles and `my_permissions` can see it. A kind check that
   is purely mechanical (signing in, how a wake-up is sent, a display label)
   carries `// kind-check: mechanical (<why>)` on its line.
3. **Runs are read and written through `runs.rs`.** No new SQL naming
   `agent_runs`, `agent_run_events`, `agent_run_usage` or `agent_approvals`
   outside the files that have it today; add a function to `runs.rs` and call
   it. Use `runs::ACTIVE_RUN_STATES`, never the literal list. (A1 turns
   `runs.rs` into the run service.)
4. **One edge per operation.** A write that both the web and agents can do is
   one domain function, called by the HTTP handler and the MCP tool, which
   only parse and render (`append_message_command_with`,
   `work_item_commands`, `collections::records::write` are the models).
   Specifically: no new `INSERT INTO conversations` (C1 makes one
   thread-creation function; until then add no new path), no new
   `INSERT INTO messages` outside the send pipeline, no new
   `UPDATE work_items` outside `work_item_commands.rs`, and no new caller of
   `mcp::call_tool_direct`. New task view, board and filter features are
   built in the collections view engine, not the task list (until T1 merges
   them).
5. **State lives in PostgreSQL, work runs on the job queue.** No new
   process-local shared state (`static … Mutex`, `LazyLock<Mutex<HashMap…>>`,
   a new map in `AppState`) and no new `tokio::spawn` outside `main.rs`;
   background work is a job in `jobs.rs`. Any server process must be able to
   serve any request. (K5 moves the existing ones.)
6. **Links and events are shared pieces.** Don't create another link table
   (`*_links`, `*_references`); add a kind to `work_item_links` or
   `thing_references` until K3's link store lands. A write that other people,
   agents, webhooks or other tabs should notice enqueues an outbox event in the
   same transaction.
7. **Every conversation read goes through `conversation_visible_to`.** It is
   the only gate (K1 owns its body; K4 and C1 each own one helper inside it;
   never `CREATE OR REPLACE` it or a helper you don't own). Until K4 makes it
   whole, it only says whether the conversation is project-wide or the viewer
   is on its member list, so pair it with a membership check
   (`actor_can_access_project` or the membership join). A query that lists
   conversations or messages (a sidebar, a list, a search, a count) filters
   every row by the rule, even after a project check: being in a project,
   including the hidden direct project or the direct scope, doesn't make its
   DMs and groups yours. No role, admin or staff path reads a DM. A by-id
   lookup after an access check says so in a `-- visibility: <why>` SQL
   comment.
8. **Nobody's stack is hard-coded.** No provider, model, harness, code host,
   machine, path or person by name in core code, tool descriptions or copy,
   outside `provider/`, `ai/providers/` and adapter manifests. Read display
   names from manifests.
9. **Every new write is an operation with an agent tool, a `clientRequestId`
   if it creates something, and an isolation case** in both
   `write_isolation.rs` (another workspace) and `privacy_inside_workspace.rs`
   (a non-member of a private conversation, a narrowed role). Once K2's
   registry lands, follow the `add-operation` skill: the web route, tool,
   public endpoint and automation action are generated, never hand-written.
10. **Content stays in its record.** Events, notifications, job payloads,
    delivery bodies and logs carry ids, kinds and field names, never message,
    comment, page or cell text. Readers fetch the text live, with the
    reader's access.
11. **Every row belongs to a workspace.** A new table has `workspace_id` (and
    `space_id` where it lives in a space), named so even while their foreign
    keys still point at `organizations(id)` and `projects(id)` (ruling R1), or
    it is on the published list of global tables with a reason. It has an
    export decision once H1's declarations exist. Never insert into
    `projects` (or `spaces`) outside the spaces module, and never add a hidden
    kind of project.
12. **Modules are registered, not matched.** Never compare a module id by hand
    (`== "pages"`, `module_off(…, "Pages")`); register a filter. Once M1's
    registry lands, a new module is a manifest, a server folder and a web
    `module.ts`. A user-facing part checks `shows(part)`.
13. **Addresses are built in one place.** Build links to things with the
    builder that exists (`projects/locations.ts` on the web; K3's links module
    and W1's `app/href.ts` replace it), never by pasting `"/projects/…"` or
    `"/w/…"`.
14. **Built for cheap hosting** (ruling R24). Database use is pooler-safe:
    settings only by `SET LOCAL` inside a transaction, no session advisory
    locks (take a lease), no hand `PREPARE`, no transaction held across a
    network call. Nothing runs per workspace: no timer, loop, cache warm-up or
    in-memory map that exists because a workspace exists; schedulers query due
    rows. Limits are declared in one list and counted in shared storage; plan
    checks happen only in metering. `LISTEN`/`NOTIFY` is never the only way to
    learn of an event.
15. **Outbound calls go through `outbound.rs`.** No new HTTP client built
    elsewhere; any address a workspace can set is resolved, checked and pinned
    there.
16. **History is never cut.** No `DELETE FROM` a content or history table
    outside the owning module's delete operation, workspace deletion and the
    listed retention jobs.
17. **Web data comes from the generated client.** No new hand-written API type
    or `requestJson` call in a module that has moved to `callOp`; invalidate
    exactly the things an event names, never a whole project.

Shared test fixtures are in `apps/server/tests/integration/support/`: a test
database from the run's template, one app builder, a world with people,
agents and connected apps, two workspaces, and a `Caller` that runs one
operation as each kind of account. Use them instead of copying another file's
setup.

## Architecture rules

- A new write is an operation in `apps/server/src/<module>/ops.rs`; never
  a hand-written route or tool (the `add-operation` skill).
- Keep one Rust server crate until module boundaries are proven.
- Keep provider-specific JSON inside provider modules.
- Resolve Actor identity server-side; clients cannot choose authorship.
- Conversation access follows rule 7 above: `conversation_visible_to` paired
  with a membership check, or `conversation_readable_by`, its one-call
  wrapper (ruling R32), which adds nothing to it.
- MCP calls are checked against the caller's current memberships every time
  (`actor_can_access_project`, migration 0042). Apps connected by a person act
  as that person; agents act as themselves. See
  `docs/plans/agent-interface.md` for the tools.
- Keep application Threads, Agent Runs, and provider sessions separate.
- Use argument arrays for Git and process execution; never interpolate a shell
  command from user input.
- Never expose secrets to the browser, logs, database records, provider child
  environments, or repository files.
