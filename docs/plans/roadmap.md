# Roadmap: from today to the platform

October 16, 2026. For Mischa, and for the threads that will build it.

This is the plan to take Agent Workspace from what it is today to the
platform you want: a polished workspace that holds a whole project, where
agents of any make do the work in the right place, and where your own
harness ideas plug in. It takes over from the "Phase 4: after cutover" list
in `docs/plans/workspace-merge-plan.md` and folds in `docs/plans/agent-tag.md`.
`docs/plans/agent-interface.md` and `docs/plans/mcp-tools.md` still describe
the MCP surface; the pieces below add to it.

## In one page

**Where it is.** The bones are good: chat with people and agents (DMs,
groups, channels, threads with a status and an outcome), agents that answer on
their own with claims and leases, 71 MCP tools, pages and decisions, a task
model with keys, labels, subtasks, milestones and dependencies, Codex runs on
the host that end in GitHub pull requests, and a design system with five
themes.

**What's in the way.**

- Codex is built into the schema and the code. Database checks pin
  `codex_app_server` at version 0.146.0, there's no adapter interface, and
  runs have no model, reasoning or speed setting.
- There's no settings model and no encrypted secrets store, and you sign in
  to Codex separately in every project.
- Labels, subtasks, milestones and dependencies exist only as MCP tools. The
  web shows a list sorted newest first and a board that stops at 100 cards a
  column, and no project can change its workflow.
- Nothing tracks plan limits over time, nothing notifies people, and nothing
  runs in a cloud.
- Every push to main since October 5 has failed CI.

