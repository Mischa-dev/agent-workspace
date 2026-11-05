# Roadmap: from today to the platform

October 16, 2026. For Mischa, and for the agents that will build it.

This is the plan to take Agent Workspace from what it is today to the
platform you want: a polished workspace that holds a whole project, where
agents of any make do the work in the right place, and where your own
harness ideas plug in. It takes over from the "Phase 4: after cutover" list
in `docs/plans/workspace-merge-plan.md` and folds in `docs/plans/agent-tag.md`.
`docs/plans/agent-interface.md` and `docs/plans/mcp-tools.md` still describe
the MCP surface; the items below add to it.

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
- Every push to main since October 5 has failed CI (the fix is PR #131).

**How we get there.**
The work is 103 checklist items in 16 workstreams: a small runway, 14
workstreams in three waves of six, five and three, and one for opening up
later.
Each workstream is one agent session and one pull request, with a goal, a
checklist, the files it owns and how it meets the others, so all six in wave
1 can run at once. After wave 2 you can use it day to day. See
[Workstreams and waves](#workstreams-and-waves).

**Four moments prove it:**

1. **Launch.** For a project you pick a repository, a Codex Cloud
   environment and defaults (model, reasoning, speed) once. On any task,
   Launch starts a cloud run there, and its progress and pull request land in
   the task's thread. *(Wave 2)*
2. **Capacity.** Your phone says "Codex weekly limit resets in 2 h · 38%
   unused · 6 tasks ready". One tap launches all six, each in its own
   project's environment. *(The alert in wave 2, the one-tap batch in wave
   3)*
3. **A whole project.** You import a 300-task plan. The AI proposes
   dependencies and you accept most of them from a review queue. The graph
   shows what's ready, and "Ready for agents" is a saved view sorted by what
   unblocks the most. *(Waves 1 and 2)*
4. **Your harness.** You write a runner for a new harness idea with the SDK
   in under 100 lines and register it. It appears in settings and the Launch
   dialog with its own models and options, and takes a task to a pull
   request. *(Wave 3)*

**Who it's for.** You decided (D1): private for now, then open source and
a SaaS later. So nothing public gets built yet, but everything is built so
that opening up needs no rewrite (principle 8).

**What it needs from you.** Merging or closing the five older open PRs and
the CI fix in PR #131, saying go for wave 1, and merging each workstream's
pull request when it's green. The decisions have defaults; change any you
disagree with.

## Words used here

- **Workstream**: one agent session's worth of work, built as one pull
  request. It has a goal, a checklist of items, the files it owns, and seams
  where it meets other workstreams.
- **Wave**: the workstreams that run at the same time. A workstream starts as
  soon as the ones it needs have merged.
- **Item**: one entry on a workstream's checklist, with what to build and how
  you'll know it's done. Items are not pull requests.
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
   then the project, then your own setting, then the workspace, then the
   built-in default, and always says where its value came from.
3. **Secrets are write-only.** Keys are encrypted at rest, shown only by their
   last four characters, and handed to nothing but the adapter call that needs
   them. Your Anthropic key goes into Workspace settings, under Runs and models,
   once W1 and W10 land. It never goes into the repository, a file or an
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
7. **Polish is part of every item.** An item isn't done while it's missing
   a loading state, an empty state, a useful error, a keyboard path or a phone
   layout.
8. **Private now, ready to open up.** The app stays private for now, but it
   will become open source (so it must be easy to deploy) and a SaaS (so it
   must hold many workspaces safely). Every workstream follows these rules so
   neither needs a rewrite:
   - Every new table belongs to a workspace, directly or through its project,
     and every query is scoped to it. Nothing that should be per workspace is
     global. The two-workspace isolation tests (O7) grow with each feature.
   - No new state lives only in one process. Background work goes through the
     job queue (F9), so a second server can be added later (O6).
   - Nothing in the code names your machines. Hostnames, paths, addresses and
     the Red Hat setup are configuration.
   - What a SaaS would swap sits behind a small interface: the secrets master
     key (a file now, a key service later), email, file storage, push.
   - Usage is recorded per workspace from the start (U2, AI1), so plans and
     billing can come later without new tracking.
   - Other people's code never runs loose on a host you share: hosted runs go
     to provider clouds or containers (C7) before the SaaS opens.
9. **Access is checked when the work happens.** Anything that acts later on
   someone's behalf (a queued launch, an automation, a webhook delivery) is
   tied to the person, app or agent it acts as. At the moment it acts, it
   checks that actor's current access to the project, conversation, provider
   account and environment involved, not only the access they had when they
   set it up. If the check fails, it stops, and its owner, or a project admin
   if the owner is gone, is told why. Notifications and AI suggestions apply
   the same idea when they're read (N1, AI2).

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
| Operations | One Red Hat host, daily dumps on the same disk, self-hosted CI | Main CI red (PR #131 fixes it); no off-host backups; no metrics; everything in one process |
| Accounts and tenancy | Organizations, roles, invite links, optional WorkOS sign-in, OAuth for MCP apps | No workspace management, self-serve password change or API tokens, email or billing |

## Workstreams and waves

Today's models build something the size of one of these workstreams in one
session, so the plan is a few big pieces of work run side by side rather than
a hundred small pull requests. A wave is the workstreams that run at the same
time. A workstream starts as soon as the ones it needs have merged, without
waiting for the rest of its wave, and every workstream merges after W0.

| Wave | Workstreams, run at once | What you get |
|---|---|---|
| 0 | [W0](#w0-runway) Runway | Green CI on the Red Hat runner, cloud sessions that can run every test, and pull requests that don't collide. Small; starts with wave 1 and merges first. |
| 1 | [W1](#w1-backbone) Backbone, [W2](#w2-harness-platform) Harness platform, [W3](#w3-tasks) Tasks, [W4](#w4-app-shell-and-polish) App shell and polish, [W5](#w5-agents-in-conversations) Agents in conversations, [W6](#w6-notifications) Notifications | The plumbing (jobs, secrets, settings, the harness adapter), task views that hold a whole project, Cmd+K and instant changes, tag-an-agent finished, and an inbox. |
| 2 | [W7](#w7-launch) Launch, [W8](#w8-settings-and-accounts) Settings and accounts, [W9](#w9-capacity-and-push) Capacity and push, [W10](#w10-ai-planning) AI planning, [W11](#w11-views-and-data) Views and data | Launch to Codex Cloud with your defaults, reset alerts on your phone, one settings area, AI dependency suggestions, saved views and import. Usable day to day from here. |
| 3 | [W12](#w12-your-harnesses) Your harnesses, [W13](#w13-payoff-and-finish) Payoff and finish, [W14](#w14-automations-and-api) Automations and API | Claude Code and your own harnesses, run pages with logs and cost, one tap to use capacity before a reset, Home and onboarding, the API and automations. |
| Later | [W15](#w15-open-up) Open up | Self-hosting, the open-source release and the SaaS, when you decide. |

Each workstream below lists its checklist (the items are described in
[The items](#the-items)), the files it owns so parallel sessions don't
collide, its seams with other workstreams, and when it's done.

### Wave 0: Runway

#### W0. Runway

*5 items · Starts now, alongside wave 1*

Clear the way for many agents at once: the older pull requests settled, main
green on the Red Hat runner, cloud sessions that can test, and changes that
don't collide.

- [ ] [F1](#f1-land-or-close-the-open-pull-requests) Land or close the open pull requests
- [ ] [F2](#f2-make-main-ci-green-and-keep-it-green) Make main CI green and keep it green
- [x] [F3](#f3-cloud-sessions-can-build-and-test) Cloud sessions can build and test
- [x] [F4](#f4-changes-that-dont-collide) Changes that don't collide
- [x] [F10](#f10-docs-that-match-the-code) Docs that match the code

**Owns:** The checks F4 changes (`tests/integration/m6_mcp_foundation.rs`,
`scripts/web-budget-lib.mjs`, `migration_preflight.rs`), the cloud setup
script and `.claude/settings.json`, and the docs F10 names. `tools/ci/` and
`.github/workflows/` belong to PR #131 until it merges.

**Seams:**

- F1 needs Mischa: merge or close the five older open pull requests. If #125
  is still open when W5 starts, W5 takes it over.
- F2 is PR #131, from making the Red Hat host the CI runner.
- Wave 1 starts at the same time. Its workstreams name migrations by timestamp
  from day one and merge main once W0 lands.

**Done when:** Main is green on the Red Hat runner, no pull request from
before the roadmap is still open, a fresh cloud session runs every test, and
two branches that each add a migration, a tool and a route merge in either
order.

### Wave 1: Foundations and the first things you'll notice

#### W1. Backbone

*7 items · Starts now · 3 add a migration*

The plumbing everything else uses, and what keeps it safe to run: background
jobs, encrypted secrets, settings that inherit, backups you can restore,
metrics, and the two-workspace isolation tests.

- [x] [F9](#f9-background-jobs) Background jobs
- [x] [S1](#s1-encrypted-secrets) Encrypted secrets
- [x] [S2](#s2-settings-that-inherit) Settings that inherit
- [x] [O1](#o1-backups-you-can-restore) Backups you can restore
- [x] [O2](#o2-metrics-and-error-tracking) Metrics and error tracking
- [x] [O3](#o3-retention-on-a-schedule) Retention on a schedule
- [x] [O7](#o7-two-workspace-isolation-tests) Two-workspace isolation tests

**Owns:** New server modules for jobs, secrets and settings; scheduling in
`retention.rs`; the metrics endpoint; the isolation suite under
`tests/integration/`; the backup scripts in `deploy/redhat/`; and moving the
webhook secret out of `external_agents.rs`. No settings screens: those are
W8's.

**Seams:**

- Post the Rust interfaces (enqueue a job, store and use a secret, resolve a
  setting) in the draft PR on day one, so W2 can code against them before this
  merges.
- O2's alerts go to the log until W6's notifications are on main.
- O1's scripts land in the PR; switching the host's backup service over is a
  deploy step for Mischa.

**Done when:** A project's setting overrides the workspace's and the API says
where each value came from; a saved key never shows up in a database dump, an
API response, a log line or a run's environment; a job scheduled for later
survives a restart and its retries reuse one idempotency key; a restore drill
works from the off-host copy; and the isolation suite covers every existing
list, search and MCP read tool.

#### W2. Harness platform

*7 items · Starts now · 5 add a migration*

Codex becomes one adapter among many, runs carry a model, reasoning level and
speed, and provider accounts belong to people instead of projects.

- [ ] [F8](#f8-free-codex-supervisor-slots) Free Codex supervisor slots
- [ ] [H1](#h1-open-the-schema-to-any-provider) Open the schema to any provider
- [ ] [H2](#h2-the-harness-adapter-interface) The harness adapter interface
- [ ] [S6](#s6-provider-accounts-belong-to-you) Provider accounts belong to you
- [ ] [H3](#h3-capabilities-and-the-model-catalog) Capabilities and the model catalog
- [ ] [H4](#h4-run-settings-model-reasoning-speed) Run settings: model, reasoning, speed
- [ ] [H8](#h8-several-runs-at-once) Several runs at once

**Owns:** `provider.rs` and `provider/`, `scheduler.rs`, `execution.rs`,
`execution_setup.rs`, `profile_runtime.rs`, `recovery.rs`, `runs.rs`,
`account.rs`, the branch prefix in `git_runtime.rs`, the run and execution
profile tables, and `apps/web/src/execution/`.

**Seams:**

- Settings (S2) and secrets (S1) come from W1. Until W1 merges, a run's
  settings come from the request and the adapter's defaults, and an account
  keeps only its login state; whichever merges second wires them together.
- Agent sessions (A3) wait for W7, which builds on this adapter.

**Done when:** Codex runs behave as before, through the adapter; a run started
with a model, reasoning level and speed shows them on its card and Codex
received them; one Codex sign-in works in every project; two runs go at once
on one account; and the demo adapter passes the same contract tests as Codex.

#### W3. Tasks

*13 items · Starts now · 3 add a migration*

A whole project fits in the task list: the planning model complete on the web
and MCP, fast at 10,000 tasks, and a list, board, task panel and milestones
page that make it easy to work through.

- [ ] [T1](#t1-web-api-for-planning) Web API for planning
- [ ] [T2](#t2-dependencies-and-milestones-everywhere) Dependencies and milestones everywhere
- [ ] [T3](#t3-more-kinds-of-links-between-tasks) More kinds of links between tasks
- [ ] [T4](#t4-your-own-workflow) Your own workflow
- [ ] [T5](#t5-dates-estimates-and-watchers) Dates, estimates and watchers
- [ ] [T6](#t6-fast-at-10000-tasks) Fast at 10,000 tasks
- [ ] [T7](#t7-bulk-edits-from-the-web) Bulk edits from the web
- [ ] [T8](#t8-before-and-after-in-history) Before and after in history
- [ ] [V1](#v1-list-view) List view
- [ ] [V2](#v2-keyboard-and-inline-editing) Keyboard and inline editing
- [ ] [V3](#v3-task-panel-and-detail-page) Task panel and detail page
- [ ] [V4](#v4-board) Board
- [ ] [V9](#v9-milestones-page) Milestones page

**Owns:** `work_items.rs`, `work_item_commands.rs`, `work_item_labels.rs`, the
task tools in `mcp/` (planning, labels, tasks, task batches, moves, workflow,
criteria), `apps/web/src/work-items/` and `work.css`, and the pickers V1 adds
to `apps/web/src/ui/` as new files.

**Seams:**

- W4 changes the UI kit and query keys everywhere except
  `apps/web/src/work-items/`, which moves to project-first query keys here.
- Markdown (P4), toasts with undo (F6) and patched live updates (F7) come from
  W4. Until they're on main, descriptions stay plain text and changes refetch;
  whichever merges second switches over.
- The task panel's Launch button comes with W7.
- Watchers (T5) hear about changes through W6's notifications. Whichever of W3
  and W6 merges second adds the watched-task kind to the fan-out.

**Done when:** On the 10,000-task fixture the list, board and `task_list`
answer in under 200 ms; workflow, dates, dependencies, milestones, labels and
bulk edits work from both the web and MCP; and 20 tasks can be triaged from
the keyboard.

#### W4. App shell and polish

*9 items · Starts now*

The app feels like a product: Cmd+K to go anywhere, instant changes with undo
and clear errors, Markdown, threads beside channels, live updates that patch
in place, and smoke tests in a real browser.

- [ ] [F5](#f5-web-groundwork) Web groundwork
- [ ] [F6](#f6-ui-kit-additions) UI kit additions
- [ ] [F7](#f7-live-updates-that-patch-instead-of-refetch) Live updates that patch instead of refetch
- [ ] [P1](#p1-command-palette-and-shortcuts) Command palette and shortcuts
- [ ] [P2](#p2-instant-changes-undo-and-clear-errors) Instant changes, undo and clear errors
- [ ] [P4](#p4-markdown) Markdown
- [ ] [P6](#p6-threads-beside-the-channel) Threads beside the channel
- [ ] [P8](#p8-smoke-tests-in-a-real-browser) Smoke tests in a real browser
- [ ] [N5](#n5-toasts-titles-and-badges) Toasts, titles and badges

**Owns:** `apps/web/src/ui/` apart from V1's pickers, and `app/`, `shell/`,
`realtime/`, `styles/`, `theme/`, `conversations/`, `threads/` and
`direct/ThreadPanel.tsx` under `apps/web/src/`; splitting `App.test.tsx`; the
Playwright setup; and the event payloads in `outbox.rs` that F7 needs.

**Seams:**

- Leave `apps/web/src/work-items/` to W3, which adopts the kit, Markdown and
  patched updates once they're on main.
- W5 adds agent status lines and Stop inside conversations. Move shared
  components without rewriting them, and merge main often.
- P8's flows for a demo run and the inbox follow once W2 and W6 are on main.

**Done when:** Cmd+K reaches every page and common action; no screen says to
reload, and archive can be undone; descriptions, pages and comments render
Markdown; channel threads open beside the channel; a change in one tab shows
in another without a refetch; and CI runs the browser smoke tests.

#### W5. Agents in conversations

*6 items · Starts now · 2 add a migration*

Finish `docs/plans/agent-tag.md`: a tagged agent shows progress, takes
follow-ups in its thread, answers a channel tag in a new thread and stops when
told, and adding a runner is one form.

- [x] [A1](#a1-progress-and-interim-messages) Progress and interim messages
- [x] [A2](#a2-follow-ups-stick-to-the-thread) Follow-ups stick to the thread
- [x] [A4](#a4-tags-in-a-channel-open-a-thread) Tags in a channel open a thread
- [x] [A5](#a5-stop) Stop
- [x] [A6](#a6-typed-wakeups) Typed wakeups
- [x] [A7](#a7-runner-contract-and-presets) Runner contract and presets

**Owns:** `replies.rs`, `external_agents.rs`, `external_events.rs`,
`external_execution.rs`, `mcp/replies.rs`, `mcp/agents.rs`,
`conversations/ReplyStatus.tsx` and the agent status line,
`apps/web/src/external-agents/`, and `docs/agent-runner.md`.

**Seams:**

- Starts from draft PR #125 (A1): merge it first, or branch from it.
- Stop (A5) cancels the claimed request and typed wakeups (A6) say what
  happened; both add the agent's session once W7's A3 lands.
- W1 moves the webhook secret into the secrets store; leave how it's stored to
  W1.

**Done when:** A tagged agent posts progress and interim messages, picks up
'also fix the test' in its thread, answers a channel tag in a new thread, and
stops when someone presses Stop; and adding a runner is one form on the Agents
page.

#### W6. Notifications

*2 items · Starts now · 1 adds a migration*

People hear about what matters: mentions, assignments, approvals and run
results land in an inbox with a badge.

- [ ] [N1](#n1-notifications-for-people) Notifications for people
- [ ] [N2](#n2-inbox) Inbox

**Owns:** A new notifications module and table, the fan-out from outbox
events, any event payloads in `outbox.rs` it needs for approvals, pull request
checks and review requests, `attention_count` in `projects.rs`, notifications
in `mcp/briefing.rs`, and a new `apps/web/src/inbox/`.

**Seams:**

- Notifications are written in the outbox publisher's transaction, so they
  don't wait for W1's job queue.
- W4 also adds payloads to `outbox.rs` (for F7). Add lines next to related
  ones, as F4 says, so either can merge first.
- Uses W4's toasts and skeletons if they're on main, and plain states
  otherwise.
- Changes to watched tasks need W3's watchers (T5). Whichever of W3 and W6
  merges second adds that kind.
- Capacity alerts (W9) and AI suggestions (W10) add their own kinds later.

**Done when:** A comment mention reaches the person and the project's
attention count; mentions, assignments, approvals waiting and finished runs
land in the inbox with a badge on the rail; and a morning's notifications
clear from the keyboard.

### Wave 2: Launch, capacity and AI planning

#### W7. Launch

*7 items · Starts when W1, W2 and W5 have merged · 3 add a migration · Built in #144*

Moment 1: a project knows its environments and run defaults, and one click, or
'@codex do X', starts a run on the host or in Codex Cloud, with its progress
and pull request in the task's thread.

- [x] [C1](#c1-environments) Environments
- [x] [C2](#c2-codex-cloud-adapter) Codex Cloud adapter
- [x] [C4](#c4-the-launch-button) The Launch button
- [x] [C5](#c5-launch-queue) Launch queue
- [x] [C6](#c6-launch-prompt-templates) Launch prompt templates
- [x] [A3](#a3-agent-sessions) Agent sessions
- [x] [A8](#a8-tag-to-launch) Tag to launch

**Owns:** New environments, launch and launch-queue modules, the Codex Cloud
adapter in `provider/`, the agent sessions table (A3), `task_launch`, and the
Launch button and dialog.

**Seams:**

- The Environments page and run defaults are settings pages that W8's settings
  area links to; until W8 is on main they sit in today's project settings.
- 'Start when a window resets' uses W9's usage windows once they're on main.
- W9's capacity alerts open a list of an account's ready tasks. Whichever of
  W7 and W9 merges second adds the Launch button to each one.

**Done when:** One click on a task starts a Codex Cloud task in the project's
default environment with its defaults, and its status and pull request show in
the task's thread; '@codex fix the flaky test' does the same; and ten queued
tasks start as capacity frees up, blocked ones last.

#### W8. Settings and accounts (#141)

*6 items · Starts when W1 and W4 have merged · 3 add a migration*

One settings area for your account, the workspace and each project, showing
what's inherited; GitHub connected in three clicks; members, roles,
invitations, API tokens, two-factor and the audit log.

- [x] [S3](#s3-one-settings-area) One settings area
- [x] [S4](#s4-manage-the-workspace) Manage the workspace
- [x] [S5](#s5-connect-github-properly) Connect GitHub properly
- [x] [S7](#s7-account-security-and-api-tokens) Account security and API tokens
- [x] [S8](#s8-audit-log) Audit log
- [x] [S9](#s9-appearance-follows-you) Appearance follows you

**Owns:** A new `apps/web/src/settings/` that replaces
`projects/ProjectSettingsPage.tsx`, the `github/` and `repositories/` screens,
`memberships.rs`, `invitations.rs`, the GitHub App install flow in
`github.rs`, account security in `auth/`, and the audit log API.

**Seams:**

- Post a small registry for settings pages on day one, so W7 (environments),
  W9 (notifications and alert rules) and W10 (AI and models) each add a line.

**Done when:** Every setting is reachable from `/settings` and old links
redirect; a project setting shows the value it inherits and can override it; a
repository connects in three clicks without copying an ID; a new person joins
the workspace once; and passwords, sessions and API tokens need no command
line.

#### W9. Capacity and push (#147)

*5 items · Starts when W1, W2 and W6 have merged · 2 add a migration*

The first half of moment 2: every plan window tracked in the background with a
countdown, capacity alerts, push to your phone, and notification settings.

- [x] [U1](#u1-usage-windows) Usage windows
- [x] [U3](#u3-capacity-alerts) Capacity alerts
- [x] [U6](#u6-banked-resets) Banked resets
- [x] [N3](#n3-push-to-phone-and-desktop) Push to phone and desktop
- [x] [N4](#n4-notification-settings) Notification settings

**Owns:** New usage and push modules (windows, the polling job, alert rules,
Web Push), the push handlers in `sw.js`, and the usage section of the account
page.

**Seams:**

- The alert's Launch action opens the account's ready tasks across its
  projects. Each task gets W7's Launch button; whichever of W7 and W9 merges
  second adds it. The one-tap batch comes in W13.
- Notification and alert settings go into W8's settings area once it's on
  main.

**Done when:** The account page shows every window with a live countdown,
including a plan with only a weekly window, and your phone shows 'Codex weekly
limit resets in 2 h · 38% unused' with a Launch button.

#### W10. AI planning (#145)

*7 items · Starts when W1 and W3 have merged · 3 add a migration*

Moment 3's brain: the model gateway on your key from settings, suggested
dependencies and blockers in a review queue and on a graph, ready-and-next
ranking, plan to tasks, and triage on create.

- [x] [AI1](#ai1-model-gateway) Model gateway
- [x] [AI2](#ai2-dependency-and-blocker-detection) Dependency and blocker detection
- [x] [AI3](#ai3-plan-to-tasks) Plan to tasks
- [x] [AI4](#ai4-triage-on-create) Triage on create
- [x] [AI5](#ai5-ready-and-next) Ready and next
- [x] [AI6](#ai6-summaries) Summaries
- [x] [V7](#v7-dependency-graph) Dependency graph

**Owns:** A new AI module with its prompts and fixtures, the `ai_suggestions`
table, readiness and ranking, the AI and models settings page, and the
dependency graph page.

**Seams:**

- W13's one-tap batch uses AI5's ranking.
- The AI and models page goes into W8's settings area once it's on main.

**Done when:** On a 200-task project, suggested dependencies arrive in a
review queue and on the graph, and accepting one updates the ready list; this
roadmap's JSON becomes tasks, milestones and dependencies through plan to
tasks; and it all runs against a fake provider in tests and your Anthropic key
from settings.

#### W11. Views and data (#143)

*5 items · Starts when W1 and W3 have merged · 2 add a migration*

The rest of what a whole project needs: saved views, My work, a timeline,
import and export, and templates.

- [x] [V5](#v5-saved-views) Saved views
- [x] [V6](#v6-my-work) My work
- [x] [V8](#v8-timeline) Timeline
- [x] [T9](#t9-import-and-export) Import and export
- [x] [T10](#t10-templates-and-recurring-tasks) Templates and recurring tasks

**Owns:** New views, import and export, and templates modules, the sidebar's
pinned views, and the My work, timeline and import pages.

**Seams:**

- The timeline shows suggested dependencies once W10's graph is on main.

**Done when:** Ready for agents is one click from the sidebar and agents can
read it; Monday starts on My work; dragging a milestone shows what slips; 300
GitHub issues import with their labels; a project made from a template arrives
with its workflow and defaults; and a weekly recurring task appears on
schedule.

### Wave 3: Your harnesses and the payoff

#### W12. Your harnesses (#150)

*9 items · Starts when W1, W2, W5 and W7 have merged · 4 add a migration*

Moment 4: Claude Code and your own harnesses run exactly like Codex, every run
has a page with its log, diff and cost, and agents become profiles you create
from templates.

- [x] [H5](#h5-see-what-a-run-did) See what a run did
- [x] [U2](#u2-tokens-and-cost) Tokens and cost
- [x] [H6](#h6-claude-code-adapter) Claude Code adapter
- [x] [C3](#c3-claude-code-cloud-adapter) Claude Code cloud adapter
- [x] [H7](#h7-command-line-harness-adapter) Command-line harness adapter
- [x] [X1](#x1-runner-sdks-and-the-harness-guide) Runner SDKs and the harness guide
- [x] [X2](#x2-register-a-harness) Register a harness
- [x] [A9](#a9-agent-profiles) Agent profiles
- [x] [A10](#a10-agent-presence) Agent presence

**Owns:** New adapters in `provider/` (Claude Code, its cloud, the command
line), run event payloads and the run page, token and cost records, the runner
SDKs as new TypeScript and Python packages, `docs/harness-guide.md`, harness
registration, and agent profiles.

**Seams:**

- W13's usage page shows cost, and W14's automations launch agent profiles,
  once this is on main.

**Done when:** In a thread you pick Claude Code or your own harness instead of
Codex and get the same run card, approvals and pull request; a 50-line script
harness and a 100-line SDK harness both take a task to a pull request and show
in the Launch dialog with their own models; and every run shows its log, diff,
tokens and cost.

#### W13. Payoff and finish (#149)

*5 items · Starts when W2, W4, W6, W7, W8, W9 and W10 have merged*

Moment 2 in full (one tap launches the batch the alert suggests), the usage
page, a real Home, onboarding, and a polish pass over every area.

- [x] [U4](#u4-use-it-before-it-resets) Use it before it resets
- [x] [U5](#u5-usage-page) Usage page
- [x] [P3](#p3-home) Home
- [x] [P5](#p5-onboarding) Onboarding
- [x] [P7](#p7-finish-each-area) Finish each area

**Owns:** The suggested batch, the usage page, `HomePage.tsx`, the onboarding
path, and the polish checklist in `docs/frontend/`. The polish pass touches
every area, so it comes last and merges main first; pages that land after it
(W12, W14) pass the checklist in their own workstream.

**Seams:**

- Cost per project on the usage page comes from W12 once it's on main.

**Done when:** Tapping Launch on the capacity alert starts the batch, each
task in its own project's environment; Home answers 'what's going on?' in one
look; a new workspace reaches its first launched run without leaving the
onboarding path; and each area passes the polish checklist.

#### W14. Automations and API (#148)

*3 items · Starts when W1, W6, W7, W8 and W11 have merged · 2 add a migration*

Everything scriptable: a versioned public API with tokens, outgoing webhooks,
and automations that launch agents on events and schedules.

- [x] [X3](#x3-public-api) Public API
- [x] [X4](#x4-outgoing-webhooks) Outgoing webhooks
- [x] [X5](#x5-automations) Automations

**Owns:** The public API and its OpenAPI document, outgoing webhooks and their
delivery log, and the automation rules and builder.

**Seams:**

- Automations launch a harness with the project's defaults, and an agent
  profile once W12 is on main.

**Done when:** A script with a personal token can do what the web app does; an
external service hears about every finished run; and 'when a PR's checks fail,
launch an agent on it' works with nobody watching.

### Later: Opening up

#### W15. Open up

*7 items · Starts when W1, W7, W8, W10 and W12 have merged · 4 add a
migration*

When you decide to open it up (D1): self-hosting in one command, the
open-source release, sign-up and email, billing, more than one server,
containers for other people's code, and semantic search if full-text falls
short.

- [ ] [O8](#o8-self-host-in-one-command) Self-host in one command
- [ ] [O9](#o9-open-source-release) Open-source release
- [ ] [O4](#o4-sign-up-and-email) Sign-up and email
- [ ] [O5](#o5-billing-and-plans) Billing and plans
- [ ] [O6](#o6-more-than-one-server) More than one server
- [ ] [C7](#c7-containers-you-control) Containers you control
- [ ] [AI7](#ai7-semantic-search) Semantic search

**Owns:** Decided when it starts. It may well split in two: self-hosting and
the open-source release first, then the SaaS.

**Seams:**

- Principle 8 keeps this short: by then every workstream has scoped its data
  to the workspace, used the job queue and kept machine names in
  configuration.

**Done when:** A stranger can clone the repository, start it with
`docker compose up` and use it, and a hosted workspace can sign up, pay and
run agents in containers.

### The path to your Codex example

W1 and W2 in wave 1 give the settings, secrets and harness adapter. In wave
2, W7 gives Launch to Codex Cloud with your defaults and W9 the reset alert on
your phone. W13 in wave 3 adds the one-tap batch. If only a few sessions are
free at once, start W0, W1 and W2, then W7 and W9.

## The items

Every item belongs to one workstream and says what must be merged before it
(Needs: an item in the same workstream or an earlier wave), what it uses if
it's already on main but must work without (Better with), whether it adds a
migration, what to build, and how to know it's done. File references point at
main at `410e579`; paths without a folder are under `apps/server/src/` or
`apps/web/src/`.

### F. Foundations: make parallel work safe

Many agents will build at once. These items stop them colliding and give
them somewhere to test.

#### F1. Land or close the open pull requests

*In [W0](#w0-runway) Runway · Needs nothing*

Five PRs from before this roadmap are open: #125 (agent progress and interim
replies, item A1), #128 (webhook address checks and origin validation), #112
(run controls respect private threads), #123 (sidebar after archiving) and #108 (CI script fixes, draft). Bring each up to date with main, re-run the
checks, then merge or close. #128 matters before agents get more webhooks, and #112 before runs get more controls.

**Done when:** No PR from before this roadmap is still open, and main contains #112 and #128.

#### F2. Make main CI green and keep it green

*In [W0](#w0-runway) Runway · Needs nothing*

Every push to main since October 5 has failed while PR runs passed. Two
causes, fixed in PR #131: a cancelled run left the machine lock held for 90
minutes while runs give up after 45 (`tools/ci/run.sh:88`), and new rustls
advisories failed cargo-deny. Once it merges, also run both areas on PRs that
touch migrations, the API or shared types (a PR runs only the areas it
touches, `tools/ci/changed-areas.sh`), and run Rust and web checks one after
the other on the laptop under the 8G cap.

**Done when:** Five pushes to main in a row are green, and every PR's CI
starts on the Red Hat runner without anyone touching it.

#### F3. Cloud sessions can build and test

*In [W0](#w0-runway) Runway · Needs nothing*

Workstreams will mostly be built by agents in the cloud: Claude Code cloud
sessions (Rust 1.97.1, Node 22 and PostgreSQL 16, but no setup) and Codex
Cloud tasks. Add one setup script that starts PostgreSQL, creates a test
database, installs web dependencies with the pinned pnpm and warms the cargo
build. Run it from a SessionStart hook (`.claude/settings.json`), and use it
as the Codex Cloud environment's setup script, checking what that image
already has. An export dies with the hook's process, so it saves
`AGENT_WORKSPACE_DATABASE_URL` where later commands see it: appended to
`CLAUDE_ENV_FILE` in a Claude session, and to the shell profile in Codex
Cloud. It installs the Node in `.node-version` (24.18.1) when the image has
another and the download is allowed, and the web checks keep passing on Node
22 for a session where it isn't.

**Done when:** A fresh Claude cloud session and a fresh Codex Cloud task both
run `cargo test --workspace` and `corepack pnpm check` with no manual steps,
and their integration tests reach PostgreSQL instead of skipping.

#### F4. Changes that don't collide

*In [W0](#w0-runway) Runway · Needs nothing*

Parallel PRs will all want migration 0055, all edit the exact MCP tool count
(`tests/integration/m6_mcp_foundation.rs:293`) and all raise the one total in
the web byte budget (`scripts/web-budget-lib.mjs`). Name new migrations by
timestamp (`20261020143000_settings.sql`), which sqlx orders after 0054; check
`migration_pending`'s version ranges still hold. Wave-1 workstreams use
timestamp names from the start, so this only has to make the checks accept
them. Replace the exact tool count with a check that every tool has a
description and valid schemas. Budget the entry chunk and each route chunk
instead of the total, so a new lazy route doesn't touch the budget file.

**Done when:** Two branches that each add a migration, a tool and a route
merge in either order without edits.

#### F5. Web groundwork

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F4*

Make every query key start with the project, so realtime refreshes pages,
decisions, claims and pickers too (`WorkItemDetailPage.tsx:164`,
`memory/api.ts:73`, `SearchPage.tsx:39`). Fix the `.aw-board` class shared by
the threads drawer and the task board (`threadBoard.tsx:150`, `work.css:240`).
Split `App.test.tsx` (2,005 lines) by area so parallel PRs stop colliding in
it.

**Done when:** Changing a page or a claim in one tab updates another, and no
test file is over 800 lines.

#### F6. UI kit additions

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F5*

Add the shared components the rest of the app needs: Toast with Undo, a
CommandMenu base, skeletons shaped like their page (`MessageSkeleton` is used
51 times for everything), and one relative-time formatter (four copies exist).
A `/design-system` page (dev-only `/dev/ui` until the design system pass) shows each component in every theme. The pickers for
the task views (Popover, Combobox, DatePicker, Table) are built with them in
V1.

**Done when:** No settings or list page shows a chat-shaped skeleton, and
dates are formatted in one place.

#### F7. Live updates that patch instead of refetch

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F5*

Any task change refetches every project query today
(`realtime/useRealtimeConnection.ts:82-100`). Outbox events carry the id,
version and changed fields, and the web patches cached lists, boards and
detail views in place, refetching only when it can't. Needed before projects
hold thousands of tasks.

**Done when:** Moving a card on one screen moves it on another without
refetching the list.

#### F8. Free Codex supervisor slots

*In [W2](#w2-harness-platform) Harness platform · Needs nothing*

The supervisor registry holds at most `CODEX_HOST_CONCURRENCY_LIMIT` profiles
(default 1) and releases one only on logout
(`provider/codex_app_server.rs:296`, `account.rs:422`), so opening a second
person's account page can block runs until a restart. Release idle
supervisors, and read account status and limits without taking a run slot
(needed for U1's background polling).

**Done when:** Opening any number of account pages never blocks a run, with a
test.

#### F9. Background jobs

*In [W1](#w1-backbone) Backbone · Needs F4 · Migration*

One durable job queue in PostgreSQL (`FOR UPDATE SKIP LOCKED`, retries with
backoff, run-at times, unique keys, a dead state) with a small worker pool.
The outbox publisher, reply webhooks and run scheduler stay as they are, and
so does notification fan-out, which N1 writes in the outbox publisher's
transaction; new background work (usage polling, AI jobs, push delivery, the
launch queue, automations, outgoing webhooks) uses this instead of another
hand-written loop. HTTP calls happen outside row locks, which also settles the
open risk PR #128 noted for webhooks. Delivery is at least once, because a
worker can crash after a remote call succeeds and before the job is marked
done. So a job is leased to one worker at a time, and every handler with an
outside effect (a push, a launch, a webhook) sends a stable idempotency key
made from the job id, passes it to the remote API where the API takes one, and
checks what it already did before acting again.

**Done when:** A job scheduled for later survives a restart, retries on
failure and is never held by two workers at once, and a handler that crashes
after its outside call sends the same idempotency key when it retries, with
tests.

#### F10. Docs that match the code

*In [W0](#w0-runway) Runway · Needs nothing*

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

*In [W1](#w1-backbone) Backbone · Needs F4 · Migration*

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

*In [W1](#w1-backbone) Backbone · Needs F4 · Migration*

One settings table (scope: workspace, project or user; key; JSON value;
version) and a typed registry in Rust where each key declares its type,
allowed scopes, default and validation. A value resolves from the launch
override, then the project, then the person's own setting, then the workspace,
then the built-in default, and always reports where it came from. HTTP
`GET /settings/effective?projectId=` and an MCP `settings_get`. Changes are
people-only: owners and admins change the workspace's and a project's
settings, and every member changes their own.

**Done when:** A project overrides the default model, the UI says it overrides
the workspace, and Reset falls back to the workspace value; a member who isn't
an admin can change their own settings and nobody else's.

#### S3. One settings area

*In [W8](#w8-settings-and-accounts) Settings and accounts · Needs F6, S2*

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

*In [W8](#w8-settings-and-accounts) Settings and accounts · Needs S3 ·
Migration*

Org roles are derived from project roles (`memberships.rs:638`) and nothing
can rename an org. Add rename, a members list for people and agents, org
roles, ownership transfer, invitations to the workspace rather than one
project, and leaving. Fix the Direct space always picking your earliest org
(`direct.rs:956`).

**Done when:** A new person joins the workspace once and can then be added to
any project.

#### S5. Connect GitHub properly

*In [W8](#w8-settings-and-accounts) Settings and accounts · Needs nothing ·
Migration*

Replace the numeric installation and repository ID fields
(`GitHubIntegrationSettings.tsx:273`, `RepositorySettings.tsx:189`) with the
GitHub App install flow: a signed state, the setup callback, user OAuth to
list the installations you can actually reach, and a repository picker that
adds the repo to the project in one step. This also stops anyone who knows an
installation ID from linking it.

**Done when:** Connect a repository in three clicks without copying an ID, and
a test shows another account can't claim your installation.

#### S6. Provider accounts belong to you

*In [W2](#w2-harness-platform) Harness platform · Needs H1 · Better with S1 ·
Migration*

Execution profiles are per project and per person (`execution.rs:482`), so you
sign in to Codex again in every project and usage can't be added up. Make
provider accounts belong to the user (several allowed, such as personal and
work), keep login state in the account's private home and API keys in S1, and
let each project choose which of your accounts its runs use. Migrate existing
profiles.

**Done when:** Sign in to Codex once and start runs in any project.

#### S7. Account security and API tokens

*In [W8](#w8-settings-and-accounts) Settings and accounts · Needs S3 ·
Migration*

Change your password, see your sessions and sign out elsewhere, create
personal API tokens with scopes and expiry (today only the
`mcp-credential-create` command can), and turn on TOTP two-factor for password
sign-in.

**Done when:** No routine account task needs the server command line.

#### S8. Audit log

*In [W8](#w8-settings-and-accounts) Settings and accounts · Needs S3*

`audit_events` is written everywhere and read nowhere. Add a workspace page
and API with filters for who, which app, what kind and when.

**Done when:** You can see what an agent changed yesterday and through which
app.

#### S9. Appearance follows you

*In [W8](#w8-settings-and-accounts) Settings and accounts · Needs S2*

Theme, density and text size live in the browser (`theme/preferences.ts:49`).
Store them on the account and keep the pre-paint script reading a cached copy.

**Done when:** Your theme is the same on the laptop and the phone.


### H. Harnesses and execution: any harness, any model

Today there is one adapter in all but name: Codex App Server, started per
account as a child process on the host, working in a git worktree, with
approvals and recovery. H1 and H2 make it one adapter among many without
changing what it does; C, U and X all build on that.

#### H1. Open the schema to any provider

*In [W2](#w2-harness-platform) Harness platform · Needs F4 · Migration*

The database allows only `codex_app_server` on `local_project_host` at binary
version 0.146.0, and those columns can't change (`0009:162`). Replace the
checks with validation in code against the registered adapters, allow version
ranges, rename `codex_home_path` to `home_path`, let each adapter set its
branch prefix (`git_runtime.rs:708` requires `codex/run-`), and take the word
Codex out of shared run messages (`provider.rs:224`, `scheduler.rs:21`,
`api.rs:269`). No behaviour change.

**Done when:** All tests pass, and a second adapter kind can be stored.

#### H2. The harness adapter interface

*In [W2](#w2-harness-platform) Harness platform · Needs H1*

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

*In [W2](#w2-harness-platform) Harness platform · Needs H2 · Better with S2 ·
Migration*

Each adapter publishes a manifest: its models (id, name, context size,
reasoning levels, speed options, prices), its features (streaming, approvals,
resume, cloud, several attempts, runs at once) and a JSON Schema for any extra
options. The catalog refreshes from the provider when it can list models and
can be edited when it can't. Settings and the Launch dialog draw from the
manifest, so a new harness needs no web code.

**Done when:** The default-model picker lists Codex's models from the catalog,
with nothing about Codex in web code.

#### H4. Run settings: model, reasoning, speed

*In [W2](#w2-harness-platform) Harness platform · Needs H3 · Better with S2 ·
Migration*

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

*In [W12](#w12-your-harnesses) Your harnesses · Needs H2 · Migration*

Run events are stored with empty payloads and nothing shows commands, output
or diffs. Store payloads with redaction and size caps, and add a run page: a
step timeline, a log view, the diff of the run's branch against its base,
files changed, tests run, and tokens from the provider's usage events (dropped
today as `ProviderEvent::Ignored`). A Runs page lists every run across your
projects.

**Done when:** You can review a run's work without opening its worktree.

#### H6. Claude Code adapter

*In [W12](#w12-your-harnesses) Your harnesses · Needs H2, H4, S1*

Run Claude Code headless in the run's worktree under your own Claude login or
an API key from S1, handed over without putting it in the environment (through
a key helper the CLI calls, if it has one), turn its streamed JSON events into
neutral events, and send its permission prompts to the approvals UI. Check the
current CLI and Agent SDK when building.

**Done when:** In a thread, pick Claude Code instead of Codex and get the same
run card, approvals and pull request.

#### H7. Command-line harness adapter

*In [W12](#w12-your-harnesses) Your harnesses · Needs H2, H3*

A configured adapter for any harness that runs as a command: an argument
template, the S1 secrets it names handed over on an inherited file descriptor
(never as environment variables, which every child process inherits and
diagnostics print), the run's worktree as its folder, and a small JSON-lines
protocol on stdout for progress, messages, approvals and results. The fastest
way to try one of your own harnesses on the host.

**Done when:** A 50-line script harness takes a task to a pull request, and
the secret it was given appears nowhere in its environment.

#### H8. Several runs at once

*In [W2](#w2-harness-platform) Harness platform · Needs H2, S6 · Migration*

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

*In [W7](#w7-launch) Launch · Needs H3, S2 · Migration*

An environment says where a run executes: this host (today's local worktrees),
a provider's cloud (a Codex Cloud or Claude Code cloud environment), a
connected runner (your own harness), or later a container you control. Each
project lists its environments with the adapter, the provider's environment
ID, the repository and branch, setup notes and a default. Import them from the
provider where it can list them; otherwise paste the ID and test it.

**Done when:** A project's default environment is 'Codex Cloud ·
agent-workspace', and launching preselects it.

#### C2. Codex Cloud adapter

*In [W7](#w7-launch) Launch · Needs C1, H2, S6*

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

*In [W12](#w12-your-harnesses) Your harnesses · Needs C1, H6*

The same as C2 for Claude Code's cloud sessions and environments, or Claude
running in a managed sandbox through the Agent SDK. Check which launch API
exists when building.

**Done when:** Claude Code runs in its cloud environment from the same Launch
button.

#### C4. The Launch button

*In [W7](#w7-launch) Launch · Needs H4, C1*

One click on a task, or several selected tasks, starts a run with the
project's defaults. Shift-click or the menu opens a dialog with harness,
environment, account, model, reasoning, speed and attempts filled in from the
effective settings. The prompt is built from the task (title, description,
criteria, linked pages, project guide) and can be edited in a preview. It
opens or reuses the task's thread. MCP gets `task_launch`.

**Done when:** One click on a task starts a run with the defaults, and the
task shows it running.

#### C5. Launch queue

*In [W7](#w7-launch) Launch · Needs C4, F9 · Better with U1 · Migration*

Queue launches to start now, at a set time, when an account's window resets,
or when the tasks blocking them finish. The queue respects per-account
concurrency and dependency order and drains by itself. A queue view shows
what's waiting and why. When a queued launch is about to start, it checks
again that the person who queued it can still reach the project
(`actor_can_access_project`), the provider account and the environment; one
that fails is cancelled, and the person is told why.

**Done when:** Queue ten tasks and they start as capacity frees up, blocked
ones last; a launch queued by someone removed from the project never starts.

#### C6. Launch prompt templates

*In [W7](#w7-launch) Launch · Needs C4, S2*

A per-project launch template in Markdown with fields such as
`{{task.title}}`, `{{task.criteria}}` and `{{project.guide}}`, kept as a
versioned setting, plus additions per harness.

**Done when:** Editing the template changes the next launch's prompt preview.

#### C7. Containers you control

*In [W15](#w15-open-up) Open up · Needs C1, H7 · Migration*

Run any command-line harness in an isolated container on the host (rootless
Podman, no host files, network only to allowed hosts) or at a sandbox
provider, built from a project setup script. Needed once other people's code
runs on your host or a harness needs its own toolchain. Required before the
SaaS runs anyone's code on your servers (D1); until then, see D3.

**Done when:** A custom harness runs in a container that can't read the host's
files or credentials.


### U. Usage, limits and capacity: know what you have and use it

Windows are general: a plan with no five-hour limit just has a weekly
window. Where a provider reports nothing, you set the reset schedule by hand.

#### U1. Usage windows

*In [W9](#w9-capacity-and-push) Capacity and push · Needs S6, H2, F9, F8 ·
Migration*

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

*In [W12](#w12-your-harnesses) Your harnesses · Needs H3, H5 · Migration*

Record tokens in, out and cached for each run from the adapter's usage events,
price them from the model catalog, and total them per project, account and
day.

**Done when:** Every run and project shows its tokens and estimated cost.

#### U3. Capacity alerts

*In [W9](#w9-capacity-and-push) Capacity and push · Needs U1, N1*

Rules per account: a window resets within X hours with Y% unused; a window is
above 90%; a limit was hit and resets at a given time; for Codex, banked
resets available or about to expire. On by default and editable.

**Done when:** You get 'Codex weekly limit resets in 2 h · 38% unused' in the
inbox and on your phone.

#### U4. Use it before it resets

*In [W13](#w13-payoff-and-finish) Payoff and finish · Needs U3, C5, AI5*

The alert carries a suggested batch: ready tasks across your projects
(dependencies done, criteria written, repository and default environment set)
whose project can run on the alert's account: a harness that account serves
and an environment it can reach. They're ranked by priority and by how much
they unblock, and sized to that account's capacity left. One tap launches the
batch, each task in its own project's environment with its defaults but on the
alert's account; Review opens the batch to edit first.

**Done when:** Tapping Launch on the alert starts the batch in the right
environments, every run on the account whose window is about to reset.

#### U5. Usage page

*In [W13](#w13-payoff-and-finish) Payoff and finish · Needs U1 · Better with
U2*

For each account: windows over time, runs per window, cost per project, the
next resets, and how much went unused last cycle.

**Done when:** One page answers how much of each plan you use and how much you
waste.

#### U6. Banked resets

*In [W9](#w9-capacity-and-push) Capacity and push · Needs U1*

Show Codex's banked rate-limit resets and when they expire, and spend one
after a confirmation, because it can't be undone.

**Done when:** A spent reset shows as a refilled window.


### T. Tasks at project scale: the server side

Much of what a whole project needs already exists behind MCP. T1 and T2
bring it to the web and make it consistent; T6 makes it fast.

#### T1. Web API for planning

*In [W3](#w3-tasks) Tasks · Needs nothing*

Labels, subtasks, milestones, dependencies, blocked state and claims exist
only as MCP tools (`mcp/planning.rs`, `mcp/labels.rs`). Add HTTP routes that
call the same command functions.

**Done when:** The web client can do everything the task, label and milestone
tools do.

#### T2. Dependencies and milestones everywhere

*In [W3](#w3-tasks) Tasks · Needs T1*

`task_depend` writes only an audit row, and `milestone_write` writes no
activity or event (`mcp/planning.rs:292`, 592). Write activity, bump versions
and emit events; include blockers, blocked tasks, a ready flag and the
milestone in `task_open`, `task_list` and the HTTP views; make
`project_open`'s next list skip blocked tasks; check cycles with a recursive
query instead of loading every edge in the project.

**Done when:** Blocking a task shows in its activity and updates other screens
live.

#### T3. More kinds of links between tasks

*In [W3](#w3-tasks) Tasks · Needs T2 · Migration*

Add relates-to and duplicates (with close as duplicate), and blocks across
projects in the same workspace, checked against what each reader can see.

**Done when:** A duplicate closes with a link to the original.

#### T4. Your own workflow

*In [W3](#w3-tasks) Tasks · Needs T1*

Every create checks that the project's statuses exactly match the built-in set
(`work_items.rs:764`), so no project can change its workflow. Remove that
check, keep the semantic categories, and let owners add, rename, reorder and
retire statuses and types and edit transitions. Ship `workflow_update` and a
settings page.

**Done when:** A project adds a QA status between Review and Done, and tasks
move through it.

#### T5. Dates, estimates and watchers

*In [W3](#w3-tasks) Tasks · Needs T1 · Better with N1 · Migration*

Add start and due dates, an estimate (points or hours, chosen per project) and
watchers who hear about changes through N1's notifications. Keep one assignee
plus agent claims.

**Done when:** Tasks sort by due date, and watchers are notified of changes.

#### T6. Fast at 10,000 tasks

*In [W3](#w3-tasks) Tasks · Needs F4 · Migration*

The list sorts only newest first, search is an unindexed `ILIKE`
(`work_items.rs:1020`), a board column stops at 100 cards, and `task_list`
ranks every open task on each call (`mcp/tasks.rs:565`). Add keyset cursors
for every sort, a trigram index, per-column board paging, a stored board
position, label filters by join, and a seeded 10,000-task fixture to measure
against.

**Done when:** List, board and `task_list` answer in under 200 ms on the
10,000-task fixture.

#### T7. Bulk edits from the web

*In [W3](#w3-tasks) Tasks · Needs T1*

Expose `task_update_many`'s all-or-nothing batch over HTTP for status,
assignee, labels, milestone, priority and archive.

**Done when:** Select 40 tasks and move them in one request.

#### T8. Before and after in history

*In [W3](#w3-tasks) Tasks · Needs nothing*

Activity records which fields changed but not the old and new title,
description, priority or assignee (`work_item_commands.rs:2216`). Store both,
with a diff for descriptions.

**Done when:** History reads 'Priority High → Urgent'.

#### T9. Import and export

*In [W11](#w11-views-and-data) Views and data · Needs T1, T5*

Import a Markdown plan, GitHub issues (through the App) or a CSV from Linear
or Jira with a mapping preview, all through ordinary commands. Export a
project as JSON and CSV.

**Done when:** Import 300 GitHub issues with their labels in one preview and
commit.

#### T10. Templates and recurring tasks

*In [W11](#w11-views-and-data) Views and data · Needs T5, T4, F9 · Migration*

Task templates with criteria and subtasks, project templates (workflow,
labels, channels, default settings) and recurring tasks, which F9's job queue
creates on schedule.

**Done when:** A project made from a template arrives with its workflow,
labels and defaults, and a weekly recurring task appears each week, even
across a restart.


### V. Task views: seeing a whole project

`docs/task-platform/product-spec.md` has more ideas for views and imports;
the ones worth building are folded in here.

#### V1. List view

*In [W3](#w3-tasks) Tasks · Needs T1, T6 · Better with F6*

Add Popover, Combobox (searchable single and multi select for people, labels
and statuses), DatePicker and a Table with a sticky header and keyboard
navigation to the UI kit, as new files. Then the list: dense rows; sort; group
by status, assignee, priority, label, milestone, parent or type, with
collapsible groups; a filter bar with chips for every field, including labels,
milestone, blocked, ready, claimed and dates; search that waits for typing to
pause and replaces the URL instead of adding a history entry per keystroke
(`WorkItemsPage.tsx:106`); clear filters; virtualized rows.

**Done when:** Group 2,000 tasks by milestone and scroll smoothly.

#### V2. Keyboard and inline editing

*In [W3](#w3-tasks) Tasks · Needs V1, T7*

Change status, priority, assignee, labels and milestone straight from a row or
card; select with Shift and Cmd; a bulk action bar; keys to move and act (J
and K, X, S, A, L, P, Enter).

**Done when:** Triage 20 tasks without the mouse.

#### V3. Task panel and detail page

*In [W3](#w3-tasks) Tasks · Needs T1, T2 · Better with P4*

Open any task in a side panel from lists, boards and links. Rebuild the detail
page (1,183 lines) from small parts: each field saves when changed instead of
one Save bar, Markdown description, subtasks, dependencies and milestone,
editable labels, live claim and run status, and the Launch button once C4
lands.

**Done when:** Edit a task from the board without leaving it.

#### V4. Board

*In [W3](#w3-tasks) Tasks · Needs V1 · Better with F7*

Columns by any field, swimlanes, reordering within a column (a drop only
appends today), touch dragging, instant moves, and paging per column.

**Done when:** Reorder cards within a column on a phone.

#### V5. Saved views

*In [W11](#w11-views-and-data) Views and data · Needs V1 · Migration*

Named views (filters, sort, grouping, layout, visible fields), personal or
shared with the project, pinned in the sidebar. Ship four: My tasks, Ready for
agents, Blocked, Recently changed. MCP gets `view_list` and `task_list` by
view.

**Done when:** Ready for agents is one click from the sidebar, and agents can
read it.

#### V6. My work

*In [W11](#w11-views-and-data) Views and data · Needs V1*

One personal view across every project: assigned to you, claimed by your
agents, waiting for your review.

**Done when:** Monday morning starts on one page.

#### V7. Dependency graph

*In [W10](#w10-ai-planning) AI planning · Needs T2, V3*

An interactive graph for a project or milestone: the ready frontier
highlighted, the critical path, AI-suggested links dashed with accept and
dismiss, a click opens the task panel. Load the layout library only on this
page.

**Done when:** See what blocks a release and accept a suggested dependency
from the graph.

#### V8. Timeline

*In [W11](#w11-views-and-data) Views and data · Needs T5 · Better with V7*

Milestones and tasks on a time axis from their dates and estimates, with
dependency arrows and drag to reschedule.

**Done when:** Drag a milestone and see what slips.

#### V9. Milestones page

*In [W3](#w3-tasks) Tasks · Needs T2, V1*

Milestones with progress, target date and overdue state; each opens as a
filtered view.

**Done when:** Every milestone's progress at a glance.


### AI. AI planning: dependencies and blockers found for you

All of it runs through the model gateway, so any provider works and your key
stays in settings.

#### AI1. Model gateway

*In [W10](#w10-ai-planning) AI planning · Needs S1, S2, F9 · Migration*

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

*In [W10](#w10-ai-planning) AI planning · Needs AI1, T2 · Migration*

An `ai_suggestions` table (blocks, relates to, duplicates, blocked reason,
parent) with confidence, a one-line reason, the evidence used, status, and the
model and prompt version. It runs when tasks are created or edited (batched),
on Analyze project, and nightly. Cheap signals pick candidates first
(full-text similarity, references between tasks, shared milestone or labels),
then the model judges them in batches. A dismissed pair is never suggested
again. Accepting writes through `task_depend`, shown as via AI. A per-project
switch applies suggestions above a confidence threshold automatically (off by
default). It also reads conversations for 'waiting on X' and suggests the
block. Runs with no person behind them (on edit, nightly, auto-apply) read
only tasks and conversations every project member can see, never DMs, private
groups or private threads. A person's own Analyze may use what they can see,
but its suggestions stay visible to that person only. Each suggestion keeps
its sources, and `conversation_visible_to` is checked again whenever one is
shown or accepted.

**Done when:** On a 200-task project, suggestions arrive in a review queue,
and accepting one updates the graph and the ready list; nobody outside a
private thread ever sees a suggestion drawn from it.

#### AI3. Plan to tasks

*In [W10](#w10-ai-planning) AI planning · Needs AI1, V3 · Migration*

Paste a plan or pick a page and get a proposed tree of tasks with criteria,
labels, milestones and dependencies; edit the preview, then commit through
`task_create_many`. A plan bigger than one call (100 tasks) commits in
batches, and links between batches are added once every task exists. The
import keeps its own record of each batch and the tasks it made, since the
tool only replays a request for 24 hours, so resuming an import at any time
skips the batches that finished.

**Done when:** This roadmap's 103 items import as tasks, milestones and
dependencies in one go, and resuming a half-finished import a day later
creates nothing twice.

#### AI4. Triage on create

*In [W10](#w10-ai-planning) AI planning · Needs AI2*

When a task is created, suggest labels, priority, type, parent and likely
duplicates as chips to accept.

**Done when:** A new bug report arrives labelled and linked to its duplicate.

#### AI5. Ready and next

*In [W10](#w10-ai-planning) AI planning · Needs T2*

A readiness check per task: dependencies done, criteria written, repository
and environment set, a real description. Rank ready tasks by priority, how
many tasks they unblock, and milestone date. Mostly rules; AI writes missing
criteria on request. U4 and 'what should I launch next?' use it.

**Done when:** Ready for agents is sorted by what unblocks the most.

#### AI6. Summaries

*In [W10](#w10-ai-planning) AI planning · Needs AI1*

On request, summarize a long thread or a task's history and suggest the
thread's outcome.

**Done when:** Catch me up on a 200-message thread gives five lines.

#### AI7. Semantic search

*In [W15](#w15-open-up) Open up · Needs AI1 · Migration*

Embeddings through the gateway, stored with pgvector or beside the database,
for search and as another candidate source for AI2. Only once full-text search
clearly falls short.

**Done when:** Searching 'theme colours' finds a message that said 'palette'.


### N. Notifications and the inbox: hear about what matters, anywhere

#### N1. Notifications for people

*In [W6](#w6-notifications) Notifications · Needs nothing · Better with F9 ·
Migration*

A notifications table per recipient (kind, subject, actor, source conversation
and message, group key, read, archived, snoozed), filled from outbox events:
@mentions in messages and task comments (comment mentions are stored but never
shown today), assignments, approvals waiting, runs finished, failed or
waiting, failing PR checks and review requests, changes to tasks you watch
(T5), and later capacity alerts and AI suggestions. Repeats collapse into one.
`attention_count` stops being hard-coded to 0 (`projects.rs:479`). The MCP
briefing includes them. Rows hold ids, not message text: the inbox, push and
the briefing build each title and preview from its source when they read it,
checking the reader's current access each time (`actor_can_access_project` for
the project, `conversation_visible_to` for a conversation,
`pull_request_visible_to` for a pull request), and hide a notification whose
source the reader can no longer see. Notifications are written in the outbox
publisher's transaction, so they need no job queue; fan-out moves to a job if
volume calls for it.

**Done when:** A comment @mention reaches the person and the project's
attention count, and someone removed from a private group or a project stops
seeing its notifications.

#### N2. Inbox

*In [W6](#w6-notifications) Notifications · Needs N1 · Better with F6*

An inbox page: notifications grouped by subject, a preview pane, read, unread,
archive and snooze, filters, keyboard, and actions in place (approve, launch,
open). A badge on the rail.

**Done when:** Clear a morning's notifications from the keyboard.

#### N3. Push to phone and desktop

*In [W9](#w9-capacity-and-push) Capacity and push · Needs N1, S1 · Better with
C4 · Migration*

Web Push with keys from S1, a subscription per device, push and
notification-click handlers in `sw.js` with actions (Open, and Launch), and a
test button per device. A capacity alert belongs to an account that may serve
several projects, so its Launch opens that account on the account page with
the ready tasks (open, unblocked, repository set) in every project it can run,
each with C4's Launch button once that's on main. Works for the installed app
on iOS.

**Done when:** Your phone shows the capacity alert with a Launch button.

#### N4. Notification settings

*In [W9](#w9-capacity-and-push) Capacity and push · Needs N1, S2*

Per kind and channel (in app, push, email), quiet hours, mute per project,
digest frequency.

**Done when:** Mute a project's run notifications but keep its mentions.

#### N5. Toasts, titles and badges

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F6*

Toasts for results that happen in the background ('Run finished · Open PR'),
page titles with unread counts (`index.html` has one fixed title), and favicon
and app badge counts.

**Done when:** A finished run shows a toast, and the tab title counts unread.


### A. Agents in conversations: tag an agent and it does the work

A1 to A7 are the seven pieces of `docs/plans/agent-tag.md` (decided October
8: `@codex` uses the same protocol as `@claude`). A3 is widened so runs
started by Launch and by a tag share one idea of a session. A8 to A10 are new.

#### A1. Progress and interim messages

*In [W5](#w5-agents-in-conversations) Agents in conversations · Needs nothing*

Piece 1 of `docs/plans/agent-tag.md`, already built as draft PR #125:
`reply_progress`, and `reply_send` with `final: false`.

**Done when:** #125 is merged.

#### A2. Follow-ups stick to the thread

*In [W5](#w5-agents-in-conversations) Agents in conversations · Needs A1 ·
Migration*

`agent-tag.md` piece 2: a person's next message in a thread where an agent was
tagged reaches that agent without a new mention, queued behind a claimed
request rather than replacing it.

**Done when:** 'Also fix the test' reaches the agent already working in the
thread, and never an agent who can't see it.

#### A3. Agent sessions

*In [W7](#w7-launch) Launch · Needs A1, H2 · Migration*

`agent-tag.md` piece 3, widened: one sessions table for external runners and
for harness runs, so a thread knows its Claude Code, Codex Cloud or custom
harness session and links to it. Keep it separate from Agent Runs and provider
sessions, as `AGENTS.md` requires.

**Done when:** A thread header shows 'Codex · Open session' whichever way the
work started.

#### A4. Tags in a channel open a thread

*In [W5](#w5-agents-in-conversations) Agents in conversations · Needs A1*

`agent-tag.md` piece 4, plus agents' status lines in project channels and
their threads (today only DMs and groups show them).

**Done when:** Tag @codex in a channel and the answer arrives in a new thread
under the message.

#### A5. Stop

*In [W5](#w5-agents-in-conversations) Agents in conversations · Needs A1 ·
Migration*

`agent-tag.md` piece 5: a Stop button on a working agent's status line that
the runner hears at once. It cancels the claimed request, and also the agent's
session once A3 lands.

**Done when:** Stop ends the agent's work, and its next call is refused with a
clear reason.

#### A6. Typed wakeups

*In [W5](#w5-agents-in-conversations) Agents in conversations · Needs A1 ·
Better with A3*

`agent-tag.md` piece 6: webhooks say what happened (requested, follow-up,
cancelled, launch) and where, still with no message text, and name the agent's
session once A3 lands. `inbox.available` keeps working for one release.

**Done when:** A runner routes a wakeup without a round trip.

#### A7. Runner contract and presets

*In [W5](#w5-agents-in-conversations) Agents in conversations · Needs A6*

`agent-tag.md` piece 7: `docs/agent-runner.md` for the protocol, and presets
on the Agents page for Claude Code, Codex and a custom runner, ending on the
token, webhook secret and settings to copy.

**Done when:** Adding a runner is one form.

#### A8. Tag to launch

*In [W7](#w7-launch) Launch · Needs A3, C4*

Tagging an agent backed by a harness (@codex do X) launches a run with the
project's defaults in that thread; tagging an external runner wakes it. One
rule for people: tag an agent and it does the work.

**Done when:** '@codex fix the flaky test' in a thread starts a Codex Cloud
run there when that's the project default.

#### A9. Agent profiles

*In [W12](#w12-your-harnesses) Your harnesses · Needs H3, A3 · Migration*

An agent becomes a profile: name and avatar, harness, default model settings,
instructions, allowed tools and environment. Templates such as Reviewer,
Planner and Fixer. Agent cards show current work from claims and sessions.

**Done when:** Create a Reviewer agent from a template and tag it on a pull
request.

#### A10. Agent presence

*In [W12](#w12-your-harnesses) Your harnesses · Needs A3*

Live state per agent (idle, working on AWD-12, waiting for you) in the sidebar
and on agent cards, replacing the hard-coded online dot
(`ProjectRail.tsx:261`).

**Done when:** You can see which agents are busy without opening anything.


### P. Polish: feels like a product

Every item carries its own polish (principle 7). These are the cross-cutting
ones.

#### P1. Command palette and shortcuts

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F6*

Cmd+K only finds projects today (`ProjectRail.tsx:115`). Make it the way to go
anywhere (projects, tasks by key, channels, people, pages, settings) and do
anything (create a task, launch, assign, change status, switch theme), with
actions for the current selection, a `?` shortcut sheet, and global keys (C to
create, G then I for the inbox). Task keys work as URLs (`/AWD-12`).

**Done when:** Everything common is reachable from Cmd+K.

#### P2. Instant changes, undo and clear errors

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F6*

One way to write mutations: instant cache updates, error toasts instead of
'Reload the page' (`WorkItemDetailPage.tsx:813`), Undo for archive, move and
bulk changes, and busy states. Move ThreadPage's eight direct request calls
onto it.

**Done when:** No screen tells you to reload, and archive can be undone.

#### P3. Home

*In [W13](#w13-payoff-and-finish) Payoff and finish · Needs N2, U1*

Home becomes a real start page: what needs you (from the inbox), agents
working now, capacity windows with countdowns, your tasks, recent projects.

**Done when:** Home answers 'what's going on?' in one look.

#### P4. Markdown

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F6*

Pages say Markdown but render plain text (`pages.css:1571`). Add a safe
renderer (no raw HTML) for descriptions, pages and comments, an editor with
preview and a small toolbar, task keys that turn into links, and code
highlighting loaded only when needed.

**Done when:** A task description with a checklist and code renders properly.

#### P5. Onboarding

*In [W13](#w13-payoff-and-finish) Payoff and finish · Needs S3, S5, S6, C1*

A first-run path: name the workspace, connect GitHub and pick a repository,
connect a provider account, choose an environment and defaults, create or
import the first tasks, add an agent. Empty states point to the next step.

**Done when:** A new workspace reaches its first launched run without leaving
the path.

#### P6. Threads beside the channel

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs P2*

Project channels open threads as a full page while DMs use a side panel
(`direct/ThreadPanel.tsx`). Use the panel everywhere with one thread
component.

**Done when:** Open a thread in a project channel without losing your place.

#### P7. Finish each area

*In [W13](#w13-payoff-and-finish) Payoff and finish · Needs F6, P2*

One pass per area (conversations, tasks, settings, agents), repeated as areas
land: skeletons shaped like the page, consistent dates, no raw markup outside
the kit (`WorkOsAgentSettings.tsx`), the agent's name instead of a hard-coded
'Codex', phone layouts, and big files split (`ThreadPage.tsx` is 1,128 lines).
The error boundary reports errors instead of dropping them
(`AppErrorBoundary.tsx:13`). Workstreams that merge after this one run the
same checklist on their own pages before they're marked ready.

**Done when:** Each area passes a short checklist kept in docs/frontend.

#### P8. Smoke tests in a real browser

*In [W4](#w4-app-shell-and-polish) App shell and polish · Needs F3 · Better
with H2, N2*

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

*In [W12](#w12-your-harnesses) Your harnesses · Needs A7*

TypeScript and Python packages for the runner side: verify the signature,
claim, bind the session, stream progress and events, post interim and final
results, handle Stop. An example harness and `docs/harness-guide.md`.

**Done when:** A new harness written with the SDK in under 100 lines takes a
task to a pull request.

#### X2. Register a harness

*In [W12](#w12-your-harnesses) Your harnesses · Needs H3, A7, H7 · Migration*

A workspace registers a custom harness, either a connected runner (X1) or a
command-line adapter (H7), with a manifest of models, options and features. It
then shows in settings, the Launch dialog and agent profiles exactly like
Codex.

**Done when:** Your harness is a choice in the Launch dialog with its own
models and options.

#### X3. Public API

*In [W14](#w14-automations-and-api) Automations and API · Needs S7*

A versioned REST API with an OpenAPI document generated from the Rust types,
personal tokens with scopes (S7), and a docs page.
`docs/task-platform/api-contract.md` is a starting point.

**Done when:** A script with a personal token can do what the web app does.

#### X4. Outgoing webhooks

*In [W14](#w14-automations-and-api) Automations and API · Needs F9 ·
Migration*

Subscriptions to a workspace's events (task, run, pull request, message), each
owned by the person or connected app that made it and delivering only what
that owner can see now: access is checked again on every delivery, and a
message event goes through `conversation_visible_to` for the owner. Signed and
retried, with a delivery log and PR #128's address checks. Each delivery
carries one id across its retries, so a receiver can drop a repeat.

**Done when:** An external service hears about every finished run, and never
about a message in a conversation its subscription's owner can't see.

#### X5. Automations

*In [W14](#w14-automations-and-api) Automations and API · Needs C5, N1, V5 ·
Better with A9 · Migration*

Rules with a trigger (an event, a schedule, a capacity window), conditions (a
saved view's filters) and actions (launch an agent with the project's run
defaults, or an agent profile once A9 is on main; set a field; notify; post a
message). For example: launch when a task labelled agent becomes Ready; launch
an agent on a PR whose checks fail; post a standup every weekday at 9. A
builder in project settings plus MCP tools. Each rule acts as the person or
app that made it, and before every action it checks that actor's current
access to the project and to any account, environment or conversation the
action uses (principle 9). A rule that fails the check is turned off, and its
owner, or a project admin if the owner has left, is told why.

**Done when:** 'When a PR's checks fail, launch an agent on it' works without
anyone watching, and a rule whose maker left the project stops instead of
acting for them.


### O. Operations, open source and SaaS

O1, O2, O3 and O7 are in W1, because backups, alerts and leaks between
workspaces are cheap to get right early and expensive later. O4 to O6, O8
and O9 are for when you open it up (D1); principle 8 keeps that path short.

#### O1. Backups you can restore

*In [W1](#w1-backbone) Backbone · Needs nothing*

Daily dumps stay on the same disk forever and skip the data root
(`agent-workspace-daily-backup.service`). Encrypt them and copy them off the
host, include what can't be rebuilt (provider account homes; the secrets
master key kept separately), prune old dumps, and rehearse a restore with a
script.

**Done when:** A restore drill on a spare database succeeds from the off-host
copy.

#### O2. Metrics and error tracking

*In [W1](#w1-backbone) Backbone · Needs F9 · Better with N1*

A metrics endpoint (requests, job queue, run states, outbox lag, webhook
failures, AI spend), error reporting from the browser and the server, and
alerts to you through notifications (in the log until N1 lands).

**Done when:** A failing background job notifies you before you notice it.

#### O3. Retention on a schedule

*In [W1](#w1-backbone) Backbone · Needs F9*

Run the existing retention commands as jobs instead of by hand.

**Done when:** Old events and logs are trimmed without anyone remembering to.

#### O4. Sign-up and email

*In [W15](#w15-open-up) Open up · Needs S7 · Migration*

For the SaaS later (D1). Email on accounts, verification, password reset,
invitation emails and digests through an SMTP or provider adapter, and public
sign-up, switched off by default for self-hosted installs.

**Done when:** A stranger can sign up, verify and reset a password alone.

#### O5. Billing and plans

*In [W15](#w15-open-up) Open up · Needs O4, U2 · Migration*

For the SaaS later (D1). Plans, seats or usage pricing, a payment provider,
and limits enforced per workspace, built on the usage that U2 and AI1 already
record per workspace. Absent from self-hosted installs.

**Done when:** A workspace can upgrade and its limits change.

#### O6. More than one server

*In [W15](#w15-open-up) Open up · Needs F9*

For the SaaS later (D1). The realtime hub, rate limits, MCP sessions and
background loops live in one process (`realtime.rs`, `rate_limit.rs`,
`mcp.rs:1646`). Share what must be shared through PostgreSQL LISTEN/NOTIFY or
a small broker.

**Done when:** Two server processes behind one address behave like one.

#### O7. Two-workspace isolation tests

*In [W1](#w1-backbone) Backbone · Needs nothing*

The SaaS will hold many workspaces in one database, so leaks between them must
be caught from now on, not audited later. A test fixture builds two workspaces
with look-alike people, agents, projects, tasks, DMs and pages, and a suite
checks that every list, search, task view, MCP tool and realtime event in one
never shows the other's. Each new feature adds its cases (see 'How an agent
runs a workstream').

**Done when:** The suite covers every existing list and search route and MCP
read tool, and fails if one leaks across workspaces.

Writes have their own net on the same fixture (`write_isolation.rs`): every
POST, PUT, PATCH and DELETE route and every MCP tool that writes is called as
one workspace with the other's IDs, must be refused, and must leave every row
that mentions the other workspace as it was. Its guard fails when a new write
has no case.

#### O8. Self-host in one command

*In [W15](#w15-open-up) Open up · Needs S1, S3*

For the open-source release (D1). There's no container image today, and the
first admin comes from an interactive command (`bootstrap-admin`). Add a
container image and a compose file (the app plus PostgreSQL) that start a
working install, first-run setup in the browser that creates the first admin
and generates the secrets master key, a configuration reference with safe
defaults, and upgrade notes. An empty database isn't enough to open setup,
since anyone who reaches a fresh install first would own it. On first start
the server writes a one-time setup code to a file in its data folder that only
the host can read, and logs the file's path but never the code (`AGENTS.md`
keeps secrets out of logs). Setup asks for the code, which stops working once
the first admin exists. The Red Hat scripts stay in `deploy/redhat` as one
example of a host.

**Done when:** On a fresh machine, `docker compose up` and a browser get you
to a working workspace, and setup refuses anyone without the code from the
host.

#### O9. Open-source release

*In [W15](#w15-open-up) Open up · Needs O8*

Choose a license (D8), move what's specific to your machines out of the code
(hostnames and Tailscale addresses in `tools/ci/run.sh` and `deploy/redhat`,
paths under `/home/bot`) into configuration or a private deploy repository,
check the git history for anything private, and add a README for strangers,
CONTRIBUTING, a security policy and public docs.

**Done when:** A stranger can clone the repository, follow the README and run
it, and nothing in it points at your machines.


## Decisions

**D1. Who it's for** (decided October 16). Private for now: you, your
agents and people you invite. Later it becomes both open source, so it must
be easy to deploy (O8, O9), and a SaaS, so it must not need a rewrite to hold
many workspaces (principle 8, O4 to O7, C7).

**D8. License** (later, before O9). For example, AGPL-3.0 keeps anyone who
runs a modified copy as a service sharing their changes, which protects the
SaaS; Apache-2.0 or MIT makes adoption easiest. Nothing needs it until the
release.

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
- **D5. Big workstreams, run in parallel.** Each workstream is one agent
  session and one pull request, sized for what a capable model builds in one
  go, with up to six running at once. If fewer sessions are free, the launch
  path goes first (W1 and W2, then W7 and W9), because your harness work
  builds on it.
- **D6. No model is special.** Each adapter lists its models, and the
  defaults you set are ordinary settings.
- **D7. Embeddings wait** until full-text search clearly falls short (AI7).

## How an agent runs a workstream

1. Read AGENTS.md, then in this file the workstream's entry and every item on
   its checklist, plus anything they need from other workstreams.
2. Work on one branch and one pull request titled `W<n>: <name>` (in a
   Claude session, the branch is `claude/w<n>-<short-name>`). Open it as a
   draft early and push as you go. It's meant to be large.
3. Stay inside what the workstream owns. In the registries everyone touches
   (the tool list in `mcp.rs`, routes in `api.rs` and `App.tsx`,
   `policy_inventory.rs`, `ui/index.ts`, the settings page registry) add lines
   without reordering, and merge main often.
4. Name migrations by timestamp, such as `20261020143000_settings.sql`.
5. At a seam, build against what's merged on main. Where an item is better
   with something that isn't there yet, leave a path that works without it;
   whichever side merges second wires the two together before it's marked
   ready.
6. Ship the web and MCP sides together, with loading, empty, error, keyboard
   and phone states. Once P7's polish checklist is in `docs/frontend/`, pass
   it for your pages.
7. Test where AGENTS.md says bugs are silent and costly, plus: secrets never
   leak (S1), settings resolve in the right order (S2), every adapter passes
   the same contract tests (H2), AI suggestions never cite anything the
   reader can't see (AI2), anything that acts later stops for someone who
   lost access (principle 9), and the workstream's lists, writes and tools
   are in the two-workspace isolation tests (O7).
8. Follow principle 8: new tables belong to a workspace, background work
   uses the job queue, and nothing in the code names a machine.
9. Provider CLIs and APIs (Codex Cloud, Claude Code, rate-limit reads)
   change often. Check the current version when building, and keep their JSON
   inside the adapter.
10. CI queues on the Red Hat runner when several workstreams push at once, so
    run the fast checks in your own session as you go, and the full checks
    (`tools/ci/run.sh`, or the cloud setup from F3) before marking the pull
    request ready. In the same pull request, tick the workstream's checklist
    and add the pull request's number after its heading.

### Starting a workstream

Paste this into a new Claude thread or a Codex Cloud task, with the number
and name filled in:

```text
You're building workstream W<n> (<name>) of the Agent Workspace roadmap in
Mischa-dev/agent-workspace. Read AGENTS.md and docs/plans/roadmap.md: the
entry for W<n> under "Workstreams and waves", and every item on its
checklist. Build all of it as one pull request titled "W<n>: <name>", on one
branch. Open it as a draft early and push as you go. Stay inside the files
the workstream owns; in the shared registries (MCP tool list, routes, policy
inventory, UI kit index, settings pages) only add lines. Where the entry
lists a seam, build against what's merged on main and leave the rest working
without it. Follow "How an agent runs a workstream" in the roadmap, run the
full checks before marking the pull request ready, and tick the checklist in
the same pull request.
```

`roadmap.json`, next to this file, holds every workstream and item with its
dependencies and done-when, ready to load into the workspace as milestones and
tasks with `task_create_many`, `task_depend` and `milestone_write`. That is
AI3's test. This Markdown file is the one to keep up to date.