**How we get there.**
The plan is 100 pieces (23 small, 61 medium, 16 large);
36 of them add a migration.
They're grouped into 13 areas and five phases. Each says what it
needs first and how you'll know it's done, so several threads can build at
once along the tracks in [Order of work](#order-of-work).

**Four moments prove it:**

1. **Launch.** For a project you pick a repository, a Codex Cloud
   environment and defaults (model, reasoning, speed) once. On any task,
   Launch starts a cloud run there, and its progress and pull request land in
   the task's thread. *(Phase 2)*
2. **Capacity.** Your phone says "Codex weekly limit resets in 2 h · 38%
   unused · 6 tasks ready". One tap launches all six, each in its own
   project's environment. *(Phase 3)*
3. **A whole project.** You import a 300-task plan. The AI proposes
   dependencies and you accept most of them from a review queue. The graph
   shows what's ready, and "Ready for agents" is a saved view sorted by what
   unblocks the most. *(Phases 2 and 3)*
4. **Your harness.** You write a runner for a new harness idea with the SDK
   in under 100 lines and register it. It appears in settings and the Launch
   dialog with its own models and options, and takes a task to a pull
   request. *(Phase 3)*

**What it needs from you.** One decision, D1 below (who it's for). And
merging or closing the six open PRs, so Phase 0 can finish. The other
decisions have defaults; change any you disagree with.

## Words used here

- **Workspace**: the organization. All your projects, people, agents, keys,
  provider accounts and the defaults projects inherit.
- **Project**: one product, usually one main repository. It holds its
  repositories, environments and run defaults, and overrides the workspace.
- **Harness**: the program that does agent work. Codex, Claude Code, or one
  of yours.
- **Provider account**: your login with a provider, such as a ChatGPT plan
  for Codex, a Claude plan, or an API key.
- **Environment**: where a run executes. This host, a provider's cloud
  environment, your own runner, or later a container.
- **Run**: one piece of work by a harness (today's Agent Run). **Session**:
  the harness's own conversation, which a thread keeps reusing.
- **Model gateway**: how the server makes short AI calls, such as dependency
  detection, with any provider.

## Principles

These add to AGENTS.md, whose rules all still hold: `conversation_visible_to`
for every conversation query, identity resolved on the server, one server
crate, argument arrays for processes, and no secrets in the browser, logs,
records or child environments.

1. **Every provider is an adapter.** Provider names appear only in adapter
   modules and their manifests, never in table checks, shared code or web
   copy. Settings screens and the Launch dialog are drawn from what an adapter
   declares, so adding a harness needs no web code.
2. **Set it once, override where needed.** A setting resolves from the launch,
   then the project, then the workspace, then the built-in default, and always
   says where its value came from.
3. **Secrets are write-only.** Keys are encrypted at rest, shown only by their
   last four characters, and handed to nothing but the adapter call that needs
   them. Your Anthropic key goes into Workspace settings, under AI and models,
   once S1 and AI1 land. It never goes into the repository, a file or an
   environment variable.
4. **AI suggests; you decide.** The server now makes model calls for short
   jobs through the model gateway. This reverses principle 2 of
   agent-interface.md ("no model calls in the server") because you asked for
   automatic dependency detection. Suggestions carry a reason and their
   evidence, accepted ones are marked "via AI", and auto-apply is a
   per-project switch that starts off. Long work stays with agents.
5. **Parity.** Every feature ships for the web and MCP together, and for the
   public API once X3 exists, as mcp-tools.md already requires.
6. **Built for 10,000 tasks.** Lists page by cursor on every sort, live
   updates patch caches instead of refetching, and nothing loads a whole
   project to answer one question.
7. **Polish is part of every piece.** A piece isn't done while it's missing
   a loading state, an empty state, a useful error, a keyboard path or a phone
   layout.

## Where it stands today

Checked in the code on main at `410e579`.

| Area | Today | The gap |
|---|---|---|
| Conversations and agents | DMs, groups, channels, threads with status and outcome, reply requests with claims and leases, signed webhooks, pages, decisions, search, 71 MCP tools | agent-tag.md pieces 2 to 7; agents' status lines only in DMs and groups; project channels open threads as a full page |
| Tasks | Keys, types, statuses, priority, one assignee, subtasks up to 20 deep, labels, milestones, "blocks" dependencies, criteria, comments, activity, claims, moves between projects, a full-text index | Labels, subtasks, milestones and dependencies are MCP-only; the workflow must match the defaults (`work_items.rs:764`); no dates or estimates; history lacks old values; `task_depend` emits no event |
| Task views | List and board with URL filters | Newest-first only, no grouping, sorting, inline edits, bulk, saved views, graph or timeline; board stops at 100 cards a column |
| Execution | Codex App Server as a child process on the host, git worktrees, approvals, recovery, publishing and PRs through the GitHub App, sponsored runs | Codex pinned by database checks (`0009:162`); no adapter trait; no model, reasoning or speed; one run per account; event payloads empty, so no logs, diffs or cost; no cloud |
| Settings | Project tabs: members, agents, Codex sign-in, repositories by numeric ID, GitHub by installation ID, two automation switches | No settings model, no workspace settings, no secrets store (webhook secrets are plaintext); Codex sign-in per project (`execution.rs:482`) |
| Usage | Codex limits read when the account page opens; only the first window's percentage is shown | Not stored, reset times dropped, no alerts, no tokens or cost |
| Notifications | Unread badges; the MCP briefing for agents | No inbox for people, no push or email; comment mentions never surface; `attention_count` is always 0 (`projects.rs:479`) |
| AI | None in the server, by design; planning through MCP prompts in the client | No gateway, no suggestions |
| Polish | Design system, five themes, solid accessibility baseline, installable app shell, lazy routes | Cmd+K only finds projects; no toasts; Markdown not rendered; one Save bar on tasks; chat skeletons everywhere; files over 1,000 lines |
| Operations | One Red Hat host, daily dumps on the same disk, self-hosted CI | Main CI red; no off-host backups; no metrics; everything in one process |
| Accounts and tenancy | Organizations, roles, invite links, optional WorkOS sign-in, OAuth for MCP apps | No workspace management, self-serve password change or API tokens, email or billing |

## The pieces

Each piece has a size (S is one small PR, M one PR, L two or three PRs), a
phase, what it needs merged first, whether it adds a migration, what to
build, and how to know it's done. File references point at main at
`410e579`; paths without a folder are under `apps/server/src/` or
`apps/web/src/`.

### F. Foundations: make parallel work safe

Many threads will build at once. These pieces stop them colliding and give
them somewhere to test.

#### F1. Land or close the open pull requests

*Size S (one small PR) · Phase 0 · Needs nothing*

Six PRs are open: #125 (agent progress and interim replies, piece A1), #127
(batched GitHub sync writes), #128 (webhook address checks and origin
validation), #112 (run controls respect private threads), #123 (sidebar after
archiving) and #108 (CI script fixes, draft). Bring each up to date with main,
re-run the checks, then merge or close. #128 matters before agents get more
webhooks, and #112 before runs get more controls.

**Done when:** No PR from before this roadmap is still open, and main contains #112 and #128.

#### F2. Make main CI green and keep it green

*Size M (one PR) · Phase 0 · Needs nothing*

Every push to main since October 5 has failed while PR runs pass. A PR runs
only the areas it touches (`tools/ci/changed-areas.sh`), and some main runs
fail within a minute, which looks like no free machine rather than a failing
test. Read the logs on the Red Hat host (cloud sessions can't reach the
Actions log storage), fix the real failures, then make the runner sturdier:
run Rust and web checks one after the other on the laptop under the 8G cap,
reclaim a dead machine lock in minutes instead of 90 (`tools/ci/run.sh:88`),
and run both areas on PRs that touch migrations, the API or shared types.

**Done when:** Five pushes to main in a row are green.

#### F3. Cloud threads can build and test

*Size S (one small PR) · Phase 0 · Needs nothing*

Most pieces will be built by threads running in cloud sessions, which have
Rust 1.97.1, Node 22 and PostgreSQL 16 but no setup. Add a SessionStart hook
(`.claude/settings.json` and a setup script) that starts PostgreSQL, creates a
test database, exports `AGENT_WORKSPACE_DATABASE_URL`, installs web
dependencies with the pinned pnpm, and warms the cargo build. Note in
`AGENTS.md` that one web test (`SessionBoundary`) needs Node 24.

**Done when:** A fresh cloud session runs `cargo test --workspace` and
`corepack pnpm check` with no manual steps.

#### F4. Changes that don't collide

*Size S (one small PR) · Phase 0 · Needs nothing*

Parallel PRs will all want migration 0055, all edit the exact MCP tool count
(`tests/integration/m6_mcp_foundation.rs:293`) and all raise the one total in
the web byte budget (`scripts/web-budget-lib.mjs`). Name new migrations by
timestamp (`20261020143000_settings.sql`), which sqlx orders after 0054; check
`migration_pending`'s version ranges still hold. Replace the exact tool count
with a check that every tool has a description and valid schemas. Budget the
entry chunk and each route chunk instead of the total, so a new lazy route
doesn't touch the budget file.

**Done when:** Two branches that each add a migration, a tool and a route
merge in either order without edits.

#### F5. Web groundwork

*Size M (one PR) · Phase 1 · Needs F4*

Make every query key start with the project, so realtime refreshes pages,
decisions, claims and pickers too (`WorkItemDetailPage.tsx:164`,
`memory/api.ts:73`, `SearchPage.tsx:39`). Fix the `.aw-board` class shared by
the threads drawer and the task board (`threadBoard.tsx:150`, `work.css:240`).
Split `App.test.tsx` (2,005 lines) by area so parallel PRs stop colliding in
it.

**Done when:** Changing a page or a claim in one tab updates another, and no
test file is over 800 lines.

#### F6. UI kit additions

*Size M (one PR) · Phase 1 · Needs F5*

Add the components later pieces need: Toast with Undo, Popover, Combobox
(searchable single and multi select for people, labels and statuses),
DatePicker, a Table with sticky header and keyboard navigation, skeletons
shaped like their page (`MessageSkeleton` is used 51 times for everything), a
CommandMenu base, and one relative-time formatter (four copies exist). A
dev-only `/dev/ui` page shows each component in every theme.

**Done when:** The task assignee picker is one Combobox, and no settings or
list page shows a chat-shaped skeleton.

#### F7. Live updates that patch instead of refetch

*Size M (one PR) · Phase 1 · Needs F5*

Any task change refetches every project query today
(`realtime/useRealtimeConnection.ts:82-100`). Outbox events carry the id,
version and changed fields, and the web patches cached lists, boards and
detail views in place, refetching only when it can't. Needed before projects
hold thousands of tasks.

**Done when:** Moving a card on one screen moves it on another without
refetching the list.

#### F8. Free Codex supervisor slots

*Size S (one small PR) · Phase 0 · Needs nothing*

The supervisor registry holds at most `CODEX_HOST_CONCURRENCY_LIMIT` profiles
(default 1) and releases one only on logout
(`provider/codex_app_server.rs:296`, `account.rs:422`), so opening a second
person's account page can block runs until a restart. Release idle
supervisors, and read account status and limits without taking a run slot
(needed for U1's background polling).

**Done when:** Opening any number of account pages never blocks a run, with a
test.

#### F9. Background jobs

*Size M (one PR) · Phase 1 · Needs F4 · Migration*

One durable job queue in PostgreSQL (`FOR UPDATE SKIP LOCKED`, retries with
backoff, run-at times, unique keys, a dead state) with a small worker pool.
The outbox publisher, reply webhooks and run scheduler stay as they are; new
background work (usage polling, AI jobs, notification fan-out, push delivery,
the launch queue, automations, outgoing webhooks) uses this instead of another
hand-written loop. HTTP calls happen outside row locks, which also settles the
open risk PR #128 noted for webhooks.

**Done when:** A job scheduled for later runs exactly once, survives a restart
and retries on failure, with tests.

#### F10. Docs that match the code

*Size S (one small PR) · Phase 0 · Needs nothing*

This roadmap's PR points `AGENTS.md` here. Still to do: mark
`docs/frontend/current-feature-inventory.md` as historical (it describes
August), fix `docs/local-codex-mcp.md:139` (`run_start` now creates runs), and
note in `workspace-merge-plan.md` that this roadmap carries its Phase 4.

**Done when:** `AGENTS.md`, the plans and the code agree.


### S. Settings, secrets and accounts: set things once

The settings you described (pick the repository, set up the cloud for the
project, choose the default model, reasoning and speed) need three things the
app lacks: somewhere to keep keys safely (S1), settings that inherit (S2), and
provider accounts that belong to you rather than to one project (S6). S3 is
the screen that puts them together.

#### S1. Encrypted secrets

*Size M (one PR) · Phase 1 · Needs F4 · Migration*

A secrets table for anything the server must use later: provider API keys
(your Anthropic key goes here, entered in settings), webhook signing secrets,
push keys. Envelope encryption with AES-256-GCM: a master key file read the
way the GitHub key is (`config.rs:389`), a data key per secret, a key id so
the master key can rotate, and a `secret-key-rotate` command. Secrets are
write-only: the API returns the name, the last four characters, who added it
and when it was last used. Move `external_agent_connections.webhook_secret`
(plaintext since 0038) into it.

**Done when:** A test proves a saved key never appears in a database dump, an
API response, a log line or a run's environment, except the one adapter call
that needs it.

#### S2. Settings that inherit

*Size M (one PR) · Phase 1 · Needs F4 · Migration*

One settings table (scope: workspace, project or user; key; JSON value;
version) and a typed registry in Rust where each key declares its type,
allowed scopes, default and validation. A value resolves from the launch
override, then the project, then the workspace, then the built-in default, and
always reports where it came from. HTTP `GET /settings/effective?projectId=`
and an MCP `settings_get`; changes are people-only, owner or admin.

**Done when:** A project overrides the default model, the UI says it overrides
the workspace, and Reset falls back to the workspace value.

#### S3. One settings area

*Size M (one PR) · Phase 2 · Needs F6, S2*

Replace the project settings tabs with `/settings` and a left nav in three
groups. Account: profile, security, notifications, appearance, provider
accounts, connected apps, API tokens. Workspace: general, members, agents, AI
and models, keys, environments, GitHub, audit log. Project: general, workflow,
labels, repositories, environments and defaults, agents, automation. One field
component shows the inherited value and an override switch. Settings has its
own search.

**Done when:** Every existing setting is reachable from the new nav, and old
links redirect.

#### S4. Manage the workspace

*Size M (one PR) · Phase 3 · Needs S3 · Migration*

Org roles are derived from project roles (`memberships.rs:638`) and nothing
can rename an org. Add rename, a members list for people and agents, org
roles, ownership transfer, invitations to the workspace rather than one
project, and leaving. Fix the Direct space always picking your earliest org
(`direct.rs:956`).

**Done when:** A new person joins the workspace once and can then be added to
any project.

#### S5. Connect GitHub properly

*Size M (one PR) · Phase 1 · Needs nothing · Migration*

Replace the numeric installation and repository ID fields
(`GitHubIntegrationSettings.tsx:273`, `RepositorySettings.tsx:189`) with the
GitHub App install flow: a signed state, the setup callback, user OAuth to
list the installations you can actually reach, and a repository picker that
adds the repo to the project in one step. This also stops anyone who knows an
installation ID from linking it.

**Done when:** Connect a repository in three clicks without copying an ID, and
a test shows another account can't claim your installation.

#### S6. Provider accounts belong to you

*Size L (two or three PRs) · Phase 2 · Needs H1, S1 · Migration*

Execution profiles are per project and per person (`execution.rs:482`), so you
sign in to Codex again in every project and usage can't be added up. Make
provider accounts belong to the user (several allowed, such as personal and
work), keep login state in the account's private home and API keys in S1, and
let each project choose which of your accounts its runs use. Migrate existing
profiles.

**Done when:** Sign in to Codex once and start runs in any project.

#### S7. Account security and API tokens

*Size M (one PR) · Phase 3 · Needs S3 · Migration*

Change your password, see your sessions and sign out elsewhere, create
personal API tokens with scopes and expiry (today only the
`mcp-credential-create` command can), and turn on TOTP two-factor for password
sign-in.

**Done when:** No routine account task needs the server command line.

#### S8. Audit log

*Size S (one small PR) · Phase 3 · Needs S3*

`audit_events` is written everywhere and read nowhere. Add a workspace page
and API with filters for who, which app, what kind and when.

**Done when:** You can see what an agent changed yesterday and through which
app.

#### S9. Appearance follows you

*Size S (one small PR) · Phase 4 · Needs S2*

Theme, density and text size live in the browser (`theme/preferences.ts:49`).
Store them on the account and keep the pre-paint script reading a cached copy.

**Done when:** Your theme is the same on the laptop and the phone.


### H. Harnesses and execution: any harness, any model

Today there is one adapter in all but name: Codex App Server, started per
account as a child process on the host, working in a git worktree, with
approvals and recovery. H1 and H2 make it one adapter among many without
changing what it does; C, U and X all build on that.

#### H1. Open the schema to any provider

*Size M (one PR) · Phase 1 · Needs F4 · Migration*

The database allows only `codex_app_server` on `local_project_host` at binary
version 0.146.0, and those columns can't change (`0009:162`). Replace the
checks with validation in code against the registered adapters, allow version
ranges, rename `codex_home_path` to `home_path`, let each adapter set its
branch prefix (`git_runtime.rs:708` requires `codex/run-`), and take the word
Codex out of shared run messages (`provider.rs:224`, `scheduler.rs:21`,
`api.rs:269`). No behaviour change.

**Done when:** All tests pass, and a second adapter kind can be stored.

#### H2. The harness adapter interface

*Size L (two or three PRs) · Phase 1 · Needs H1*

There is no provider trait: `provider.rs` mixes run bookkeeping with Codex
calls. Define a `HarnessAdapter`: start or resume a session, start and
interrupt a turn, a stream of neutral events (promote the private
`ProviderEvent` set), answer an approval or question, read history for
recovery, account status and login, and usage windows. Codex App Server
becomes the first adapter and the demo executor (`scheduler.rs:418`) the
second, so scheduler and recovery tests run without Codex. Split it: the trait
and Codex move first, the demo adapter and tests second.

**Done when:** Codex runs behave exactly as before, and the demo adapter
passes the scheduler and recovery tests.

#### H3. Capabilities and the model catalog

*Size M (one PR) · Phase 2 · Needs H2, S2 · Migration*

Each adapter publishes a manifest: its models (id, name, context size,
reasoning levels, speed options, prices), its features (streaming, approvals,
resume, cloud, several attempts, runs at once) and a JSON Schema for any extra
options. The catalog refreshes from the provider when it can list models and
can be edited when it can't. Settings and the Launch dialog draw from the
manifest, so a new harness needs no web code.

**Done when:** The default-model picker lists Codex's models from the catalog,
with nothing about Codex in web code.

#### H4. Run settings: model, reasoning, speed

*Size M (one PR) · Phase 2 · Needs H3 · Migration*

Runs have no model or effort today; Codex gets only the thread, folder and
prompt (`provider/codex_app_server.rs:542`). Add `agent_runs.config`, resolved
from the settings cascade and the launch overrides and frozen at start:
harness, model, reasoning level, speed, attempts, sandbox, approval policy and
extra options. The Codex adapter sends model and effort when it starts the
thread and turn, and maps speed to its service tier option. The run card shows
what was used.

**Done when:** A run started on high reasoning says so on its card, and Codex
received it.

#### H5. See what a run did

*Size L (two or three PRs) · Phase 3 · Needs H2 · Migration*

Run events are stored with empty payloads and nothing shows commands, output
or diffs. Store payloads with redaction and size caps, and add a run page: a
step timeline, a log view, the diff of the run's branch against its base,
files changed, tests run, and tokens from the provider's usage events (dropped
today as `ProviderEvent::Ignored`). A Runs page lists every run across your
projects.

**Done when:** You can review a run's work without opening its worktree.

#### H6. Claude Code adapter

*Size L (two or three PRs) · Phase 3 · Needs H2, H4, S1*

Run Claude Code headless in the run's worktree under your own Claude login or
API key from S1, turn its streamed JSON events into neutral events, and send
its permission prompts to the approvals UI. Check the current CLI and Agent
SDK when building.

**Done when:** In a thread, pick Claude Code instead of Codex and get the same
run card, approvals and pull request.

#### H7. Command-line harness adapter

*Size M (one PR) · Phase 3 · Needs H2, H3*

A configured adapter for any harness that runs as a command: an argument
template, environment variables from S1 secrets, the run's worktree as its
folder, and a small JSON-lines protocol on stdout for progress, messages,
approvals and results. The fastest way to try one of your own harnesses on the
host.

**Done when:** A 50-line script harness takes a task to a pull request.

#### H8. Several runs at once

*Size M (one PR) · Phase 3 · Needs H2, S6 · Migration*

A profile runs one turn at a time (`max_active_turns = 1`, `0009:451`) and one
host-wide cap covers everything (`scheduler.rs:109`). Take concurrency per
adapter and per account from the manifest, keep a host cap only for local
runs, and let cloud runs skip it.

**Done when:** Two tasks run at once on one Codex account, and cloud runs
never wait on the host cap.


### C. Environments and launching: one click to the right place

Your Codex example lives here: the project knows its environment and
defaults, and Launch sends work there.

#### C1. Environments

*Size M (one PR) · Phase 2 · Needs H3, S2 · Migration*

An environment says where a run executes: this host (today's local worktrees),
a provider's cloud (a Codex Cloud or Claude Code cloud environment), a
connected runner (your own harness), or later a container you control. Each
project lists its environments with the adapter, the provider's environment
ID, the repository and branch, setup notes and a default. Import them from the
provider where it can list them; otherwise paste the ID and test it.

**Done when:** A project's default environment is 'Codex Cloud ·
agent-workspace', and launching preselects it.

#### C2. Codex Cloud adapter

*Size L (two or three PRs) · Phase 2 · Needs C1, H2, S6*

Start a Codex Cloud task with `codex cloud exec --env <id> [--attempts N]`
under your Codex login, follow it with `codex cloud list --json` (status, URL,
summary), and keep the task as the run's external session with a link. Bring
the result back as a pull request, either opened by Codex or by applying the
diff to a worktree and publishing through the GitHub App. Map its statuses to
run states and recover by task ID after a restart. The CLI changes often, so
check its current commands when building.

**Done when:** Launch on a task starts a Codex Cloud task in the project's
environment, and its status and pull request show in the task's thread.

#### C3. Claude Code cloud adapter

*Size L (two or three PRs) · Phase 4 · Needs C1, H6*

The same as C2 for Claude Code's cloud sessions and environments, or Claude
running in a managed sandbox through the Agent SDK. Check which launch API
exists when building.

**Done when:** Claude Code runs in its cloud environment from the same Launch
button.

#### C4. The Launch button

*Size M (one PR) · Phase 2 · Needs H4, C1*

One click on a task, or several selected tasks, starts a run with the
project's defaults. Shift-click or the menu opens a dialog with harness,
environment, account, model, reasoning, speed and attempts filled in from the
effective settings. The prompt is built from the task (title, description,
criteria, linked pages, project guide) and can be edited in a preview. It
opens or reuses the task's thread. MCP gets `task_launch`.

**Done when:** One click on a task starts a run with the defaults, and the
task shows it running.

#### C5. Launch queue

*Size M (one PR) · Phase 3 · Needs C4, F9 · Migration*

Queue launches to start now, at a set time, when an account's window resets,
or when the tasks blocking them finish. The queue respects per-account
concurrency and dependency order and drains by itself. A queue view shows
what's waiting and why.

**Done when:** Queue ten tasks and they start as capacity frees up, blocked
ones last.

#### C6. Launch prompt templates

*Size S (one small PR) · Phase 3 · Needs C4, S2*

A per-project launch template in Markdown with fields such as
`{{task.title}}`, `{{task.criteria}}` and `{{project.guide}}`, kept as a
versioned setting, plus additions per harness.

**Done when:** Editing the template changes the next launch's prompt preview.

#### C7. Containers you control

*Size L (two or three PRs) · Phase 4 · Needs C1, H7 · Migration*

Run any command-line harness in an isolated container on the host (rootless
Podman, no host files, network only to allowed hosts) or at a sandbox
provider, built from a project setup script. Needed once other people's code
runs on your host or a harness needs its own toolchain. See decision D3.

**Done when:** A custom harness runs in a container that can't read the host's
files or credentials.


### U. Usage, limits and capacity: know what you have and use it

Windows are general: a plan with no five-hour limit just has a weekly
window. Where a provider reports nothing, you set the reset schedule by hand.

#### U1. Usage windows

*Size M (one PR) · Phase 2 · Needs S6, H2, F9, F8 · Migration*

A general model of plan limits: each provider account has windows (a five-hour
session, a week, a month of credits) with percent used, limit, reset time and
when it was read. Adapters report them. Codex already exposes its primary and
secondary windows with reset times through `account/rateLimits/read`
(`provider/codex_app_server.rs:1066`), but the app reads them only when the
account page opens, and that page shows just the first window's percentage.
Poll in the background without taking a run slot (F8), also take limits from
run events, and let accounts whose provider reports nothing set a schedule by
hand ('resets Mondays at 09:00').

**Done when:** The account page shows every window with a live countdown,
including a plan that has only a weekly window.

#### U2. Tokens and cost

*Size M (one PR) · Phase 3 · Needs H3, H5 · Migration*

Record tokens in, out and cached for each run from the adapter's usage events,
price them from the model catalog, and total them per project, account and
day.

**Done when:** Every run and project shows its tokens and estimated cost.

#### U3. Capacity alerts

*Size M (one PR) · Phase 2 · Needs U1, N1*

Rules per account: a window resets within X hours with Y% unused; a window is
above 90%; a limit was hit and resets at a given time; for Codex, banked
resets available or about to expire. On by default and editable.

**Done when:** You get 'Codex weekly limit resets in 2 h · 38% unused' in the
inbox and on your phone.

#### U4. Use it before it resets

*Size M (one PR) · Phase 3 · Needs U3, C5, AI5*

The alert carries a suggested batch: ready tasks across your projects
(dependencies done, criteria written, repository and default environment set),
ranked by priority and by how much they unblock, sized to the capacity left.
One tap launches the batch, each task in its own project's environment with
its defaults; Review opens the batch to edit first.

**Done when:** Tapping Launch on the alert starts the batch in the right
environments.

#### U5. Usage page

*Size M (one PR) · Phase 3 · Needs U1, U2*

For each account: windows over time, runs per window, cost per project, the
next resets, and how much went unused last cycle.

**Done when:** One page answers how much of each plan you use and how much you
waste.

#### U6. Banked resets

*Size S (one small PR) · Phase 4 · Needs U1*

Show Codex's banked rate-limit resets and when they expire, and spend one
after a confirmation, because it can't be undone.

**Done when:** A spent reset shows as a refilled window.


### T. Tasks at project scale: the server side

Much of what a whole project needs already exists behind MCP. T1 and T2
bring it to the web and make it consistent; T6 makes it fast.

#### T1. Web API for planning

*Size M (one PR) · Phase 1 · Needs nothing*

Labels, subtasks, milestones, dependencies, blocked state and claims exist
only as MCP tools (`mcp/planning.rs`, `mcp/labels.rs`). Add HTTP routes that
call the same command functions.

**Done when:** The web client can do everything the task, label and milestone
tools do.

#### T2. Dependencies and milestones everywhere

*Size M (one PR) · Phase 1 · Needs T1*

`task_depend` writes only an audit row, and `milestone_write` writes no
activity or event (`mcp/planning.rs:292`, 592). Write activity, bump versions
and emit events; include blockers, blocked tasks, a ready flag and the
milestone in `task_open`, `task_list` and the HTTP views; make
`project_open`'s next list skip blocked tasks; check cycles with a recursive
query instead of loading every edge in the project.

**Done when:** Blocking a task shows in its activity and updates other screens
live.

#### T3. More kinds of links between tasks

*Size S (one small PR) · Phase 4 · Needs T2 · Migration*

Add relates-to and duplicates (with close as duplicate), and blocks across
projects in the same workspace, checked against what each reader can see.

**Done when:** A duplicate closes with a link to the original.

#### T4. Your own workflow

*Size M (one PR) · Phase 2 · Needs T1*

Every create checks that the project's statuses exactly match the built-in set
(`work_items.rs:764`), so no project can change its workflow. Remove that
check, keep the semantic categories, and let owners add, rename, reorder and
retire statuses and types and edit transitions. Ship `workflow_update` and a
settings page.

**Done when:** A project adds a QA status between Review and Done, and tasks
move through it.

#### T5. Dates, estimates and watchers

*Size M (one PR) · Phase 2 · Needs T1 · Migration*

Add start and due dates, an estimate (points or hours, chosen per project) and
watchers who hear about changes. Keep one assignee plus agent claims.

**Done when:** Tasks sort by due date, and watchers are notified of changes.

#### T6. Fast at 10,000 tasks

*Size M (one PR) · Phase 1 · Needs F4 · Migration*

The list sorts only newest first, search is an unindexed `ILIKE`
(`work_items.rs:1020`), a board column stops at 100 cards, and `task_list`
ranks every open task on each call (`mcp/tasks.rs:565`). Add keyset cursors
for every sort, a trigram index, per-column board paging, a stored board
position, label filters by join, and a seeded 10,000-task fixture to measure
against.

**Done when:** List, board and `task_list` answer in under 200 ms on the
10,000-task fixture.

#### T7. Bulk edits from the web

*Size S (one small PR) · Phase 2 · Needs T1*

Expose `task_update_many`'s all-or-nothing batch over HTTP for status,
assignee, labels, milestone, priority and archive.

**Done when:** Select 40 tasks and move them in one request.

#### T8. Before and after in history

*Size S (one small PR) · Phase 1 · Needs nothing*

Activity records which fields changed but not the old and new title,
description, priority or assignee (`work_item_commands.rs:2216`). Store both,
with a diff for descriptions.

**Done when:** History reads 'Priority High → Urgent'.

#### T9. Import and export

*Size M (one PR) · Phase 3 · Needs T1, T5*

Import a Markdown plan, GitHub issues (through the App) or a CSV from Linear
or Jira with a mapping preview, all through ordinary commands. Export a
project as JSON and CSV.

**Done when:** Import 300 GitHub issues with their labels in one preview and
commit.

#### T10. Templates and recurring tasks

*Size M (one PR) · Phase 4 · Needs T5, T4 · Migration*

Task templates with criteria and subtasks, project templates (workflow,
labels, channels, default settings) and recurring tasks.

**Done when:** A project made from a template arrives with its workflow,
labels and defaults.


### V. Task views: seeing a whole project

`docs/task-platform/product-spec.md` has more ideas for views and imports;
the ones worth building are folded in here.

#### V1. List view

*Size M (one PR) · Phase 2 · Needs F6, T1, T6*

Dense rows; sort; group by status, assignee, priority, label, milestone,
parent or type, with collapsible groups; a filter bar with chips for every
field, including labels, milestone, blocked, ready, claimed and dates; search
that waits for typing to pause and replaces the URL instead of adding a
history entry per keystroke (`WorkItemsPage.tsx:106`); clear filters;
virtualized rows.

**Done when:** Group 2,000 tasks by milestone and scroll smoothly.

#### V2. Keyboard and inline editing

*Size M (one PR) · Phase 2 · Needs V1, T7*

Change status, priority, assignee, labels and milestone straight from a row or
card; select with Shift and Cmd; a bulk action bar; keys to move and act (J
and K, X, S, A, L, P, Enter).

**Done when:** Triage 20 tasks without the mouse.

#### V3. Task panel and detail page

*Size M (one PR) · Phase 2 · Needs T1, T2, P4*

Open any task in a side panel from lists, boards and links. Rebuild the detail
page (1,183 lines) from small parts: each field saves when changed instead of
one Save bar, Markdown description, subtasks, dependencies and milestone,
editable labels, live claim and run status, and the Launch button once C4
lands.

**Done when:** Edit a task from the board without leaving it.

#### V4. Board

*Size M (one PR) · Phase 2 · Needs V1, F7*

Columns by any field, swimlanes, reordering within a column (a drop only
appends today), touch dragging, instant moves, and paging per column.

**Done when:** Reorder cards within a column on a phone.

#### V5. Saved views

*Size M (one PR) · Phase 3 · Needs V1 · Migration*

Named views (filters, sort, grouping, layout, visible fields), personal or
shared with the project, pinned in the sidebar. Ship four: My tasks, Ready for
agents, Blocked, Recently changed. MCP gets `view_list` and `task_list` by
view.

**Done when:** Ready for agents is one click from the sidebar, and agents can
read it.

#### V6. My work

*Size S (one small PR) · Phase 3 · Needs V1*

One personal view across every project: assigned to you, claimed by your
agents, waiting for your review.

**Done when:** Monday morning starts on one page.

#### V7. Dependency graph

*Size L (two or three PRs) · Phase 3 · Needs T2, V3*

An interactive graph for a project or milestone: the ready frontier
highlighted, the critical path, AI-suggested links dashed with accept and
dismiss, a click opens the task panel. Load the layout library only on this
page.

**Done when:** See what blocks a release and accept a suggested dependency
from the graph.

#### V8. Timeline

*Size L (two or three PRs) · Phase 4 · Needs T5, V7*

Milestones and tasks on a time axis from their dates and estimates, with
dependency arrows and drag to reschedule.

**Done when:** Drag a milestone and see what slips.

#### V9. Milestones page

*Size S (one small PR) · Phase 3 · Needs T2, V1*

Milestones with progress, target date and overdue state; each opens as a
filtered view.

**Done when:** Every milestone's progress at a glance.


### AI. AI planning: dependencies and blockers found for you

All of it runs through the model gateway, so any provider works and your key
stays in settings.

#### AI1. Model gateway

*Size L (two or three PRs) · Phase 2 · Needs S1, S2, F9 · Migration*

A small server module for short AI jobs that works with any provider:
Anthropic, OpenAI and any OpenAI-compatible endpoint (OpenRouter, Ollama,
vLLM), and Google. Keys come from S1 at workspace level with a project
override. It asks for structured output against a JSON Schema, validates it
and retries once, logs usage per call, stops at a monthly budget per
workspace, and never logs prompt or message text. Prompts are versioned files
in the repository with test fixtures. Settings pick a provider and model for
quick jobs and for deep ones.

**Done when:** A job runs against a fake provider in tests and against your
Anthropic key from settings.

#### AI2. Dependency and blocker detection

*Size L (two or three PRs) · Phase 2 · Needs AI1, T2 · Migration*

An `ai_suggestions` table (blocks, relates to, duplicates, blocked reason,
parent) with confidence, a one-line reason, the evidence used, status, and the
model and prompt version. It runs when tasks are created or edited (batched),
on Analyze project, and nightly. Cheap signals pick candidates first
(full-text similarity, references between tasks, shared milestone or labels),
then the model judges them in batches. A dismissed pair is never suggested
again. Accepting writes through `task_depend`, shown as via AI. A per-project
switch applies suggestions above a confidence threshold automatically (off by
default). It also reads conversations for 'waiting on X' and suggests the
block. Suggestions only ever cite tasks and messages the reader can see.

**Done when:** On a 200-task project, suggestions arrive in a review queue,
and accepting one updates the graph and the ready list.

#### AI3. Plan to tasks

*Size M (one PR) · Phase 3 · Needs AI1, V3*

Paste a plan or pick a page and get a proposed tree of tasks with criteria,
labels, milestones and dependencies; edit the preview, then commit through
`task_create_many`.

**Done when:** This roadmap imports as tasks, milestones and dependencies in
one go.

#### AI4. Triage on create

*Size M (one PR) · Phase 3 · Needs AI2*

When a task is created, suggest labels, priority, type, parent and likely
duplicates as chips to accept.

**Done when:** A new bug report arrives labelled and linked to its duplicate.

#### AI5. Ready and next

*Size M (one PR) · Phase 2 · Needs T2*

A readiness check per task: dependencies done, criteria written, repository
and environment set, a real description. Rank ready tasks by priority, how
many tasks they unblock, and milestone date. Mostly rules; AI writes missing
criteria on request. U4 and 'what should I launch next?' use it.

**Done when:** Ready for agents is sorted by what unblocks the most.

#### AI6. Summaries

*Size S (one small PR) · Phase 4 · Needs AI1*

On request, summarize a long thread or a task's history and suggest the
thread's outcome.

**Done when:** Catch me up on a 200-message thread gives five lines.

#### AI7. Semantic search

*Size L (two or three PRs) · Phase 4 · Needs AI1 · Migration*

Embeddings through the gateway, stored with pgvector or beside the database,
for search and as another candidate source for AI2. Only once full-text search
clearly falls short.

**Done when:** Searching 'theme colours' finds a message that said 'palette'.


### N. Notifications and the inbox: hear about what matters, anywhere

#### N1. Notifications for people

*Size M (one PR) · Phase 1 · Needs F9 · Migration*

A notifications table per recipient (kind, subject, actor, preview, group key,
read, archived, snoozed), filled from outbox events: @mentions in messages and
task comments (comment mentions are stored but never shown today),
assignments, approvals waiting, runs finished, failed or waiting, failing PR
checks and review requests, and later capacity alerts and AI suggestions.
Repeats collapse into one. `attention_count` stops being hard-coded to 0
(`projects.rs:479`). The MCP briefing includes them.

**Done when:** A comment @mention reaches the person and the project's
attention count.

#### N2. Inbox

*Size M (one PR) · Phase 2 · Needs N1, F6*

An inbox page: notifications grouped by subject, a preview pane, read, unread,
archive and snooze, filters, keyboard, and actions in place (approve, launch,
open). A badge on the rail.

**Done when:** Clear a morning's notifications from the keyboard.

#### N3. Push to phone and desktop

*Size M (one PR) · Phase 2 · Needs N1, S1 · Migration*

Web Push with keys from S1, a subscription per device, push and
notification-click handlers in `sw.js` with actions (Open, Launch), and a test
button per device. Works for the installed app on iOS.

**Done when:** Your phone shows the capacity alert with a Launch button.

#### N4. Notification settings

*Size S (one small PR) · Phase 3 · Needs N1, S2*

Per kind and channel (in app, push, email), quiet hours, mute per project,
digest frequency.

**Done when:** Mute a project's run notifications but keep its mentions.

#### N5. Toasts, titles and badges

*Size S (one small PR) · Phase 1 · Needs F6*

Toasts for results that happen in the background ('Run finished · Open PR'),
page titles with unread counts (`index.html` has one fixed title), and favicon
and app badge counts.

**Done when:** A finished run shows a toast, and the tab title counts unread.


### A. Agents in conversations: tag an agent and it does the work

A1 to A7 are the seven pieces of `docs/plans/agent-tag.md` (decided October
8: `@codex` uses the same protocol as `@claude`). A3 is widened so runs
started by Launch and by a tag share one idea of a session. A8 to A10 are new.

#### A1. Progress and interim messages

*Size S (one small PR) · Phase 0 · Needs nothing*

Piece 1 of `docs/plans/agent-tag.md`, already built as draft PR #125:
`reply_progress`, and `reply_send` with `final: false`.

**Done when:** #125 is merged.

#### A2. Follow-ups stick to the thread

*Size M (one PR) · Phase 1 · Needs A1 · Migration*

`agent-tag.md` piece 2: a person's next message in a thread where an agent was
tagged reaches that agent without a new mention, queued behind a claimed
request rather than replacing it.

**Done when:** 'Also fix the test' reaches the agent already working in the
thread, and never an agent who can't see it.

#### A3. Agent sessions

*Size M (one PR) · Phase 2 · Needs A1, H2 · Migration*

`agent-tag.md` piece 3, widened: one sessions table for external runners and
for harness runs, so a thread knows its Claude Code, Codex Cloud or custom
harness session and links to it. Keep it separate from Agent Runs and provider
sessions, as `AGENTS.md` requires.

**Done when:** A thread header shows 'Codex · Open session' whichever way the
work started.

#### A4. Tags in a channel open a thread

*Size M (one PR) · Phase 2 · Needs A1*

`agent-tag.md` piece 4, plus agents' status lines in project channels and
their threads (today only DMs and groups show them).

**Done when:** Tag @codex in a channel and the answer arrives in a new thread
under the message.

#### A5. Stop

*Size S (one small PR) · Phase 2 · Needs A3 · Migration*

`agent-tag.md` piece 5: a Stop button on a working agent's status line that
the runner hears at once.

**Done when:** Stop ends the agent's work, and its next call is refused with a
clear reason.

#### A6. Typed wakeups

*Size S (one small PR) · Phase 2 · Needs A3*

`agent-tag.md` piece 6: webhooks say what happened (requested, follow-up,
cancelled, launch) and where, still with no message text. `inbox.available`
keeps working for one release.

**Done when:** A runner routes a wakeup without a round trip.

#### A7. Runner contract and presets

*Size M (one PR) · Phase 2 · Needs A6*

`agent-tag.md` piece 7: `docs/agent-runner.md` for the protocol, and presets
on the Agents page for Claude Code, Codex and a custom runner, ending on the
token, webhook secret and settings to copy.

**Done when:** Adding a runner is one form.

#### A8. Tag to launch

*Size M (one PR) · Phase 3 · Needs A3, C4*

Tagging an agent backed by a harness (@codex do X) launches a run with the
project's defaults in that thread; tagging an external runner wakes it. One
rule for people: tag an agent and it does the work.

**Done when:** '@codex fix the flaky test' in a thread starts a Codex Cloud
run there when that's the project default.

#### A9. Agent profiles

*Size M (one PR) · Phase 3 · Needs H3, A3 · Migration*

An agent becomes a profile: name and avatar, harness, default model settings,
instructions, allowed tools and environment. Templates such as Reviewer,
Planner and Fixer. Agent cards show current work from claims and sessions.

**Done when:** Create a Reviewer agent from a template and tag it on a pull
request.

#### A10. Agent presence

*Size S (one small PR) · Phase 4 · Needs A3*

Live state per agent (idle, working on AWD-12, waiting for you) in the sidebar
and on agent cards, replacing the hard-coded online dot
(`ProjectRail.tsx:261`).

**Done when:** You can see which agents are busy without opening anything.


### P. Polish: feels like a product

Every piece carries its own polish (principle 7). These are the cross-cutting
ones.

#### P1. Command palette and shortcuts

*Size M (one PR) · Phase 2 · Needs F6*

Cmd+K only finds projects today (`ProjectRail.tsx:115`). Make it the way to go
anywhere (projects, tasks by key, channels, people, pages, settings) and do
anything (create a task, launch, assign, change status, switch theme), with
actions for the current selection, a `?` shortcut sheet, and global keys (C to
create, G then I for the inbox). Task keys work as URLs (`/AWD-12`).

**Done when:** Everything common is reachable from Cmd+K.

#### P2. Instant changes, undo and clear errors

*Size M (one PR) · Phase 1 · Needs F6*

One way to write mutations: instant cache updates, error toasts instead of
'Reload the page' (`WorkItemDetailPage.tsx:813`), Undo for archive, move and
bulk changes, and busy states. Move ThreadPage's eight direct request calls
onto it.

**Done when:** No screen tells you to reload, and archive can be undone.

#### P3. Home

*Size M (one PR) · Phase 3 · Needs N2, U1*

Home becomes a real start page: what needs you (from the inbox), agents
working now, capacity windows with countdowns, your tasks, recent projects.

**Done when:** Home answers 'what's going on?' in one look.

#### P4. Markdown

*Size M (one PR) · Phase 2 · Needs F6*

Pages say Markdown but render plain text (`pages.css:1571`). Add a safe
renderer (no raw HTML) for descriptions, pages and comments, an editor with
preview and a small toolbar, task keys that turn into links, and code
highlighting loaded only when needed.

**Done when:** A task description with a checklist and code renders properly.

#### P5. Onboarding

*Size M (one PR) · Phase 3 · Needs S3, S5, S6, C1*

A first-run path: name the workspace, connect GitHub and pick a repository,
connect a provider account, choose an environment and defaults, create or
import the first tasks, add an agent. Empty states point to the next step.

**Done when:** A new workspace reaches its first launched run without leaving
the path.

#### P6. Threads beside the channel

*Size M (one PR) · Phase 3 · Needs P2*

Project channels open threads as a full page while DMs use a side panel
(`direct/ThreadPanel.tsx`). Use the panel everywhere with one thread
component.

**Done when:** Open a thread in a project channel without losing your place.

#### P7. Finish each area

*Size M (one PR) · Phase 2 · Needs F6, P2*

One pass per area (conversations, tasks, settings, agents), repeated as areas
land: skeletons shaped like the page, consistent dates, no raw markup outside
the kit (`WorkOsAgentSettings.tsx`), the agent's name instead of a hard-coded
'Codex', phone layouts, and big files split (`ThreadPage.tsx` is 1,128 lines).
The error boundary reports errors instead of dropping them
(`AppErrorBoundary.tsx:13`).

**Done when:** Each area passes a short checklist kept in docs/frontend.

#### P8. Smoke tests in a real browser

*Size M (one PR) · Phase 1 · Needs F3*

A few Playwright flows (sign in, create and move a task, start a run on the
demo adapter, read the inbox) and screenshots of key pages in light and dark,
run in CI's web area. They catch what many parallel PRs break; they are not a
test matrix.

**Done when:** CI fails when sign-in or the task board breaks in a real
browser.


### X. Extensibility: plug in your own harnesses

The point of the platform. A harness can join in three ways, from lightest to
heaviest: as a command the host runs (H7), as a runner on your own machine
that the workspace wakes (A7 and X1), or in a container the workspace starts
(C7). All three register the same manifest (X2).

#### X1. Runner SDKs and the harness guide

*Size M (one PR) · Phase 3 · Needs A7*

TypeScript and Python packages for the runner side: verify the signature,
claim, bind the session, stream progress and events, post interim and final
results, handle Stop. An example harness and `docs/harness-guide.md`.

**Done when:** A new harness written with the SDK in under 100 lines takes a
task to a pull request.

#### X2. Register a harness

*Size M (one PR) · Phase 3 · Needs H3, A7, H7 · Migration*

A workspace registers a custom harness, either a connected runner (X1) or a
command-line adapter (H7), with a manifest of models, options and features. It
then shows in settings, the Launch dialog and agent profiles exactly like
Codex.

**Done when:** Your harness is a choice in the Launch dialog with its own
models and options.

#### X3. Public API

*Size L (two or three PRs) · Phase 4 · Needs S7*

A versioned REST API with an OpenAPI document generated from the Rust types,
personal tokens with scopes (S7), and a docs page.
`docs/task-platform/api-contract.md` is a starting point.

**Done when:** A script with a personal token can do what the web app does.

#### X4. Outgoing webhooks

*Size M (one PR) · Phase 4 · Needs F9 · Migration*

Workspace subscriptions to events (task, run, pull request, message), signed
and retried, with a delivery log and PR #128's address checks.

**Done when:** An external service hears about every finished run.

#### X5. Automations

*Size L (two or three PRs) · Phase 4 · Needs C5, N1, A9, V5 · Migration*

Rules with a trigger (an event, a schedule, a capacity window), conditions (a
saved view's filters) and actions (launch an agent profile, set a field,
notify, post a message). For example: launch when a task labelled agent
becomes Ready; launch a fixer when a PR's checks fail; post a standup every
weekday at 9. A builder in project settings plus MCP tools.

**Done when:** 'When a PR's checks fail, launch Fixer on it' works without
anyone watching.


### O. Operations and SaaS readiness

O4 to O6 wait on decision D1.

#### O1. Backups you can restore

*Size S (one small PR) · Phase 1 · Needs nothing*

Daily dumps stay on the same disk forever and skip the data root
(`agent-workspace-daily-backup.service`). Encrypt them and copy them off the
host, include what can't be rebuilt (provider account homes; the secrets
master key kept separately), prune old dumps, and rehearse a restore with a
script.

**Done when:** A restore drill on a spare database succeeds from the off-host
copy.

#### O2. Metrics and error tracking

*Size M (one PR) · Phase 2 · Needs F9, N1*

A metrics endpoint (requests, job queue, run states, outbox lag, webhook
failures, AI spend), error reporting from the browser and the server, and
alerts to you through notifications.

**Done when:** A failing background job notifies you before you notice it.

#### O3. Retention on a schedule

*Size S (one small PR) · Phase 2 · Needs F9*

Run the existing retention commands as jobs instead of by hand.

**Done when:** Old events and logs are trimmed without anyone remembering to.

#### O4. Sign-up and email

*Size M (one PR) · Phase 4 · Needs S7 · Migration*

Only if D1 says so. Email on accounts, verification, password reset,
invitation emails and digests through an SMTP or provider adapter, and public
sign-up.

**Done when:** A stranger can sign up, verify and reset a password alone.

#### O5. Billing and plans

*Size L (two or three PRs) · Phase 4 · Needs O4, U2 · Migration*

Only if D1 says so. Plans, seats or usage pricing, a payment provider, and
limits enforced per workspace.

**Done when:** A workspace can upgrade and its limits change.

#### O6. More than one server

*Size L (two or three PRs) · Phase 4 · Needs F9*

Only if D1 says so. The realtime hub, rate limits, MCP sessions and background
loops live in one process (`realtime.rs`, `rate_limit.rs`, `mcp.rs:1646`).
Share what must be shared through PostgreSQL LISTEN/NOTIFY or a small broker.

**Done when:** Two server processes behind one address behave like one.


## Order of work

A piece can start as soon as everything it needs has merged, whatever its
phase. The phases say what to aim for together.

**Phase 0: Clear the runway** (7 pieces)

- Foundations: F1 Land or close the open pull requests; F2 Make main CI green
  and keep it green; F3 Cloud threads can build and test; F4 Changes that
  don't collide; F8 Free Codex supervisor slots; F10 Docs that match the code.
- Agents in conversations: A1 Progress and interim messages.

**Phase 1: Foundations, in parallel** (19 pieces)

- Foundations: F5 Web groundwork; F9 Background jobs; F6 UI kit additions; F7
  Live updates that patch instead of refetch.
- Settings, secrets and accounts: S5 Connect GitHub properly; S1 Encrypted
  secrets; S2 Settings that inherit.
- Harnesses and execution: H1 Open the schema to any provider; H2 The harness
  adapter interface.
- Tasks at project scale: T1 Web API for planning; T8 Before and after in
  history; T2 Dependencies and milestones everywhere; T6 Fast at 10,000 tasks.
- Notifications and the inbox: N1 Notifications for people; N5 Toasts, titles
  and badges.
- Agents in conversations: A2 Follow-ups stick to the thread.
- Polish: P8 Smoke tests in a real browser; P2 Instant changes, undo and clear
  errors.
- Operations and SaaS readiness: O1 Backups you can restore.

**Phase 2: The launch loop and real task views** (31 pieces)

- Settings, secrets and accounts: S6 Provider accounts belong to you; S3 One
  settings area.
- Harnesses and execution: H3 Capabilities and the model catalog; H4 Run
  settings: model, reasoning, speed.
- Environments and launching: C1 Environments; C2 Codex Cloud adapter; C4 The
  Launch button.
- Usage, limits and capacity: U1 Usage windows; U3 Capacity alerts.
- Tasks at project scale: T4 Your own workflow; T5 Dates, estimates and
  watchers; T7 Bulk edits from the web.
- Task views: V1 List view; V2 Keyboard and inline editing; V3 Task panel and
  detail page; V4 Board.
- AI planning: AI1 Model gateway; AI5 Ready and next; AI2 Dependency and
  blocker detection.
- Notifications and the inbox: N2 Inbox; N3 Push to phone and desktop.
- Agents in conversations: A4 Tags in a channel open a thread; A3 Agent
  sessions; A5 Stop; A6 Typed wakeups; A7 Runner contract and presets.
- Polish: P1 Command palette and shortcuts; P4 Markdown; P7 Finish each area.
- Operations and SaaS readiness: O3 Retention on a schedule; O2 Metrics and
  error tracking.

**Phase 3: Capacity, planning and your harnesses** (27 pieces)

- Settings, secrets and accounts: S4 Manage the workspace; S7 Account security
  and API tokens; S8 Audit log.
- Harnesses and execution: H5 See what a run did; H8 Several runs at once; H7
  Command-line harness adapter; H6 Claude Code adapter.
- Environments and launching: C5 Launch queue; C6 Launch prompt templates.
- Usage, limits and capacity: U2 Tokens and cost; U5 Usage page; U4 Use it
  before it resets.
- Tasks at project scale: T9 Import and export.
- Task views: V5 Saved views; V6 My work; V9 Milestones page; V7 Dependency
  graph.
- AI planning: AI4 Triage on create; AI3 Plan to tasks.
- Notifications and the inbox: N4 Notification settings.
- Agents in conversations: A9 Agent profiles; A8 Tag to launch.
- Polish: P3 Home; P6 Threads beside the channel; P5 Onboarding.
- Extensibility: X1 Runner SDKs and the harness guide; X2 Register a harness.

**Phase 4: Breadth, and SaaS if you want it** (16 pieces)

- Settings, secrets and accounts: S9 Appearance follows you.
- Environments and launching: C7 Containers you control; C3 Claude Code cloud
  adapter.
- Usage, limits and capacity: U6 Banked resets.
- Tasks at project scale: T3 More kinds of links between tasks; T10 Templates
  and recurring tasks.
- Task views: V8 Timeline.
- AI planning: AI6 Summaries; AI7 Semantic search.
- Agents in conversations: A10 Agent presence.
- Extensibility: X4 Outgoing webhooks; X3 Public API; X5 Automations.
- Operations and SaaS readiness: O6 More than one server; O4 Sign-up and
  email; O5 Billing and plans.


**What each phase ends with:**

- **Phase 0:** main is green, the open PRs are settled, a fresh cloud thread
  can run every test, and parallel PRs don't collide.
- **Phase 1:** the plumbing exists (jobs, secrets, settings, the adapter
  interface, notifications, web access to planning, the UI kit). Visible
  changes are small: toasts, better history, live updates.
- **Phase 2:** moment 1 (Launch to Codex Cloud with your defaults), capacity
  alerts on your phone, the new list, board and task panel, and AI dependency
  suggestions in a review queue.
- **Phase 3:** moments 2, 3 and 4, the Claude Code adapter, run logs and
  diffs, onboarding.
- **Phase 4:** breadth. The SaaS pieces only if D1 says so.

### Tracks

Pieces in one track share files, so one thread per track at a time;
different tracks can run at once. Order within a track follows dependencies,
and P*n* is the phase.

| Track | Pieces, in order |
|---|---|
| Runway and plumbing | F1 (P0), F2 (P0), F3 (P0), F4 (P0), F8 (P0), F10 (P0), F9 (P1), O1 (P1), O3 (P2), O2 (P2) |
| Settings and accounts | S1 (P1), S2 (P1), S5 (P1), S3 (P2), S6 (P2), S4 (P3), S7 (P3), S8 (P3), S9 (P4) |
| Harnesses and launching | H1 (P1), H2 (P1), H3 (P2), H4 (P2), C1 (P2), C2 (P2), C4 (P2), C5 (P3), C6 (P3), H5 (P3), H8 (P3), H6 (P3), H7 (P3), C3 (P4), C7 (P4) |
| Tasks on the server | T1 (P1), T2 (P1), T6 (P1), T8 (P1), T4 (P2), T5 (P2), T7 (P2), T9 (P3), T3 (P4), T10 (P4) |
| Web foundations and polish | F5 (P1), F6 (P1), F7 (P1), P2 (P1), N5 (P1), P8 (P1), P4 (P2), P1 (P2), P7 (P2), P6 (P3), P3 (P3), P5 (P3) |
| Task views | V1 (P2), V3 (P2), V2 (P2), V4 (P2), V5 (P3), V6 (P3), V9 (P3), V7 (P3), V8 (P4) |
| Notifications and usage | N1 (P1), N2 (P2), N3 (P2), U1 (P2), U3 (P2), U2 (P3), U4 (P3), U5 (P3), N4 (P3), U6 (P4) |
| AI planning | AI1 (P2), AI5 (P2), AI2 (P2), AI3 (P3), AI4 (P3), AI6 (P4), AI7 (P4) |
| Agents and extensibility | A1 (P0), A2 (P1), A4 (P2), A3 (P2), A5 (P2), A6 (P2), A7 (P2), A8 (P3), A9 (P3), X1 (P3), X2 (P3), A10 (P4), X3 (P4), X4 (P4), X5 (P4) |
| SaaS (only if D1 says so) | O4 (P4), O5 (P4), O6 (P4) |

### The path to your Codex example

F4, H1, H2, then S1 and S2, S6, H3, H4, C1, C2 and C4 give you Launch. F8,
F9, U1, N1 and U3 give you the alert. C5 and AI5 then give U4, the one-tap
batch. Everything else can run beside it.

## Decisions

**D1. Who it's for.** This is the one only you can make.

- **You, your agents and people you invite** (recommended): SaaS-grade
  polish and clean tenancy, while public sign-up, email, billing, several
  servers and tenant-isolated execution stay parked (O4 to O6, C7).
- **A public SaaS**: those pieces move into phases 2 and 3 and become
  conditions for opening up, and GitHub, execution and secrets get a
  multi-tenant security review first.

Defaults for the rest. Say if any is wrong:

- **D2. AI in the server: yes**, through the gateway, suggestions first
  (principle 4), auto-apply off until you switch it on for a project.
- **D3. Your harnesses first run on your own machines or the host**, through
  the runner protocol (A7, X1) or the command-line adapter (H7). Containers
  the app starts (C7) wait until a harness or another person needs the
  isolation.
- **D4. The project holds the setup.** You asked whether the cloud setup
  belongs to the workspace or the individual project: the project holds its
  repository, environments and run defaults, and inherits anything it doesn't
  set from the workspace.
- **D5. Launch first, views alongside.** The harness and launching track and
  the task views track run in parallel from Phase 1. If only one thread is
  free, the launch track goes first, because your harness work depends on it.
- **D6. No model is special.** Each adapter lists its models, and the
  defaults you set are ordinary settings.
- **D7. Embeddings wait** until full-text search clearly falls short (AI7).

## How a thread picks up a piece

1. Read this file, the piece, and the pieces it needs. Check the tracks
   table, and if another thread is on the same track, coordinate through the
   project chat.
2. Branch `claude/<piece-id>-<short-name>`, and title the draft PR
   `<ID>: <title>`.
3. Until F4 merges, take the next free migration number and expect to
   renumber when rebasing. After F4, name migrations by timestamp.
4. A few files everyone touches: the tool registry in `mcp.rs`, routes in
   `api.rs` and `App.tsx`, `policy_inventory.rs`, the tool-count test and the
   web budget. Put new code in new modules, keep registry edits to a line,
   and merge main before pushing.
5. Ship the web and MCP sides together, with loading, empty, error, keyboard
   and phone states.
6. Test where AGENTS.md says bugs are silent and costly, plus: secrets never
   leak (S1), settings resolve in the right order (S2), every adapter passes
   the same contract tests (H2), and AI suggestions never cite anything the
   reader can't see (AI2).
7. Provider CLIs and APIs (Codex Cloud, Claude Code, rate-limit reads)
   change often. Check the current version when building, and keep their JSON
   inside the adapter.
8. Run `tools/ci/run.sh`, or the cloud setup from F3, before pushing. When
   the PR merges, add its number after the piece's title here.

`roadmap-pieces.json`, next to this file, is a snapshot of every piece with
its phase, size, dependencies and done-when, ready to load into the
workspace with `task_create_many`, `task_depend` and `milestone_write`. That
is AI3's test. This Markdown file is the one to keep up to date.
