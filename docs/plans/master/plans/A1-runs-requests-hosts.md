# A1. Runs, requests and hosts

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner's plan for workstream A1 of the Agent Workspace master plan. Read-only research
against `main` at fe95a74. Every `path:line` is from that tree. "Inferred" marks a claim not
read directly in code.

Paths: `src/` means `apps/server/src/`, `migrations/` means `apps/server/migrations/`,
`web/` means `apps/web/src/`.

---

## 1. Goal

Every agent's work shows up the same way, whoever runs it. A bot someone runs on a laptop, an
Agent OS agent, a cloud coding service and today's in-server coding agents all get a run
with state, steps, cost, what it is waiting on and what it produced, on one Runs page, and all
of them can ask a person (or a role) for an approval or an answer on an inbox card. Workspace
keeps the record; hosts do the work through one public protocol (enrol, describe, start,
cancel, steer, answer, plus reporting back), and the built-in runner becomes a separate
program that uses only that protocol. Pull requests are recorded through a code-host seam,
with GitHub as its first implementation and none of its features lost.

---

## 2. Where the code is today

### 2.1 A server-run agent's run, traced

1. **Start.** Four entry points create a run:
   - `api::start_run_in` (`src/api.rs:223-513`), used by the browser, MCP `run_start`
     (`src/mcp.rs:3123`), automations and Launch (`src/launch.rs:2078`).
   - `api::retry_run_in` (`src/api.rs:535`, its own `INSERT INTO agent_runs` at
     `src/api.rs:684`).
   - `external_execution::launch_sponsored_run` (`src/external_execution.rs:832`), when a
     person approves an agent's `run_request`.
   - `work_items::work_with_worker` (D report F4: `work_items.rs:1431`, which calls the HTTP
     handler directly; its web hook has no callers).

   `start_run_in` checks the conversation is a thread (`src/api.rs:249`), reserves an
   idempotency key, applies the MCP-only cap (`src/api.rs:281-290`, error text "5 Codex
   runs"), refuses a second active run in the thread with a pasted state literal
   (`src/api.rs:291-308`), resolves defaults and picks an execution profile through
   `select_run_setup_for` (`src/api.rs:917`), binds the thread to the repository
   (`src/api.rs:375-390`), writes the prompt message and timeline entry
   (`src/api.rs:392-423`), inserts `agent_runs` in state `created` (`src/api.rs:425-463`),
   moves it to `queued` (`src/api.rs:464`) and emits `agent_run.updated`
   (`src/api.rs:486`). A database trigger, `validate_agent_run_scope_and_admission`
   (first in `migrations/0009_m3_execution_foundation.sql:744`, redefined five times per the
   G report), re-checks membership, roles, worker, profile and repository.
2. **Admission.** `scheduler.rs` claims queued runs under an advisory lock and a lease
   (`src/scheduler.rs:224-255`), re-checks the requester (`src/scheduler.rs:125-200`),
   enforces the host-wide limit from `CODEX_HOST_CONCURRENCY_LIMIT` (`src/config.rs:244`),
   prepares a git worktree from the server's mirror and calls `provider::start_run`
   (`src/scheduler.rs:451`, `:682`).
3. **Turn.** `provider::start_run` (`src/provider.rs:227-413`) takes an adapter slot,
   starts or resumes a provider thread (`provider_sessions`), binds it to the run before the
   side effect (`src/provider.rs:320-350`), starts a turn, binds the turn id
   (`src/provider.rs:357-385`), moves the run to `working` (`src/provider.rs:382`), records
   an agent session for turns watchable elsewhere (`src/provider.rs:388-397`) and
   `tokio::spawn`s `drive_turn` (`src/provider.rs:415-452`).
4. **Driving.** `drive_turn` (`src/provider.rs:744-895`) turns neutral `HarnessEvent`s
   (`src/provider/adapter.rs:158-176`) into: streamed message deltas on the in-process
   realtime hub (`src/provider.rs:785-795`); steps through `record_step`
   (`src/provider.rs:797-811`, writing `agent_run_events`); notes
   (`src/provider.rs:839-856`); tokens through `run_log::record_tokens`
   (`src/provider.rs:836-838`, `src/run_log.rs:424`); and account windows through
   `usage::record_run_report` (`src/provider.rs:833-835`). Every 500 ms it polls the
   database for `interrupted` (`src/provider.rs:862-888`): Stop is a database write the
   loop notices.
5. **Approvals and questions.** `handle_provider_request` (`src/provider.rs:897-1078`)
   inserts `agent_approvals` with `eligible_role = 'owner'` and `human_required = true` for
   approvals, `any_authorized` and `false` for questions (`src/provider.rs:56-68`), moves the
   run to `blocked`, writes a step and a timeline entry, then **polls the database every
   300 ms** until the row is resolved (`src/provider.rs:989-1077`) and passes the answer to
   the adapter.
6. **Finishing.** `complete_run` (`src/provider.rs:1080`) inspects the worktree on disk,
   posts the worker's message, commits, and moves the run to `completed`. Every transition
   goes through `domain::transition_run` (`src/domain.rs:227-302`), which also writes a
   `run_state_changed` event, idles the agent session and kicks the launch queue
   (`src/domain.rs:269-274`, a core function calling two feature modules), and on terminal
   states writes a legacy `agent_run.terminal` event for sponsoring external agents
   (`src/domain.rs:304-353`).
7. **Publishing.** `github.rs` pushes the run branch and opens or syncs the pull request
   from the server's worktree with the GitHub App's installation token
   (`src/github.rs:1630`, `:1822`, `:2092`, `:2875`), on its own leased job queue
   (`src/github.rs:2321-2480`).
8. **Recovery.** At every boot `recovery::reconcile_non_terminal_runs`
   (`src/recovery.rs:29-57`) takes **every** run in `starting`, `working`, `blocked` or
   `recovering`, with no owner filter, and resumes or interrupts it.

### 2.2 An outside agent's work, traced

1. The agent is an `external_agent` actor with a bearer token, created per project
   (`migrations/0033_m9_external_agent_connections.sql:3-33`). The `runtime_kind = 'hermes'`
   and `runtime_version = '0.20.0'` checks at lines 11-12 were already relaxed to a free label
   by `migrations/0037_direct_conversations.sql:183-191` (checked in review); what remains of
   0033 that differs by account kind is the trigger `validate_external_agent_connection`
   (`0033:60-101`: the agent must be an `external_agent` member or viewer, its creator a
   human), which K1.6 drops when it moves the connection's anchor to the workspace.
2. Launch to a "runner" harness (registered with `transport = runner`, `src/harnesses.rs:80`)
   finds `supervisors.runner_for(harness)` (`src/provider/registry.rs:322-326`) and calls
   `launch::hand_off` (`src/launch.rs:2054-2062`). `hand_off`
   (`src/launch.rs:2131-2206`) posts a chat message that mentions the runner with the
   prompt and `Settings: model=… reasoning=… speed=…` as text (`src/launch.rs:2140-2151`),
   and marks the launch `handed_off` with the message id (`src/launch.rs:2182-2194`;
   `migrations/20261017030400_launch_handoff.sql`). **No run row is created.** The
   registered adapter refuses every turn (`src/provider/command_line.rs:880-920`).
3. The mention creates a reply request; the agent gets a signed `reply.requested` wakeup or
   polls `inbox_list` (`docs/agent-runner.md:34-85`).
4. It claims (`reply_claim`), reports **one line** with `reply_progress`
   (`src/mcp/replies.rs:706-775`, which updates `reply_requests.progress_text` and renews
   the lease), binds its session with `session_bind` (`src/mcp/launch_tools.rs:437`,
   writing `agent_sessions` through `src/agent_sessions.rs:201`), and answers with
   `reply_send`.
5. It can ask a person with `ask_person` (`src/mcp/question_tools.rs:211`), stored in
   `person_questions` (`migrations/20261113100000_person_questions.sql`), but only a human
   can be asked (`src/mcp/question_tools.rs:198-200`, `actor.kind = 'human'`).
6. It cannot report steps, tokens, cost, approvals, a run state, a branch or a pull request.
   It does not appear on the Runs page, in run notifications, in usage or in "what is
   running in this thread". Tagging an agent in chat launches a run only for
   `execution_worker` actors (`src/launch.rs:2683-2703`).

### 2.3 The tables

| Table | Defined | What it holds | Notes |
|---|---|---|---|
| `agent_runs` | `migrations/0002_walking_skeleton.sql:216-242`, columns added in `0009:302-330`, `0012:4-37`, `0035:4-12`, `0053:3`, `20261016090300_agent_run_config.sql`, `20261106034702_imports_and_runs_via_app.sql:18` | The run | `execution_worker_id`, `execution_profile_id`, `repository_id` are `NOT NULL`; admission trigger; one active run per thread (`0002:240-242`); config frozen by trigger |
| `agent_run_events` | `0002:255-272`, kinds widened in `20261017030000_run_payloads.sql:5-19` | Steps and state changes | 15 fixed kinds; payload ≤ 64 KiB; FK'd by `conversation_entries` (`0002:300-310`) and `agent_run_acknowledgements` (`0014`) |
| `agent_run_usage` | `20261017030100_run_usage.sql` | Tokens and cost per run and model | `provider_account_id`, `adapter_kind` columns are built-in-host details |
| `agent_approvals` | `0002:274-292`, `0013:24`, `0035:74-85`, `0036:3` | Approvals and questions raised by a run, and approvals of sponsored run requests | `eligible_role` default `'owner'`, `human_required` |
| `agent_run_acknowledgements` | `0014` | "I've seen this failure" per person | |
| `person_questions` | `20261113100000_person_questions.sql` | `ask_person` questions | Asks one person; answer final |
| `agent_sessions` | `20261016220200_agent_sessions.sql` | Where an agent's session for a thread lives | Kept apart from runs on purpose (AGENTS.md); `harness` required |
| `launches` | `20261016220100_launches.sql`, `20261017030400_launch_handoff.sql` | Every launch, queued or started | `CHECK ((state = 'started') = (agent_run_id IS NOT NULL))` |
| `external_execution_requests` | `0035` | Agents' requests for a sponsored run | |
| Built-in-host only | `provider_sessions`, `provider_accounts`, `execution_profiles`, `provider_connections`, `execution_workers`, `custom_harnesses`, `harness_models`, `repository_workspaces`, `git_worktrees`, `usage_*`, `capacity_alert_*` | Accounts, sign-ins, worktrees, slots | All tied to this server's disk or processes (D report F3, F5) |

Foreign keys that point at `agent_runs` (each must be re-pointed or kept for built-in rows):
`messages.agent_run_id` (`0002:251`, composite `0005:78-81`), `agent_run_events` (`0002:257`,
`0005:93-97`), `agent_approvals` (`0002:276`), `agent_runs.predecessor_run_id` (`0012:27-30`),
`agent_run_acknowledgements` (`0014:14`), `work_item_worker_invocations` (`0019:22`),
GitHub publish jobs and pull request requests and workspace links (`0022:59`, `0023:90`,
`0023:172`), `external_execution_requests.agent_run_id` (`0035:30`), `notifications`
(`20261016120000_notifications.sql:35`), `launches` (`20261016220100_launches.sql:48`),
`agent_sessions` (`20261016220200_agent_sessions.sql:31`), `agent_run_usage`
(`20261017030100_run_usage.sql:6`). `notifications.approval_id` points at
`agent_approvals` (`20261016120000_notifications.sql:36`).

### 2.4 Every module that reads or writes the run tables directly

`agent_runs` appears in 35 files under `src/`. Grouped by what they need, with the target
each moves to (section 3.6) and the slice that moves it (section 5):

| File (lines) | What it does with runs | Moves to | Slice |
|---|---|---|---|
| `conversations.rs` (3198, 3506, 4486-4493, 4539, 4611) | Thread busy / archive refusal, thread run cards, queue position, approvals in the timeline | `runs::active_in_conversation`, `runs::thread_cards`, `requests::open_for_conversation` | A1.2, A1.8 |
| `notifications.rs` (1090, 1124, 1218, 1554) | Run finished / approval waiting notifications, open PRs of a run, inbox rows joined to runs | `runs::notify_context`, `runs::get` | A1.2 |
| `home.rs` (85) | Active runs on Home | `runs::list` | A1.2 |
| `projects.rs` (753), `mcp/admin.rs` (1022) | Refuse archiving while runs are active | `runs::active_under_space` | A1.2 |
| `repositories.rs` (580) | Refuse removing a repository with active runs | `runs::active_for_repository` | A1.2 |
| `work_items.rs` (3985) | Latest run state of a task's threads | `runs::latest_in_conversations` | A1.2 |
| `onboarding.rs` (210) | "Has this person started a run?" | `runs::exists_requested_by` | A1.2 |
| `metrics.rs` (374-385) | Run counts by state (pasted literal) | `runs::state_counts` | A1.2 |
| `events.rs` (439), `webhooks.rs` (1517), `outbox.rs` (892), `automations.rs` (2484) | Webhook/automation payloads, state for `agent_run.updated` | `runs::get` | A1.2 |
| `mcp.rs` (5787-5799) | Scope a `runId` argument to its project | `runs::space_of_visible` | A1.2 |
| `external_events.rs` (207) | Agent event for approval requested | `requests` | A1.8 |
| `mcp/approvals.rs` (541, 761, 849) | Approval tools | `requests` | A1.8 |
| `run_views.rs` (104, 388, 512, 643) | Runs page, run page, usage page, diff | `runs::list`, `runs::get`, `runs::usage_report`; diff via results | A1.6, A1.16 |
| `mcp/runs.rs` (192, 208, 626), `mcp/code.rs` (432, 587, 595, 713, 1042, 1125) | `run_get`, `run_list`, stage text; code tools joined to runs | `runs::*` reads | A1.6 |
| `agent_profiles.rs` (965, 968, 1244) | Agent busy, agent sidebar runs | `runs::active_for_account`, `runs::list` | A1.6 |
| `launch.rs` (818, 1048, 1063, 1732) | Launch view, account load, reusable thread | `runs::get`, `runs::active_in_conversation`, builtin `capacity` | A1.6, A1.13 |
| `usage/history.rs` (221, 430) | Runs per provider account in a window | builtin host | A1.6 |
| `account.rs` (311, 666) | Runs per provider account | builtin host | A1.14 |
| `agent_sessions.rs` (257) | `record_run_turn` | builtin host | A1.14 |
| `run_log.rs` (436) | Writes `agent_run_usage` | `runs::usage` | A1.3 |
| `api.rs` (21 refs) | Start, retry, approvals, interrupt, fetch | `runs::create`, `runs::request_cancel`, builtin | A1.10 |
| `external_execution.rs` (841, 931) | Sponsored run start | `runs::create` | A1.10 |
| `domain.rs` (234, 251, 319) | `transition_run` | `runs::transition` (built-in states) | A1.10 |
| `provider.rs` (20 refs), `provider/run_config.rs` (5, a doc comment), `scheduler.rs` (15), `recovery.rs` (7), `retention.rs` (237) | Built-in host machinery | builtin host | A1.9, A1.14 |
| `github.rs` (10 refs) | Publish branch, PR requests linked to runs | code host + `runs::add_result` | A1.19 |
| `runs.rs` | Shared constants and the MCP cap | becomes `runs/mod.rs` | A1.2 |

Other run tables: `agent_run_events` in 9 files (`api.rs`, `conversations.rs`,
`domain.rs`, `mcp/runs.rs`, `mcp/talk.rs`, `provider.rs`, `recovery.rs`, `run_views.rs`,
`scheduler.rs`); `agent_run_usage` in 3 (`run_log.rs`, `run_views.rs`,
`usage/history.rs`); `agent_approvals` in 13 (`api.rs`, `conversations.rs`,
`external_agents.rs`, `external_events.rs`, `external_execution.rs`, `home.rs`, `mcp.rs`,
`mcp/approvals.rs`, `mcp/runs.rs`, `notifications.rs`, `provider.rs`, `push/actions.rs`,
`recovery.rs`); `person_questions` in 4 (`mcp.rs`, `mcp/question_tools.rs`,
`notifications.rs`, `questions.rs`). The active-state list is pasted as a literal in many of
these instead of `runs::ACTIVE_RUN_STATES` (`src/runs.rs:24-31`), for example
`src/api.rs:296`, `src/metrics.rs:375`, `src/retention.rs:239`.

### 2.5 How each kind of record is kept today

- **Steps**: `agent_run_events` rows from `record_step` and `persist_event`
  (`src/provider.rs:1270-1380`), redacted and capped by `run_log`
  (`src/run_log.rs:79`, `:219`, `:298-370`). Some also get a `conversation_entries` row
  (`worker_event`) so they show in the thread.
- **Usage**: `agent_run_usage`, one row per run and model, priced from the workspace model
  catalog with the harness's own cost kept beside it (`src/run_log.rs:424-506`). Account
  windows (rate-limit windows) are separate (`usage::record_run_report`).
- **Approvals and questions from a run**: `agent_approvals`; answered over HTTP
  (`api::answer_approval`, `src/api.rs:1648`), MCP (`approval_respond`,
  `src/mcp.rs:3638`) or a push action (`src/push/actions.rs`); delivered by the drive loop
  polling (`src/provider.rs:989`).
- **Questions to a person**: `person_questions`, answer kept on the row, a
  `question.answered` agent event (`src/questions.rs:1-12`).
- **Diffs**: read live from the worktree on disk, or from the mirror once the worktree is
  gone (`src/run_views.rs:1-8`).
- **Branches and pull requests**: `repository_workspaces.branch_name`, GitHub publish jobs,
  `github_pull_requests` (`migrations/0023_m5_github_pull_requests.sql:5-52`, `html_url`
  checked against `^https://github\.com/` at line 15) and
  `github_pull_request_workspace_links` joining a PR to a run.

### 2.6 Rules that differ by account kind in this area

Each becomes a role setting (section 3.13 lists the replacement):

| Where | Rule today |
|---|---|
| `src/provider.rs:56-68`, `src/api.rs:1715-1741`, `src/mcp/approvals.rs:242` | Approvals are `human_required`: only a person may answer, whatever the role |
| `src/provider.rs:63` | Approvals go to `eligible_role = 'owner'` (a fixed role, not a permission) |
| `src/mcp/question_tools.rs:198-200` | `ask_person` can only ask a `human` |
| `src/mcp/runs.rs:422`, `:573` | `run_start` and run controls refuse non-humans (`NOT_FOR_AGENTS`) |
| `src/mcp/approvals.rs:487`, `:714-726`, `:817` | People and agents take different code paths to read and answer approvals; agents need a connection scope; `approval_list` refuses agents |
| `src/external_execution.rs:172` | `run_request` is for `external_agent` actors only |
| `src/mcp/launch_tools.rs:445` | `session_bind` is for `external_agent` actors only |
| `src/mcp/replies.rs:329-333` | Reply tools require an `external_agent` identity |
| `src/api.rs:454`, `:712` | `made_through_app` is only recorded for humans |
| `src/launch.rs:2683-2703` | Tag-to-launch only for `execution_worker` actors |
| `src/policy.rs:257`, `:285` | `people_only` permissions (agents management, roles, settings) |

### 2.7 Stack names hard-coded in this area

"Codex" in MCP tool descriptions and titles (`src/mcp.rs:3114-3116`, `:3152`, `:3260`,
`:3577-3579`, `:3608`, `:3629`), stage text (`src/mcp/runs.rs:141`, `:257-288`), errors
(`src/api.rs:288`; `:198` is inside a never-compiled `cfg(any())` block), config (`CODEX_HOST_CONCURRENCY_LIMIT`, `CODEX_HOME_ROOT`,
`src/config.rs:241-244`), and harness names in `agent_sessions::harness_name`
(`src/agent_sessions.rs:73-84`). The owner-specific bridge `integrations/agentos-nyx/`
(`bridge.py:49` sends a fixed context name) ships here.

### 2.8 What already has the right shape (keep)

- The adapter layer: `HarnessAdapter`/`HarnessSession` (`src/provider/adapter.rs:356-480`,
  `:443-461`), neutral `HarnessEvent`/`HarnessAnswer` (`:158-183`), `HarnessHost` keeping
  adapters out of the database and secrets (`src/provider/host.rs:1-46`), contract tests
  (`src/provider/contract_tests.rs`). It becomes the inside of the built-in host unchanged.
- Child process hygiene, secrets on fd 3, askpass git credentials, run log redaction
  (D report section 4).
- The durable identity binding before and after the provider side effect
  (`src/provider.rs:320-385`): the same idea as "uncertain acceptance".
- `runs::RunCaller` (`src/runs.rs:37-98`) recording who acted and through which app.
- `agent_sessions` (one session per agent per thread) and the runner contract
  (`docs/agent-runner.md`): the public shape to grow from.
- Thin, provider-neutral web run model (`web/runs/runModel.ts`).

### 2.9 What is in the way

1. A run requires an execution worker, an execution profile and a repository at the column
   and trigger level, so an outside agent's run cannot exist in `agent_runs` without
   breaking its admission trigger.
2. 35 files query run tables directly; any column change touches all of them.
3. State is set by hand in four start paths; transitions call feature modules from
   `domain.rs`.
4. Stop and approvals work by the drive loop polling rows in the same database, which only
   works in-process.
5. Recovery takes every non-terminal run regardless of owner.
6. Approvals and questions are two tables with two answer flows, and both refuse agents by
   kind.

---

## 3. The target design

### 3.1 Pieces and modules

| Piece | Rust module | Tables | End-state concept |
|---|---|---|---|
| Run service | `runs` (grows `src/runs.rs` into `src/runs/`: `mod.rs`, `store.rs`, `state.rs`, `activities.rs`, `usage.rs`, `results.rs`, `ops.rs`, `views.rs`, `liveness.rs`) | `runs`, `run_activities`, `run_usage`, `usage_reports` | Run, Activity |
| Requests | `requests` (`src/requests/`) | `requests` | Request |
| Hosts | `hosts` (`src/hosts/`: `enrol.rs`, `commands.rs`, `transport.rs`, `push.rs`, `bindings/a2a.rs`, `dispatch.rs`) | `hosts`, `host_enrolments`, `host_agents`, `host_commands` | Host |
| Built-in host | `src/hosts/builtin/` in step (a); crate `crates/builtin-host` from step (b) | `agent_runs` (renamed in meaning: built-in run detail) and the built-in-only tables in 2.3 | The built-in host |
| Code host | `code_host` (`src/code_host/`: `mod.rs` trait, `github.rs` impl wrapping today's `github.rs`) | `repositories.code_host`, read model over `github_pull_requests` | Code |

Design choice: **`runs` is a new table beside `agent_runs`, with the same id, rather than a
rename of `agent_runs`.** The alternative (rename and relax the `NOT NULL`s and the admission
trigger) would put the built-in host's columns (worker, profile, worktree, scheduler claim,
provider session) into the core record for good, which is the opposite of the host leaving
the server. A one-to-one detail row with the same id is the pattern tasks already use (a task
core beside its collection record), keeps every existing foreign key and trigger working
during the move, and lets the detail table later move to the host's own store.

### 3.2 The run record

```sql
-- A1: the record of one piece of work by an agent, whoever runs it.
CREATE TABLE runs (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),           -- K4 renames; null for work outside a space
    account_id UUID NOT NULL REFERENCES actors(id),  -- the agent account doing the work
    host_id UUID REFERENCES hosts(id),               -- null: the agent reports for itself
    host_run_ref TEXT CHECK (host_run_ref IS NULL OR (char_length(host_run_ref) BETWEEN 1 AND 200
                              AND host_run_ref !~ '[[:cntrl:]]')),
    backend TEXT CHECK (backend IS NULL OR char_length(backend) BETWEEN 1 AND 80),
                                                     -- only when the host discloses it
    title TEXT NOT NULL CHECK (char_length(title) BETWEEN 1 AND 200 AND title !~ '[[:cntrl:]]'),
    -- Why it exists.
    cause_kind TEXT NOT NULL CHECK (cause_kind IN (
        'task', 'thread', 'message', 'schedule', 'host_trigger', 'automation', 'request', 'manual')),
    cause_ref TEXT CHECK (cause_ref IS NULL OR char_length(cause_ref) <= 200),  -- schedule or trigger name
    conversation_id UUID,                            -- where its messages go
    work_item_id UUID,                               -- the task, when there is one
    cause_message_id UUID,                           -- the message that started it
    requested_by_account_id UUID REFERENCES actors(id),  -- null when the host started it
    sponsored_by_account_id UUID REFERENCES actors(id),
    via_app_id UUID,                                 -- the connected app the requester used
    parent_run_id UUID REFERENCES runs(id),          -- handed-on work
    predecessor_run_id UUID REFERENCES runs(id),     -- a retry
    launch_id UUID,
    -- Where it is.
    state TEXT NOT NULL CHECK (state IN (
        'queued', 'starting', 'start_uncertain', 'accepted', 'working', 'waiting',
        'cancel_requested', 'done', 'failed', 'cancelled', 'refused', 'interrupted')),
    liveness TEXT NOT NULL DEFAULT 'ok' CHECK (liveness IN ('ok', 'quiet', 'not_responding')),
    waiting_on_request_id UUID,                      -- oldest open request; derived
    status_line TEXT CHECK (status_line IS NULL OR char_length(status_line) <= 200),
    outcome_summary TEXT CHECK (outcome_summary IS NULL OR char_length(outcome_summary) <= 2000),
    failure_reason TEXT CHECK (failure_reason IS NULL OR char_length(failure_reason) <= 2000),
    external_url TEXT CHECK (external_url IS NULL OR external_url ~ '^https://[^[:space:][:cntrl:]]+$'),
    -- Reporting.
    client_run_id TEXT CHECK (client_run_id IS NULL OR char_length(client_run_id) BETWEEN 1 AND 200),
    report_interval_seconds INTEGER NOT NULL DEFAULT 900 CHECK (report_interval_seconds BETWEEN 30 AND 86400),
    last_report_at TIMESTAMPTZ,
    activity_seq BIGINT NOT NULL DEFAULT 0,
    gap_count INTEGER NOT NULL DEFAULT 0,
    -- Settings the run used, frozen once set (same rule as agent_runs.config).
    settings JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(settings) = 'object' AND octet_length(settings::text) <= 16384),
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    accepted_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    cancel_requested_at TIMESTAMPTZ,
    cancel_requested_by_account_id UUID REFERENCES actors(id),
    finished_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (space_id, id),
    UNIQUE (conversation_id, id),
    FOREIGN KEY (space_id, conversation_id) REFERENCES conversations(project_id, id),
    FOREIGN KEY (space_id, work_item_id) REFERENCES work_items(project_id, id),
    CHECK (parent_run_id IS NULL OR parent_run_id <> id),
    CHECK ((state IN ('done','failed','cancelled','refused','interrupted')) = (finished_at IS NOT NULL))
);
CREATE UNIQUE INDEX runs_host_ref ON runs (host_id, host_run_ref) WHERE host_run_ref IS NOT NULL;
CREATE UNIQUE INDEX runs_client_ref ON runs (account_id, client_run_id) WHERE client_run_id IS NOT NULL;
CREATE INDEX runs_workspace_recent ON runs (workspace_id, created_at DESC, id DESC);
CREATE INDEX runs_space_recent ON runs (space_id, created_at DESC, id DESC);
CREATE INDEX runs_active_conversation ON runs (conversation_id)
    WHERE state NOT IN ('done','failed','cancelled','refused','interrupted');
CREATE INDEX runs_active_account ON runs (account_id)
    WHERE state NOT IN ('done','failed','cancelled','refused','interrupted');
CREATE INDEX runs_active_host ON runs (host_id)
    WHERE host_id IS NOT NULL AND state NOT IN ('done','failed','cancelled','refused','interrupted');
CREATE INDEX runs_liveness_due ON runs (last_report_at)
    WHERE state IN ('accepted','working','waiting','cancel_requested');
```

Order note: `hosts` arrives in A1.11, after this table (A1.1) and `requests` (A1.7). Those
slices create `host_id` and `raised_via_host_id` as plain `UUID` columns; A1.11's migration
adds the foreign keys (`NOT VALID`, then `VALIDATE`), so the migrations apply in timestamp
order on the live server whatever else is pending.

`agent_runs` stays as the built-in host's detail row (same id, `agent_runs.id REFERENCES
runs(id)` added once every row has a parent). Its own state column remains the built-in
host's internal machine (scheduler, recovery) until step (c) of section 3.11.

**The message link to a run.** `messages.agent_run_id` keeps its name and, from A1.3, points at
`runs(id)` (the composite `(conversation_id, agent_run_id)` key is re-pointed at
`runs (conversation_id, id)`). It is the "`messages.run_id`" C2's run chip reads. It is set for
any account, not only the built-in host: when a message is posted by an account that has an
active run in that conversation, by a call carrying K2's `context.runId`, or with a run-scoped
token (K1.24), the send pipeline records that run's id.

**Moving a conversation off its project (K4).** `runs` and `requests` carry `space_id` beside
`conversation_id`, with a composite key to `conversations (project_id, id)`. When K4.15/K4.16
move DM conversations out of the hidden Messages project (`project_id = NULL`), the same
migration calls A1's `runs::move_conversation_space(tx, conversation_id, new_space)` (an
`UPDATE runs SET space_id = $2` and the same on `requests`, `run_usage`), so the composite key
stays valid; with `space_id` NULL the key is not checked (`MATCH SIMPLE`). A1 provides the
function in A1.2.

**State machine.** States describe the work; `liveness` describes the reporter.

| State | Meaning | Entered by | Leaves to |
|---|---|---|---|
| `queued` | Recorded, not yet sent to anyone (waiting for a slot, a schedule or blockers) | `runs::create` | `starting`, `cancelled`, `failed` |
| `starting` | Start command issued, no answer yet | dispatch | `accepted`, `working`, `refused`, `start_uncertain`, `cancel_requested` |
| `start_uncertain` | The host may or may not have started it (it said "unknown", or the acknowledgement was lost) | ack `uncertain`, ack deadline passed | `accepted`, `working` (a report arrives), `failed` (a person marks it not started), `cancel_requested` |
| `accepted` | Host took it, nothing reported yet | ack `accepted` | `working`, `waiting`, `done`, `failed`, `cancel_requested` |
| `working` | Reports arriving | `run_started`, any step | `waiting`, `done`, `failed`, `cancel_requested`, `interrupted` |
| `waiting` | At least one open request raised by this run | `interrupt` activity / `request_open` with `runId` | `working` (all answered or closed), `cancel_requested`, `failed`, `done` |
| `cancel_requested` | Someone asked it to stop; not yet confirmed | `run_cancel` | `cancelled`, `done`, `failed`, `interrupted` |
| `done`, `failed`, `cancelled` | Finished, with that outcome | `run_finished` with outcome, or `run_update` | terminal |
| `refused` | Host said no | ack `refused` | terminal |
| `interrupted` | Ended without a known outcome (host lost, unlinked, or restart that could not prove completion) | recovery, revocation | terminal |

A run opened by an agent for itself (`run_open`) starts in `working`; one adopted from a host
trigger starts in `accepted` or `working`, whichever the host reports.

**Liveness.** `ok` while reports arrive within `report_interval_seconds`; `quiet` after one
interval; `not_responding` after three intervals, or when a start is not acknowledged within
60 seconds, or the host has not fetched commands for 3 heartbeats. Any report sets it back
to `ok`. A sweeper job (`runs.liveness`, recurring on `jobs.rs`) sets it and emits
`run.liveness_changed`. Choice: liveness is separate from state because a run can be "working
but silent"; folding them would lose what it was doing. It is stored, not computed at read,
so lists can filter on it and an event can fire once.

**Derived state.** The run service applies one deterministic reducer to every write:

```
state' = reduce(state, event) where event ∈ {
  ack(accepted|refused|uncertain), activity(kind, outcome?), request_opened, request_closed,
  cancel_requested, update(explicit_state), host_lost }
```

Lifecycle activities drive it (`run_started` → `working`; `run_finished{outcome}` →
`done|failed|cancelled`; `interrupt` → `waiting`; the last open request closing →
`working`); `run_update` may set a state explicitly only along the same edges. A test replays
stored activities through the reducer and must reach the stored state (guard rail G7).

**Mapping from today's `agent_runs.state`** (used by the mirror trigger and backfill):
`created`, `queued` → `queued`; `starting` → `starting`; `working` → `working`;
`blocked` → `waiting`; `recovering` → `working` with `status_line = 'Recovering after a
restart'`; `completed` → `done`; `failed` → `failed`; `canceled` → `cancelled`;
`interrupted` → `cancelled` when `cancel_requested_at` is set (a person pressed Stop),
otherwise `interrupted`.

### 3.3 Activities

```sql
-- A1: typed entries in a run, named after AG-UI's events (pinned to one draft version).
CREATE TABLE run_activities (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    seq BIGINT NOT NULL,                              -- order Workspace received them in
    client_id TEXT CHECK (client_id IS NULL OR char_length(client_id) BETWEEN 1 AND 200),
    kind TEXT NOT NULL CHECK (kind IN (
        'run_started', 'run_finished', 'run_error',
        'step_started', 'step_finished',
        'text_message', 'reasoning', 'tool_call',
        'plan', 'state_snapshot', 'state_delta',
        'interrupt', 'interrupt_resolved',
        'usage', 'result', 'gap', 'custom')),
    step_ref TEXT CHECK (step_ref IS NULL OR char_length(step_ref) <= 200),
    parent_ref TEXT CHECK (parent_ref IS NULL OR char_length(parent_ref) <= 200),
    summary TEXT NOT NULL CHECK (char_length(summary) <= 2000),
    payload JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 65536),
    occurred_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    reported_by_account_id UUID NOT NULL REFERENCES actors(id),
    in_thread BOOLEAN NOT NULL DEFAULT false,
    legacy_event_id UUID UNIQUE,                      -- agent_run_events.id when copied
    UNIQUE (run_id, seq),
    UNIQUE (run_id, client_id)
);
CREATE INDEX run_activities_run_order ON run_activities (run_id, seq);
```

| Kind | AG-UI event | Payload (all optional unless marked) | From today's `agent_run_events.kind` |
|---|---|---|---|
| `run_started` | `RUN_STARTED` | `threadRef`, `hostRunRef` | `run_state_changed` to working |
| `run_finished` | `RUN_FINISHED` | **`outcome`** (`done`, `failed`, `cancelled`), `summary` | `run_completed`, `run_failed`, `run_interrupted`, `run_state_changed` to terminal |
| `run_error` | `RUN_ERROR` | `code`, `message` | `run_state_changed` with reason |
| `step_started` / `step_finished` | `STEP_STARTED` / `STEP_FINISHED` | `title`, `status` | |
| `text_message` | `TEXT_MESSAGE_START/CONTENT/END` collapsed into one | **`text`**, `role` (`assistant`, `note`) | `agent_note` |
| `reasoning` | `THINKING_*` / reasoning | `text` | |
| `tool_call` | `TOOL_CALL_START/ARGS/END/RESULT` collapsed | **`toolCallRef`**, `tool`, `toolKind` (`command`, `file_change`, `mcp`, `other`), `status`, `command`, `exitCode`, `output` (start and end, capped), `files[]`, `tests` | `command_started`, `command_completed`, `file_change`, `tool_used`, `test_result` |
| `plan` | `STATE_SNAPSHOT` with a plan key | **`items[]`** `{title, status: pending|in_progress|done|skipped}` (whole list each time, Linear-style) | |
| `state_snapshot` / `state_delta` | `STATE_SNAPSHOT` / `STATE_DELTA` | `state` / JSON Patch `ops` | |
| `interrupt` | `RUN_FINISHED` with an interrupt outcome (draft "interrupts") | **`request`** (section 3.8) or `requestId` | `approval_requested`, `question_requested` |
| `interrupt_resolved` | resume | **`requestId`**, `answerSummary` | `approval_resolved`, `question_resolved` |
| `usage` | (extension) | same shape as `run_usage_report` | |
| `result` | (extension) | same shape as `run_result` | |
| `gap` | (extension) | `reason`, `from`, `to`, `missed` | |
| `custom` | `CUSTOM` | `name`, `value` | `recovery` → `custom{name:"recovery"}` |

Streaming deltas are not stored: hosts send whole messages, and a live typing stream stays a
realtime-only event (K5's fan-out). Choice: storing deltas would multiply rows by ~100 for no
lasting value.

Every string in a payload goes through `run_log::redact` and `cap` (`src/run_log.rs:79`,
`:219`) whatever the reporter. Activities with `in_thread = true` (finished runs, requests,
results, errors) also get a `conversation_entries` row of a new kind `run_activity` (with a
`run_activity_id` column), so they show in the thread as today's `worker_event` entries do.

### 3.4 Usage and cost

```sql
-- A1: what a reporter said it used, as it said it (per run, or for a time window).
CREATE TABLE usage_reports (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    reported_by_account_id UUID NOT NULL REFERENCES actors(id),
    account_id UUID NOT NULL REFERENCES actors(id),       -- whose work it was
    host_id UUID REFERENCES hosts(id),
    run_id UUID REFERENCES runs(id) ON DELETE SET NULL,   -- null: a window report
    report_key TEXT NOT NULL CHECK (char_length(report_key) BETWEEN 1 AND 200),
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    window_start TIMESTAMPTZ, window_end TIMESTAMPTZ,
    input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (input_tokens >= 0),
    output_tokens BIGINT NOT NULL DEFAULT 0 CHECK (output_tokens >= 0),
    cached_input_tokens BIGINT NOT NULL DEFAULT 0 CHECK (cached_input_tokens >= 0),
    reported_cost DOUBLE PRECISION CHECK (reported_cost IS NULL OR reported_cost >= 0),
    reported_currency TEXT CHECK (reported_currency IS NULL OR reported_currency ~ '^[A-Z]{3}$'),
    attributed_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (reported_by_account_id, report_key),
    CHECK (run_id IS NOT NULL OR (window_start IS NOT NULL AND window_end > window_start))
);

-- A1: usage per run and model, as the run page and totals show it.
CREATE TABLE run_usage (
    run_id UUID NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    model TEXT NOT NULL CHECK (char_length(model) BETWEEN 1 AND 200),
    attribution TEXT NOT NULL CHECK (attribution IN ('reported', 'window')),
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    host_id UUID REFERENCES hosts(id),
    backend TEXT,
    usage_date DATE NOT NULL,
    input_tokens BIGINT NOT NULL DEFAULT 0, output_tokens BIGINT NOT NULL DEFAULT 0,
    cached_input_tokens BIGINT NOT NULL DEFAULT 0,
    estimated_cost DOUBLE PRECISION, currency TEXT CHECK (currency IS NULL OR currency ~ '^[A-Z]{3}$'),
    reported_cost DOUBLE PRECISION, reported_currency TEXT,
    confidence TEXT NOT NULL CHECK (confidence IN ('exact', 'estimated')),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (run_id, model, attribution)
);
CREATE INDEX run_usage_workspace_day ON run_usage (workspace_id, usage_date);
CREATE INDEX run_usage_account_day ON run_usage (account_id, usage_date);
```

- **Reported per run** (built-in host, Claude-style hosts, any agent that knows): added to
  `run_usage` with `attribution = 'reported'`, `confidence = 'exact'`, priced from the
  workspace model catalog as `run_log::price` does today (`src/run_log.rs:400`).
- **Attributed by window** (a host whose meter is per agent per minute, as Agent OS's
  broker is): the report is kept in `usage_reports`; a job splits it across the account's
  runs (on that host, when given) whose `[started_at, finished_at or now]` overlaps the
  window, in proportion to the overlap, writing `attribution = 'window'`,
  `confidence = 'estimated'`. A window with no overlapping run stays unattributed and shows
  as "not tied to a run" on the usage page. Choice: keeping raw reports means a later,
  better split (or a host that starts tagging runs) can recompute without data loss.
- `agent_run_usage` rows are copied in with `attribution = 'reported'`,
  `confidence = 'exact'`, `backend = adapter_kind`.
- Budgets (A2) and metering (H1) read `run_usage` totals; they never read raw reports.
  There is one counting mechanism, H1's (ruling R13): from A2.14, `runs::record_usage` calls
  `metering::record` with the reported delta for meters `reported_cost_micros` and
  `reported_tokens` (account, space, conversation of the run; `dedupe_key =
  usage_report:<usage_reports.id>`), and the window job records its estimated split with
  `dedupe_key = usage_attr:<report id>:<run id>`. For a host H1 marks as the hosted agent
  service (`host_billing`, H1.38) the same report also records `agent_service_micros`, and
  A1.13's dispatch calls `metering::admit(agent_service_micros, ...)` before sending `start`.
  A1 itself adds no counters.

### 3.5 Results

A result is a `result` activity plus, once K3 lands, a link in K3's store from the result
thing to the run with K3's existing kind `made_from` (`links::create(thing, run,
"made_from")`); K3's link kinds stay closed, so A1 adds none. Kinds: `pull_request {url, number?, branch,
repositoryId?}`, `branch {name, repositoryId, headSha?}`, `diff {fileId, truncated}`,
`thing {kind, id}` (page, sheet, task, record), `file {fileId}`, `link {url, title}`. A pull
request result whose URL belongs to a linked repository is resolved through the code host
(3.14) into `github_pull_requests` and `github_pull_request_workspace_links`, so every PR
feature (checks, reviews, task automation) works for it. Diffs reported by hosts are stored
through K5's file storage seam, at most 1 MiB, marked when truncated (Agent OS already caps
its diff at 1 MiB).

### 3.6 The run service

One module owns every read and write of `runs`, `run_activities`, `run_usage`,
`usage_reports` and, until the built-in host moves out, the cross-cutting reads of
`agent_runs`. Key signatures:

```rust
// src/runs/mod.rs
pub use state::{RunState, Liveness};

pub enum Cause { Task(Uuid), Thread(Uuid), Message(Uuid), Schedule(String),
                 HostTrigger(String), Automation(Uuid), Request(Uuid), Manual }

pub enum Reporter {
    Account { account_id: Uuid },                    // an agent reporting for itself
    Host { host_id: Uuid, account_id: Uuid },        // a host reporting for an agent it runs
    BuiltinHost,                                     // in-process, until step (b)
    System,                                          // sweeper, recovery
}

pub struct RunSpec {
    pub workspace_id: Uuid, pub space_id: Option<Uuid>, pub account_id: Uuid,
    pub host: Option<(Uuid, Option<String>)>,        // host id, host run ref
    pub cause: Cause, pub conversation_id: Option<Uuid>, pub work_item_id: Option<Uuid>,
    pub cause_message_id: Option<Uuid>, pub requested_by: Option<Uuid>,
    pub sponsored_by: Option<Uuid>, pub via_app_id: Option<Uuid>,
    pub parent_run_id: Option<Uuid>, pub predecessor_run_id: Option<Uuid>,
    pub launch_id: Option<Uuid>, pub title: String, pub settings: serde_json::Value,
    pub client_run_id: Option<String>, pub report_interval: Option<u32>,
    pub initial: RunState,                           // Queued, Working (self-opened) or Accepted (adopted)
}

// Writes: each takes the caller's transaction so history and events commit together.
pub async fn create(tx: &mut Tx<'_>, spec: RunSpec) -> Result<Run, AppError>;
pub async fn open(tx: &mut Tx<'_>, by: &Reporter, input: RunOpen) -> Result<Opened, AppError>;      // idempotent
pub async fn update(tx: &mut Tx<'_>, by: &Reporter, run_id: Uuid, change: RunUpdate) -> Result<Run, AppError>;
pub async fn append(tx: &mut Tx<'_>, by: &Reporter, run_id: Uuid, batch: Vec<NewActivity>) -> Result<Appended, AppError>;
pub async fn record_usage(tx: &mut Tx<'_>, by: &Reporter, report: UsageReportInput) -> Result<UsageRecorded, AppError>;
pub async fn add_results(tx: &mut Tx<'_>, by: &Reporter, run_id: Uuid, results: Vec<RunResult>) -> Result<(), AppError>;
pub async fn request_cancel(tx: &mut Tx<'_>, by: &RunCaller, run_id: Uuid) -> Result<Run, AppError>;
pub async fn acknowledge_start(tx: &mut Tx<'_>, host_id: Uuid, run_id: Uuid, ack: StartAck) -> Result<Run, AppError>;
pub async fn mark_not_started(tx: &mut Tx<'_>, by: &RunCaller, run_id: Uuid) -> Result<Run, AppError>;
pub async fn interrupt_all_on_host(tx: &mut Tx<'_>, host_id: Uuid, reason: &str) -> Result<u64, AppError>;

// Reads: each filters by the viewer's access.
pub async fn get(db: impl PgExecutor<'_>, viewer: Uuid, run_id: Uuid) -> Result<Option<RunView>, AppError>;
pub async fn list(db: impl PgExecutor<'_>, viewer: Uuid, filter: RunFilter, page: PageRequest) -> Result<Page<RunSummary>, AppError>;
pub async fn activities(db: impl PgExecutor<'_>, viewer: Uuid, run_id: Uuid, after_seq: i64, limit: i64) -> Result<Vec<Activity>, AppError>;
pub async fn usage_report(db: impl PgExecutor<'_>, viewer: Uuid, query: UsageQuery) -> Result<UsageTable, AppError>;
pub async fn thread_cards(db: impl PgExecutor<'_>, viewer: Uuid, conversation_id: Uuid) -> Result<Vec<RunCard>, AppError>;
pub async fn notify_context(db: impl PgExecutor<'_>, run_id: Uuid) -> Result<Option<NotifyContext>, AppError>;
pub async fn active_in_conversation(db: impl PgExecutor<'_>, conversation_id: Uuid) -> Result<bool, AppError>;
pub async fn active_under_space(db: impl PgExecutor<'_>, space_id: Uuid) -> Result<bool, AppError>;
pub async fn active_for_repository(db: impl PgExecutor<'_>, repository_id: Uuid) -> Result<bool, AppError>;
pub async fn active_for_account(db: impl PgExecutor<'_>, account_id: Uuid) -> Result<i64, AppError>;
pub async fn latest_in_conversations(db: impl PgExecutor<'_>, ids: &[Uuid]) -> Result<Vec<(Uuid, RunState)>, AppError>;
pub async fn space_of_visible(db: impl PgExecutor<'_>, viewer: Uuid, run_id: Uuid) -> Result<Option<Uuid>, AppError>;
pub async fn exists_requested_by(db: impl PgExecutor<'_>, space_id: Uuid, account_id: Uuid) -> Result<bool, AppError>;
pub async fn state_counts(db: impl PgExecutor<'_>) -> Result<Vec<(RunState, i64)>, AppError>;

// Background.
pub async fn sweep_liveness(pool: &PgPool) -> Result<u64, AppError>;
pub async fn attribute_window_usage(pool: &PgPool) -> Result<u64, AppError>;
```

Rules the service enforces in one place:

- **Visibility.** A run is visible to a viewer when (a) it has a conversation and
  `conversation_visible_to(conversation_id, viewer)`; else (b) it has a task the viewer can
  see; else (c) it has a space and the viewer can access it
  (`actor_can_access_project`); else (d) the viewer is the account, the requester, or
  manages the account. Until K1's SQL filter arrives this is one SQL fragment in
  `runs::store::VISIBLE` used by every read; then it calls K1's filter.
- **Who may report.** `Reporter::Account` may write only runs whose `account_id` is the
  caller. `Reporter::Host` may write only runs with its `host_id` for an account bound to it
  in `host_agents`. Every report re-checks the account still has access to the run's space
  and conversation (principle 9 in the G report).
- **Events** (K3's log; until it lands, the outbox): `run.opened`, `run.updated` (today's
  `agent_run.updated` and webhook `run.updated` keep their names, `src/events.rs:37-38`),
  `run.activity` (one thin event per batch), `run.finished` (unchanged),
  `run.liveness_changed`, `run.result_added`, `run.usage_updated`, and the two addressed
  commands to an agent that reports for itself, `run.start` and `run.cancel` (3.16). Each
  write passes its run id to K3's `events::emit` (`events.run_id`), so every event caused
  during a run names it.
- **Active states** come from `RunState::is_active()`; no literals elsewhere (G4).

Moving the 35 files: section 2.4 gives each file's target. Reads move first (A1.2, A1.6) with
no behaviour change, because the mirror keeps `runs` equal to `agent_runs` for built-in runs.
Writes move with the slices that own them (A1.3, A1.10, A1.14). The guard test (G1) holds an
allow-list of files still touching run tables directly; each slice deletes its lines from the
list, and the test fails both on a new file and on a listed file that no longer needs its
entry, so the list can only shrink.

### 3.7 Run operations for any agent

Defined once in K2's operation layer (`ops`), giving an MCP tool, a public API endpoint and
an automation action each. If K2's registry has not merged when A1.3 starts, they are written
as today's MCP tools plus routes, in K2's envelope (`{outcome, value}` with
`clientRequestId`), so moving them is mechanical. Names avoid today's `run_usage` (a read of
usage totals, `src/mcp.rs:3297`) and `run_request` (an agent asking for a sponsored run,
`src/mcp.rs:3586`), which keep their meaning.

| Operation | Kind | Input | Permission | Effect |
|---|---|---|---|---|
| `run_open` | create | `clientRunId?`, `hostRunRef?`, `accountId?` (hosts only), `title`, `cause {kind, taskId?, conversationId?, messageId?, ref?}`, `spaceId?`, `parentRunId?`, `reportIntervalSeconds?`, `settings?`, `state?` (`accepted`/`working`) | `runs.report` in the space; caller is the account, or a host bound to it | Creates or adopts; returns the existing run on a repeat |
| `run_update` | change | `runId`, `state?`, `statusLine?`, `title?`, `externalUrl?`, `backend?`, `failureReason?`, `expectedVersion?` | reporter of this run | Explicit transition along allowed edges; renews liveness |
| `run_activity` | change | `runId`, `activities[]` (≤ 100), each `{id, kind, at, stepRef?, parentRef?, summary?, payload}` | reporter of this run | Appends, deduplicated by `id`; derives state; may open requests (`interrupt`) |
| `run_usage_report` | change | `reportKey`, `runId?` or `{accountId, windowStart, windowEnd}`, `model`, tokens, `cost?`, `currency?` | reporter | Records usage, per run or for a window |
| `run_result` | change | `runId`, `results[]` (≤ 20) | reporter | Adds results and links |
| `run_cancel` | change | `runId`, `reason?` | `runs.interrupt` and visibility (existing permission) | `cancel_requested`; sends `cancel` to the host or `run.cancel` event to the agent |
| `run_mark_not_started` | change | `runId` | `runs.interrupt` | From `start_uncertain` to `failed` with "marked not started" |
| `run_start_input` | query | `runId` | the run's own account (or a host bound to it) | The start input of a run that has no host, in the same shape as a `start` host command's body (3.9: `input`, `cause`, `requestedBy`, `settings`, `repository` with its short-lived token, `workspaceCredential`), secrets minted at fetch and never stored. It is what A2's `run.start` event points at (`inputRef`), and the first fetch moves the run from `accepted` to `working` only when a report follows. Route `GET /api/v1/runs/{id}/start-input` |
| `run_get`, `run_list`, `run_steps`, `run_usage` | query | existing | visibility | Existing tools, now over every run |

**Permissions.** `runs.report` is new in K1's catalogue, held by every starting role
including the default agent role. No kind check anywhere: a person running a script can open
runs for their own account in exactly the same way.

**A chat reply is not a run** (ruling R6) unless a launch created one or the agent opened one
with `run_open`. A2's working indicator (`presence_set`, `reply_progress`) and the reply flow
never open a run; when a run exists for the request (below), they also write to it.

**Idempotency.** `run_open`: `(account, clientRunId)` or `(host, hostRunRef)` unique; a repeat
returns `created: false` and the current run. `run_activity`: `(run, id)` unique, conflicts
counted as duplicates, never errors. `run_usage_report`: `(reporter, reportKey)`.
`run_result`: by `(run, kind, url or id)`. `run_update`, `run_cancel`: `clientRequestId` via
K2's idempotency store, plus optional `expectedVersion`.

**Limits.** Batch ≤ 100 activities and ≤ 256 KiB per call; payload ≤ 48 KiB after redaction
(`run_log::MAX_PAYLOAD_BYTES`); summary ≤ 2,000 characters; 20,000 activities per run (past
that, steps are refused with `limit_reached` and one `gap` activity records it; lifecycle,
interrupt and result kinds are always accepted); at most 50 runs per account not finished
(workspace setting); report calls rate-limited per account through K5's shared limits
(default 20 per second, bursts of 100). Over-limit returns K2's `rate_limited` error with
`retryAfterSeconds`.

**Examples.**

```json
// run_open, a bot reporting its own scheduled work
{"clientRunId": "triage-2026-11-14", "title": "Triage new playtest feedback",
 "cause": {"kind": "schedule", "ref": "weekday 09:00"}, "spaceId": "0199…",
 "reportIntervalSeconds": 600}
// -> {"outcome": "ok", "value": {"runId": "0199a…", "state": "working", "version": 1, "created": true}}

// run_activity, batched
{"runId": "0199a…", "activities": [
  {"id": "a1", "kind": "plan", "at": "2026-11-14T09:00:02Z", "payload": {"items": [
     {"title": "Read new feedback", "status": "done"},
     {"title": "Group by theme", "status": "in_progress"},
     {"title": "File tasks", "status": "pending"}]}},
  {"id": "a2", "kind": "tool_call", "at": "2026-11-14T09:00:05Z", "stepRef": "s2",
   "summary": "record_list Playtest feedback",
   "payload": {"toolCallRef": "t7", "tool": "record_list", "toolKind": "mcp", "status": "completed"}},
  {"id": "a3", "kind": "gap", "at": "2026-11-14T09:04:00Z",
   "payload": {"reason": "process restarted", "from": "2026-11-14T09:01:10Z", "to": "2026-11-14T09:03:55Z"}},
  {"id": "a4", "kind": "interrupt", "at": "2026-11-14T09:04:10Z", "payload": {"request": {
     "kind": "question", "title": "File the 3 crash reports as one task or three?",
     "options": [{"id": "one", "label": "One task"}, {"id": "three", "label": "Three tasks"}],
     "allowText": true, "audience": {"accounts": ["0199-person…"]}, "clientRequestId": "q-1"}}}]}
// -> {"outcome": "ok", "value": {"accepted": 4, "duplicates": 0, "lastSeq": 4,
//      "state": "waiting", "requests": [{"clientRequestId": "q-1", "requestId": "0199r…"}]}}

// run_usage_report, a window (the host meters per agent per minute)
{"reportKey": "meter-2026-11-14T09:00Z-fixer", "accountId": "0199-agent…",
 "windowStart": "2026-11-14T09:00:00Z", "windowEnd": "2026-11-14T09:15:00Z",
 "model": "model-a", "inputTokens": 120000, "outputTokens": 8000}

// run_result
{"runId": "0199a…", "results": [
  {"kind": "pull_request", "url": "https://code.example/acme/app/pull/12", "branch": "agent/awd-142"},
  {"kind": "thing", "thingKind": "page", "id": "0199p…"}]}
```

**How `reply_progress` and `session_bind` map, without breaking anyone.** Both keep their
names, arguments and results (`docs/agent-runner.md:107-131`).

- A launch to an agent that connects by itself now creates a run whose `cause_message_id` is
  the hand-off message (3.12). `reply_claim` adds an optional `run {runId, state}` to its
  result when a run of this account has that message as its cause. Old runners ignore the
  extra field.
- `reply_progress(requestId, generation, status)` behaves exactly as now. When the request
  has a run (above), the same call also appends a `step_started {title: status}` activity
  (`client_id = "reply-progress:<request>:<generation>:<hash>"`) and sets the run's
  `status_line`. No run is opened implicitly: a chat reply is not a run unless a launch made
  one or the agent opens one (END-STATE section 4, "a turn ... is a chat reply unless it
  does long work").
- `reply_send(final: true)` on a request whose run is still `working` appends
  `run_finished {outcome: "done"}`; `reply_release` appends `run_finished {outcome: "failed",
  summary: reason}`; Stop (`reply.cancelled`) is `run_cancel`.
- `session_bind(conversationId, externalRef, harness, url?, state?)` keeps writing
  `agent_sessions`. When the agent has a run active in that conversation it also sets that
  run's `external_url` and, if given, `backend`. `harness` becomes optional (C7 in R1), and
  display names come from host descriptions, never from the list in
  `src/agent_sessions.rs:75-84` (which becomes "the value as given").
- The SDKs (`sdks/runner-python`, `sdks/runner-typescript`) gain `run.open`, `run.activity`
  (buffering and batching), `run.usage`, `run.result` helpers, and `docs/agent-runner.md`
  gains a "6. Show your work" section. Their tests join CI (D report F9 gap).

### 3.8 Requests

One record behind approvals, questions and acceptance of proposals.

```sql
-- A1: something an account needs from someone: an approval, an answer, or acceptance of a proposal.
CREATE TABLE requests (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),
    kind TEXT NOT NULL CHECK (kind IN ('approval', 'question', 'proposal')),
    raised_by_account_id UUID NOT NULL REFERENCES actors(id),
    raised_via_host_id UUID REFERENCES hosts(id),
    run_id UUID REFERENCES runs(id),
    host_request_ref TEXT CHECK (host_request_ref IS NULL OR char_length(host_request_ref) BETWEEN 1 AND 512),
    client_request_id TEXT CHECK (client_request_id IS NULL OR char_length(client_request_id) BETWEEN 1 AND 200),
    -- What it is about.
    subject_kind TEXT CHECK (subject_kind IS NULL OR subject_kind ~ '^[a-z][a-z0-9_]{0,63}$'),
    subject_id UUID,                     -- a proposal (C1), a sponsored run request, a thing
    conversation_id UUID,
    work_item_id UUID,
    title TEXT NOT NULL CHECK (char_length(btrim(title)) BETWEEN 1 AND 2000),
    detail TEXT CHECK (detail IS NULL OR char_length(detail) <= 8000),
    options JSONB NOT NULL DEFAULT '[]'::jsonb CHECK (
        jsonb_typeof(options) = 'array' AND jsonb_array_length(options) <= 8
        AND octet_length(options::text) <= 4000),   -- [{"id","label","style"?: "approve"|"deny"}]
    allow_text BOOLEAN NOT NULL DEFAULT false,
    -- Who may answer: anyone holding answer_permission in the space who can see the subject,
    -- narrowed to the named accounts and roles when either list is non-empty.
    answer_permission TEXT NOT NULL CHECK (answer_permission ~ '^[a-z]+\.[a-z_.]+$'),
    addressed_account_ids UUID[] NOT NULL DEFAULT '{}' CHECK (cardinality(addressed_account_ids) <= 20),
    addressed_role_keys TEXT[] NOT NULL DEFAULT '{}' CHECK (cardinality(addressed_role_keys) <= 10),
    addressed_group_ids UUID[] NOT NULL DEFAULT '{}' CHECK (cardinality(addressed_group_ids) <= 10),
                                         -- K1's account_groups (K1.27): "ask the reviewers group"
    status TEXT NOT NULL DEFAULT 'open' CHECK (status IN (
        'open', 'answered', 'expired', 'cancelled', 'superseded')),
    answer JSONB CHECK (answer IS NULL OR (jsonb_typeof(answer) = 'object' AND octet_length(answer::text) <= 8192)),
    answered_by_account_id UUID REFERENCES actors(id),
    answered_via_app_id UUID,
    answered_at TIMESTAMPTZ,
    delivered_at TIMESTAMPTZ,            -- the raiser (or its host) has it
    expires_at TIMESTAMPTZ,
    on_expiry TEXT NOT NULL DEFAULT 'expire' CHECK (on_expiry IN ('expire', 'deny')),
    legacy_source TEXT CHECK (legacy_source IN ('agent_approval', 'person_question')),
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (run_id, host_request_ref),
    UNIQUE (raised_by_account_id, client_request_id),
    CHECK ((status = 'answered') = (answered_at IS NOT NULL AND answer IS NOT NULL))
);
CREATE INDEX requests_open_space ON requests (space_id, created_at) WHERE status = 'open';
CREATE INDEX requests_open_addressed ON requests USING gin (addressed_account_ids) WHERE status = 'open';
CREATE INDEX requests_open_run ON requests (run_id) WHERE status = 'open';
CREATE INDEX requests_expiring ON requests (expires_at) WHERE status = 'open' AND expires_at IS NOT NULL;
```

- **Who may answer.** `requests::may_answer(viewer, request)`: the viewer holds
  `answer_permission` in the request's space (K1's access function), can see the subject
  (`conversation_visible_to` for a conversation, task visibility for a task), is not the
  raiser, and, if `addressed_account_ids`, `addressed_role_keys` or `addressed_group_ids` is
  non-empty, is named, holds one of the roles or is in one of the groups. No account-kind
  test. Defaults: approvals of sponsored run requests use `runs.approve` (existing,
  `src/policy.rs:459`); approvals a run raises that today are `human_required` carry K1's
  `requests.answer_sensitive` (K1.14), which the starting roles for people hold and the default
  agent role does not, so a workspace may give it to an agent's role; questions use
  `requests.answer` (new); proposals use the edit permission of the proposal's thing (C1 says
  which). Today's `eligible_role = 'owner'` becomes `addressed_role_keys = ['owner']` for copied
  rows and for the built-in host's approvals until a workspace setting picks another default.
  Any role or group can be named, which is "a role, not a fixed owner".
- **Raising.** `request_open` (operation) or an `interrupt` activity. Any account with
  `requests.raise` (held by every starting role) on any host can raise one; a host raises for
  the agent it runs. Limit: 20 open requests per raiser per addressee (today's
  `MAX_PENDING_PER_PERSON`, `src/questions.rs:37`), 200 open per run.
- **Inbox cards.** Opening a request creates notifications (kind `request`) for each named
  account and, for role-addressed requests, for each role holder who may answer; the card is
  W1's request card piece (W1.24, ruling R17) in C2's inbox (C2.12), drawn from
  `requests::card(request_id, viewer) -> RequestCard {title, detail, options,
  allowText, raisedBy, run?, subject?, canAnswer, expiresAt}`. Answering marks every copy
  read. The same commit emits `request.opened`, one event per addressed account (K3's
  `audience_account_id`; role- and group-addressed requests fan out to the holders who may
  answer), which A2 delivers to agents as reason `request_for_you`.
- **Answering.** `request_answer {requestId, optionId?, text?, expectedVersion}`; final once
  given. The answer is delivered three ways, all from the same commit: a `request.answered`
  event in K3's log (A2 delivers it to the raiser's connection; the legacy
  `question.answered` external event is kept as an alias for a release), an `answer` host
  command when `raised_via_host_id` is set, and an `interrupt_resolved` activity on the run.
  For a proposal, `request_answer` calls C1's `proposal_decide {proposalId, decision: accept
  | reject}` and does nothing else (R26): C1 applies the prepared change (`Proposable::apply`
  in a K2 dispatch, part of it if part is stale) and resolves the request; `proposal_decide`
  from C1's own review panel resolves it the same way, so either path closes both.
- **Expiry.** A raiser or host may set `expiresInSeconds`; a sweeper job expires due
  requests (`on_expiry = 'deny'` sends a deny answer to the host). Copied rows keep no expiry
  (today's behaviour).
- **Withdrawal.** `request_cancel` by the raiser, or automatically when its run finishes
  (today `cancel_terminal_run_approvals`, `src/recovery.rs:284`).
- **Operations.** `request_open`, `request_answer`, `request_cancel`, `request_get`,
  `request_list {role: "raised" | "for_me", status}`. Today's `approval_list`,
  `approval_get`, `approval_respond`, `ask_person`, `question_list`, `question_answer` and
  the HTTP approval routes stay as thin wrappers with their current shapes. Every tool
  description stops saying "Codex".

**Migration from the old tables.** `agent_approvals` and `person_questions` rows are copied
into `requests` keeping their ids (so `notifications.approval_id`, push actions and links keep
working), with `legacy_source` set. From A1.7 on, every writer writes `requests` and, in the
same transaction through `requests::legacy`, the old row with the same id; readers move to
`requests`; in A1.22 the old tables become read-only views over `requests` and the legacy
writes stop. The drive loop's polling (`src/provider.rs:1016-1026`) reads `requests` by id
until A1.14 replaces it with the `answer` command.

**Runs and requests raised from a DM (R20).** A run launched from a DM or group conversation
in the direct scope, and the requests it raises or that are raised in that conversation,
carry the direct scope's id as `workspace_id` and `space_id = NULL`. They are visible to the
conversation's participants and to nobody else: `runs::store::VISIBLE` and `requests`'
visibility take the conversation branch (R3's `conversation_member_visible`) and never a
role, so no admin, workspace owner or staff tool reads them. The agent doing the work must be
a participant. Budgets and metering for such a run count against the account that launched it
(H1's rule for the direct scope); a task or page made from its results goes to a space the
person picks (R20). A kept-DM workspace (`direct_messages = kept`) uses its own id as usual.

### 3.9 The host protocol

A **host** is a system that runs agents. In K1's terms it signs in as an **app account**
whose `hosts` row says which agent accounts it may act for. Choice: making a host an account
gives it the same credentials, audit and revocation as any app, instead of a second
credential system.

**Who owns a host (R19).** Like every app account, a host has an owner: a person ("my
machine") or a workspace ("the team's host"), set at enrolment. A person links their own host
once and may offer its agents in any workspace where they may add agents (K1's add-member
rule, A2's add flow); a workspace links a host for everyone there. So `hosts` and
`host_agents` are global tables, like `accounts` (they are on H1.3's list of global tables),
and the workspace a piece of work belongs to is on the run, the request and the host
command, never on the host.

**What is fixed and what is the host's (R21).** The fixed protocol is only what Workspace's
own records need: **enrol, describe, start, cancel, steer, answer, report** (runs,
activities, usage, status; 3.7) and **three notices** from the host (an agent's profile
changed, its reference changed, it was retired). Everything else a host can do is in two
generic lists in `describe`: **offers** (ways to get a new agent, each with the host's own
steps) and **actions** (things a person can do to one of its agents, or to the host). Workspace
draws them and sends the choice back; it never knows what an offer or action means. `inspect`
stays as an optional part of `start` (it answers "did you start this?" for uncertain starts).

```sql
CREATE TABLE hosts (                     -- global (R19): on H1.3's list of global tables
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL UNIQUE REFERENCES actors(id),   -- the host's app account
    owner_account_id UUID REFERENCES actors(id),             -- a person's host, or
    owner_workspace_id UUID REFERENCES organizations(id),    -- a workspace's host
    CHECK (num_nonnulls(owner_account_id, owner_workspace_id) = 1),
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80 AND name !~ '[[:cntrl:]]'),
    software TEXT CHECK (software IS NULL OR char_length(software) <= 80),
    software_version TEXT CHECK (software_version IS NULL OR char_length(software_version) <= 40),
    protocol_version TEXT NOT NULL,
    capabilities JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (
        jsonb_typeof(capabilities) = 'object' AND octet_length(capabilities::text) <= 32768),
    offers JSONB NOT NULL DEFAULT '[]'::jsonb,      -- [{id, title, summary, start}] as described (R21)
    actions JSONB NOT NULL DEFAULT '[]'::jsonb,     -- host-level [{id, label, needs, danger}]
    issuer TEXT,                                    -- K1 outside-reference issuer this host vouches for
    module_manifest_url TEXT,                       -- M1 connected-module manifest, installed at enrolment
    connection TEXT NOT NULL DEFAULT 'dial_out' CHECK (connection IN ('dial_out', 'push')),
    push_url TEXT CHECK (push_url IS NULL OR push_url ~ '^https://[^[:space:][:cntrl:]]+$'),
    push_secret_id UUID,            -- signing secret, held in crate::secrets
    host_token_secret_id UUID,      -- token Workspace presents when it calls the host
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'offline', 'revoked')),
    builtin BOOLEAN NOT NULL DEFAULT false,
    report_interval_seconds INTEGER NOT NULL DEFAULT 120,
    capacity JSONB NOT NULL DEFAULT '{}'::jsonb,
    last_seen_at TIMESTAMPTZ,
    last_command_seq_fetched BIGINT NOT NULL DEFAULT 0,
    enrolled_by_account_id UUID NOT NULL REFERENCES actors(id),
    revoked_at TIMESTAMPTZ, revoked_by_account_id UUID REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE host_enrolments (                -- global, like hosts
    id UUID PRIMARY KEY,
    owner_account_id UUID REFERENCES actors(id),
    owner_workspace_id UUID REFERENCES organizations(id),
    CHECK (num_nonnulls(owner_account_id, owner_workspace_id) = 1),
    code_hash BYTEA NOT NULL UNIQUE,                 -- SHA-256 of the code; the code is shown once
    label TEXT CHECK (label IS NULL OR char_length(label) <= 80),
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    expires_at TIMESTAMPTZ NOT NULL,
    used_at TIMESTAMPTZ, host_id UUID REFERENCES hosts(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE host_agents (                         -- which agent accounts a host runs, under which ref; global
    host_id UUID NOT NULL REFERENCES hosts(id),
    account_id UUID NOT NULL REFERENCES actors(id),
    host_agent_ref TEXT NOT NULL CHECK (char_length(host_agent_ref) BETWEEN 1 AND 200),
    backends JSONB NOT NULL DEFAULT '[]'::jsonb,     -- [{id, name, hidden}] as the host describes them
    actions JSONB NOT NULL DEFAULT '[]'::jsonb,      -- this agent's [{id, label, needs, danger}] (R21)
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'paused', 'gone')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (host_id, account_id),
    UNIQUE (host_id, host_agent_ref)
);

CREATE TABLE host_commands (
    id UUID PRIMARY KEY,
    seq BIGINT GENERATED ALWAYS AS IDENTITY UNIQUE,   -- the host's cursor
    workspace_id UUID NOT NULL REFERENCES organizations(id),  -- where the work is (may be the direct scope, R20)
    host_id UUID NOT NULL REFERENCES hosts(id),
    run_id UUID REFERENCES runs(id),
    request_id UUID REFERENCES requests(id),
    kind TEXT NOT NULL CHECK (kind IN ('start', 'cancel', 'steer', 'answer', 'inspect', 'action')),
    start_key TEXT,                                   -- stable for a run's start: never changes
    body JSONB NOT NULL CHECK (jsonb_typeof(body) = 'object' AND octet_length(body::text) <= 131072),
                                                      -- never holds a secret
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN (
        'pending', 'fetched', 'acked', 'refused', 'uncertain', 'expired', 'superseded')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_offer_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ack_due_at TIMESTAMPTZ,
    ack JSONB, acked_at TIMESTAMPTZ,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (host_id, start_key)
);
CREATE INDEX host_commands_due ON host_commands (host_id, seq) WHERE state IN ('pending', 'fetched', 'uncertain');
```

**Enrolment (one-time code, credentials both ways).**

1. A workspace member with `hosts.manage` presses "Link a host" (Settings, Hosts), which
   makes a workspace-owned host; or any signed-in person presses "Link a host" in their own
   settings, which makes a host they own (R19; no permission is needed to own one, only to
   offer its agents somewhere). Workspace creates a `host_enrolments` row naming the owner and
   shows the code once, for 15 minutes: `AWH-7Q4K-9MZP-2RXD` (80 bits), plus the server's
   address.
2. The host's administrator approves on their side by running the host's own enrol step
   with the address and the code. Nothing is approved by Workspace alone, and Workspace never
   holds the host's administrator login or SSH keys.
3. The host calls:

```http
POST /api/v1/hosts/enrol
Content-Type: application/json
AW-Host-Protocol: 2026-11

{"code": "AWH-7Q4K-9MZP-2RXD",
 "host": {"name": "Home lab", "software": "agent-host", "version": "1.4.0"},
 "protocolVersions": ["2026-11"],
 "connection": "dial_out",
 "push": null,
 "hostToken": null}
```

```json
{"hostId": "0199h…", "owner": {"kind": "workspace", "id": "0199w…", "name": "Acme", "address": "https://work.example/w/acme"},
 "protocol": "2026-11",
 "credential": {"kind": "client_credentials", "clientId": "app_0199…", "clientSecret": "aws_…",
                "tokenUrl": "https://work.example/api/v1/oauth/token"},
 "pushSigningSecret": null}
```

   For a push host (`"connection": "push", "push": {"url": "https://host.example/aw"},
   "hostToken": "…"`), Workspace stores the host's token through `crate::secrets` (used when
   Workspace calls the host) and returns `pushSigningSecret: "whsec_…"` (Standard Webhooks).
   So both sides hold a narrow credential they can revoke. K1 issues the app credential
   (K1.22 credentials, K1.23 client credentials) with the owner set as R19 describes;
   before K1, A1.11 issues an interim hashed bearer token for the host's `integration` actor
   and K1 migrates it. If the host's first `describe` names a module manifest, enrolment
   calls M1's `connected_module_install` (M1.12) so its panels can be drawn (R7).

**Describe.** On connect and whenever it changes, the host sends:

```http
POST /api/v1/host/describe
Authorization: Bearer <host token>
AW-Host-Protocol: 2026-11

{"software": {"name": "agent-host", "version": "1.4.0"},
 "issuer": "https://host.example",
 "module": {"manifestUrl": "https://host.example/aw/module.json"},
 "capabilities": {
   "start": true, "cancel": true, "steer": false, "answer": true, "inspect": true,
   "idempotentStart": true, "adoptsOwnRuns": true,
   "reportsSteps": "best_effort", "reportsUsage": "window", "relaysApprovals": false,
   "maxConcurrentRuns": 4},
 "reportIntervalSeconds": 120,
 "offers": [{"id": "new", "title": "New agent", "summary": "Set one up on this machine",
             "start": {"panel": "new-agent"}},
            {"id": "existing", "title": "One you already run here", "summary": "Pick from this host's agents",
             "start": {"page": "https://host.example/aw/pick"}}],
 "actions": [{"id": "sign-ins", "label": "Model sign-ins", "needs": "panel"}],
 "agents": [{"hostAgentRef": "fixer", "name": "Fixer",
             "ref": {"issuer": "https://host.example", "id": "fixer", "lineage": "l-01"},
             "backends": [{"id": "b1", "name": "Coding backend", "hidden": false}],
             "actions": [{"id": "pause", "label": "Pause", "needs": "confirm"},
                         {"id": "retire", "label": "Retire", "needs": "confirm", "danger": true}]},
            {"hostAgentRef": "scout", "name": "Scout", "backends": [{"id": "b2", "hidden": true}]}]}
```

`offers` and `actions` are generic lists (R21): an offer is `{id, title, summary, start}`
where `start` is a panel (M1's block format, from the installed module) or a page of the
host's from an origin approved at enrolment; an action is `{id, label, needs: confirm |
panel, danger?}`. Workspace does not know what any of them mean ("from a template", "copy
of an agent", "describe it in a sentence" and "model sign-ins" are all the host's words). A
backend's name may be withheld (`hidden: true`): Workspace then shows "a backend" and never
asks.

Only `start` and reporting are required; everything else is optional, and the UI shows what
a host cannot do ("This host can't take guidance mid-run") instead of pretending. A described
agent is bound to an account in `host_agents` only when an account that may add members with that role approves it (R45): through A2's
generic `agent_register` operation (the host proposes an account with a profile, an outside
reference and how it is reached; an account with the permission approves and picks the role,
A2.19), which is also how "pick one a host already has" works, or by `host_agent_bind` on an
account that already exists. The host never chooses which account it acts as.

**Offers and actions, at run time.** Choosing an offer draws its `start` panel or opens its
page; whatever steps follow are the host's, and they end with the host calling
`agent_register`. Choosing an action writes an `action` host command
`{actionId, agent?: {accountId, hostAgentRef}, input?, requestedBy}`; a `panel` action's ack
returns a panel to draw (64 KiB at most), a `confirm` action's ack returns `done` or an
error. The person needs `agents.manage` on that agent (or `hosts.manage` for a host-level
action) in the workspace they are in; the command carries that workspace.

**Notices from the host (R21).** `POST /api/v1/host/notices` (operation `host_notice`) with
one of three kinds; each needs the agent to be bound to this host:

```json
{"kind": "agent_profile_changed", "hostAgentRef": "fixer", "profile": {"name": "Fixer", "avatarUrl": "…", "summary": "…"}}
{"kind": "agent_ref_changed", "hostAgentRef": "fixer", "newHostAgentRef": "fixer-2",
 "ref": {"issuer": "https://host.example", "id": "fixer-2", "lineage": "l-01"}}
{"kind": "agent_retired", "hostAgentRef": "fixer", "reason": "retired by its owner"}
```

A profile change updates the account's profile through K1 (shown to its managers as "updated
by its host"); a reference change keeps the same account when the lineage matches (K1.7's
`agent_ref_rebind`, so identity survives a move or re-key) and otherwise opens a request to
the account's managers; a retirement sets the binding `gone`, interrupts active runs with "the
host retired this agent", and tells the managers, who decide whether to remove the account
from their workspaces. Each notice emits `host.notice` to the account's managers.

**Commands to a host.** Workspace writes `host_commands`; the host gets them one of two ways:

- **Dial-out** (one connection per host for all its agents):
  `GET /api/v1/host/commands?after=1042&wait=30` long-polls and returns
  `{"commands": [{"commandId": "…", "seq": 1043, "kind": "start", "runId": "…"}], "next": 1043}`.
  With `Accept: text/event-stream` the same address streams
  `id: 1043` / `event: host.command` / `data: {…}` and resumes from `Last-Event-ID`. A2's
  event stream also carries thin `host.command` events, so a host may use one stream for
  everything; A1's endpoint works without it.
- **Push**: Workspace `POST`s to `push_url` with Standard Webhooks headers
  (`webhook-id`, `webhook-timestamp`, `webhook-signature: v1,…`) and a thin body
  `{"type": "host.command", "commandId": "…", "kind": "start", "runId": "…"}`. The host
  answers 2xx at once and fetches the command.

Either way the full command is fetched with `GET /api/v1/host/commands/{commandId}`.
Short-lived secrets are minted at that moment and returned only in that response, never
stored (`host_commands.body` has none):

```json
{"commandId": "0199c…", "seq": 1043, "kind": "start", "runId": "0199a…",
 "startKey": "run-0199a…", "issuedAt": "2026-11-14T10:00:00Z", "expiresAt": "2026-11-15T10:00:00Z",
 "agent": {"accountId": "0199g…", "hostAgentRef": "fixer"},
 "input": {"prompt": "Fix AWD-142: …",
           "attachments": [{"fileId": "0199f…", "name": "trace.txt", "url": "https://work.example/api/v1/files/0199f…?sig=…&exp=…"}]},
 "cause": {"kind": "task", "taskId": "0199t…", "taskKey": "AWD-142", "conversationId": "0199c…"},
 "requestedBy": {"accountId": "0199u…", "name": "Dana"},
 "settings": {"backend": "b1", "model": "model-a", "reasoning": "high"},
 "repository": {"url": "https://code.example/acme/app.git", "baseRef": "main", "branch": "agent/awd-142",
                "token": {"value": "…", "expiresAt": "2026-11-14T11:00:00Z", "access": "read_write"}},
 "workspaceCredential": {"token": "…", "expiresAt": "2026-11-14T11:00:00Z",
                         "scope": "run:0199a… task:0199t… conversation:0199c…"},
 "reportTo": {"mcp": "https://work.example/api/v1/mcp", "api": "https://work.example/api/v1"}}
```

Other kinds: `cancel {runId, reason, requestedBy}`, `steer {runId, text, from}`,
`answer {runId?, requestId, hostRequestRef, answer: {optionId?, text?}, answeredBy}`,
`inspect {runId, startKey}` ("do you know this run?"), and `action {actionId, agent?, input?,
requestedBy}` for an action the host published (the reply comes back in the ack, bounded to
64 KiB). There is no fixed lifecycle list: what A2 once named `host_agent_*` are now the
host's own actions and offers (R21).

**Acknowledgement.**

```http
POST /api/v1/host/commands/{commandId}/ack
{"outcome": "accepted", "hostRunRef": "run-7f3a"}
{"outcome": "refused", "reason": "agent_unknown", "message": "No agent called fixer here"}
{"outcome": "uncertain", "message": "the controller did not confirm"}
{"outcome": "done", "result": {...}}            // for inspect and action
```

**Uncertain acceptance.** A start whose ack says `uncertain`, or that was fetched (or pushed)
and not acknowledged by `ack_due_at` (60 s), puts the run in `start_uncertain`. Then:

- If the host declared `idempotentStart`, Workspace offers **the same command** again (same
  `commandId`, same `startKey`) at 30 s, 2 min, 10 min, 30 min, then hourly until
  `expires_at` (24 h). The host must treat a known `startKey` as "already started" and
  ack `accepted` with its run ref. Workspace never issues a second, different start for the
  run: `UNIQUE (host_id, start_key)` makes that impossible.
- If not, Workspace does not re-offer. It sends `inspect` if the host supports it; otherwise
  the run view shows "We don't know whether <host> started this" with **Check again**
  (`inspect`) and **Mark as not started** (`run_mark_not_started`). Starting again is a new
  run, with this one as its predecessor, by a person's choice.
- Any later report for the run (activity, update, heartbeat listing it) moves it to
  `accepted` or `working`, even after "mark as not started", with a `custom
  {name: "late_start"}` activity saying so.

**Reports back.** By the run operations of 3.7, authenticated as the host account
(`Reporter::Host`), or by a connector beside the host that reads the host's own log and
reports for it (A2.25's bridged connector) with the host's credential. Both are the same
calls.

**Heartbeat.**

```http
POST /api/v1/host/heartbeat
{"at": "2026-11-14T10:00:30Z", "capacity": {"slots": 4, "busy": 1},
 "agents": [{"hostAgentRef": "fixer", "status": "active"}],
 "runs": [{"runId": "0199a…", "hostRunRef": "run-7f3a", "state": "working"}]}
-> {"protocol": "2026-11", "deprecated": false, "describeWanted": false}
```

A long-poll counts as a heartbeat. No heartbeat or poll for three intervals sets the host
`offline` and its active runs' liveness to `not_responding`. Listing a run in `runs` renews
its liveness; a run Workspace thinks is active but the host stops listing for three
heartbeats becomes `interrupted` ("the host no longer knows this run").

**Revocation.** Whoever manages the host's account unlinks the host (R34's rule: its owning
person; a holder of `hosts.manage` in its owning workspace, the host counterpart of
`agents.manage_any`; or a manager named on the account who holds `hosts.manage`): K1 revokes its credentials,
the host row becomes `revoked`, pending commands become `expired`, active runs on it become
`interrupted` with "The host was unlinked", open requests raised through it are
`cancelled`. The host's next call gets `401 {"error": "host_revoked"}`.

**Versioning.** Every call carries `AW-Host-Protocol: <date version>`. The server serves the
current and the previous version; `describe` and heartbeat replies say when the host's
version is deprecated. Additions are optional fields that old hosts ignore; anything else is
a new date version. Unknown capability keys are kept and ignored.

**Paths.** Host-side endpoints are operations in K2's registry (`host_enrol`,
`host_describe`, `host_command_list`, `host_command_get`, `host_command_ack`,
`host_heartbeat`, `host_notice`), so each also exists as an MCP tool, which lets a host
written as an MCP client use one transport. The paths above are their public API routes.
A host call that touches a run or request is checked in that run's workspace; a host owned by
a person reaches a workspace only through agents bound to it that are members there.

**What proves the protocol is enough.** Three pieces, all built on `crates/host-protocol`
(wire types and a client, created in A1.12): the **built-in host** (A1.14, A1.16, A1.17),
a small **example host** (`examples/minimal-host`, A1.23: one echo agent, one offer, one
action, every notice), and a **conformance suite** any host can run against a server and the
server runs against any host (`crates/host-conformance`, A1.23, grown from A1.12's test
host). None of them is named in core; none waits on any outside project.

### 3.10 The A2A binding (later slice)

For an agent platform that speaks A2A instead of this protocol, a **binding** inside the
`hosts` module turns host commands into A2A calls and A2A updates into run operations. A2A
JSON stays inside `src/hosts/bindings/a2a.rs`. Workspace is the A2A client; it runs nothing.

| A2A | Workspace |
|---|---|
| Agent Card (`/.well-known/agent-card.json`, signature checked) | `describe`; skills become `host_agents`; auth schemes become the enrolment's stored token |
| `message/send` (or `message/stream`) with `messageId = startKey` | `start`; the returned task id is `host_run_ref` |
| Task `submitted` | `accepted` |
| `working` | `working`; status messages become `text_message` activities |
| `input-required` | `waiting`; opens a `question` request from the status message; the answer is sent as a message to the same task |
| `auth-required` | `waiting`; opens an `approval` request whose detail links to the platform |
| `completed` / `failed` / `canceled` / `rejected` | `done` / `failed` / `cancelled` / `refused` |
| `unknown`, or no response to `send` | `start_uncertain`; `tasks/get` is `inspect` |
| Artifacts | `result` activities: text parts as `text_message`, file parts stored as files, data parts as `result {kind: "data"}` |
| `tasks/cancel` | `cancel` (the run is `cancel_requested` until the task reports `canceled`) |
| Push notification config | Workspace registers its own receiver URL per host; SSE `tasks/resubscribe` when the platform prefers streams; `tasks/get` polling otherwise |
| Extensions | A small published extension carries `repository`, `branch` and `pullRequest` in message metadata |

### 3.11 The built-in host leaves the server

What stays in the server: the run record and everything in 3.2 to 3.9, the Launch UI and
queue, project environments (as data), capacity display (from host heartbeats), the code
host and pull request records. What leaves: `provider/`, the turn driver in `provider.rs`,
`scheduler.rs` admission, `recovery.rs`, `git_runtime.rs` mirrors and worktrees,
`harness_host.rs` secrets for provider accounts, `profile_runtime.rs`, provider account
sign-ins and their usage windows, and the tables listed as built-in-only in 2.3.

**Step (a): wrap, in process (A1.14).** A `Host` trait in `src/hosts/dispatch.rs`:

```rust
#[async_trait::async_trait]
pub trait Host: Send + Sync {
    fn host_id(&self) -> Uuid;
    async fn describe(&self) -> HostDescription;
    async fn start(&self, command: StartCommand) -> CommandAck;   // Accepted{host_run_ref} | Refused | Uncertain
    async fn cancel(&self, command: CancelCommand) -> CommandAck;
    async fn steer(&self, command: SteerCommand) -> CommandAck;
    async fn answer(&self, command: AnswerCommand) -> CommandAck;
    async fn inspect(&self, command: InspectCommand) -> CommandAck;
}
```

`BuiltinHost` implements it by calling today's scheduler admission, worktree preparation and
`provider::start_run`, unchanged. It reports through the run service as `Reporter::BuiltinHost`:
`drive_turn` events become `runs::append` calls (steps, notes, usage), approvals become
`requests` raised by the run, and the 300 ms polling for answers and Stop
(`src/provider.rs:878-887`, `:989-1077`) is replaced by `answer` and `cancel` commands
delivered to the driver over an in-process channel per run. `RemoteHost` implements the
trait by writing `host_commands`. The built-in host gets a `hosts` row (`builtin = true`)
created by migration per workspace and owned by that workspace (R19), and the
execution-worker accounts are bound to it in `host_agents`.

**Step (b): only the public protocol (A1.16).** The built-in host code moves to a crate
`crates/builtin-host` that depends on a small `crates/host-protocol` crate (wire types and a
client), not on the server crate. In the server process it runs as a task that enrols itself
on first start (a code minted internally), long-polls `/api/v1/host/commands` over loopback
HTTP, and reports with the run operations over HTTP. The server crate keeps no call into the
provider code except spawning that task, which only a process holding K5.3's `builtin-host`
lease does (`AGENT_WORKSPACE_BUILTIN_HOST=on`, K5's variable and default). The diff is
reported as a `diff` result file when the run finishes, and offered live while the host is
online as a run action the built-in host publishes ("Current changes", `needs: panel`),
replacing the live read in `src/run_views.rs:1-8`. A switch
`AGENT_WORKSPACE_BUILTIN_HOST_TRANSPORT=inprocess|protocol` keeps (a) available for one
release as a rollback.

**Step (c): its own program (A1.17).** The same crate builds an `agent-workspace-host`
binary. The built-in-only tables move to a `host_builtin` schema with the host's own
migrations (`ALTER TABLE … SET SCHEMA`, no copy); on a self-hosted install the host may use
the same PostgreSQL server with that schema, or its own database. The server enrols it
automatically on a self-hosted install: at bootstrap the server writes a one-time code to a
file both containers share (or prints it), and the host reads it on first start. Screens that
manage provider accounts and harness registrations become the built-in host's own
host-level actions and offers (R21: "Model sign-ins", "Backends", "New agent"), drawn as
host panels (M1's connected-module panel format, M1.14) instead of reading tables; the
server has no list of their names. The server's default becomes
`AGENT_WORKSPACE_BUILTIN_HOST=off` with the separate program; `on` (embedded) remains for one
release. H1's compose `host` profile ships this program (H1.13).

**Step (d): coding agents through ACP (A1.21, optional).** The built-in host gains an
`acp` adapter that implements `HarnessAdapter` by speaking ACP over stdio to any ACP agent,
with plans, tool calls, diffs and permission requests mapped onto `HarnessEvent`s. Existing
adapters stay; ACP is one more, offered per backend when it reaches parity in
`provider/contract_tests.rs`.

**In-flight runs at each step.**

| Step | During its deploy | After |
|---|---|---|
| (a) | Same process and same tables; runs recover as today, with leases (below) | Unchanged deploy behaviour |
| (b) | On restart the embedded host recovers its runs from its tables (same code) and reports through the protocol; the run ids and records don't change | A server deploy still restarts the embedded host |
| (c) | The schema move runs while both are stopped; the host recovers on start as in (b) | A server deploy no longer touches running agents; a host deploy shows its runs `not_responding` until it reconnects, then they continue or are interrupted by the host's own recovery |

**Recovery with leases (K5.3, A1.9).** K5.3 puts the whole built-in host behind one
single-holder lease `builtin-host`: only the holder runs recovery, the scheduler and
repository preparation (as a host job), so a second process or a rolling deploy can no
longer take another process's live runs (G report F4, D report F3). A1 does not add a second
lease scheme. A1.9 keeps only what K5.3 does not cover: `fail_run` failures no longer
ignored (`src/provider.rs:442`, handed over by P0), and recovery taking detail rows with
`FOR UPDATE SKIP LOCKED` so a lease hand-over mid-recovery cannot double-resume. If K5.3 has
not merged, A1.9 adds `agent_runs.driver_instance_id` and `driver_lease_expires_at` as the
stopgap and removes them once K5.3 lands.

### 3.12 Launching: one path

Every launch (`launch::start_launch`, `src/launch.rs:1958`; tags; automations; MCP
`task_launch`) does:

1. Resolve the **agent account** (from the agent profile, the environment or the tag) and
   its route: a `host_agents` binding (a host, built-in or not), or none (the agent connects
   by itself).
2. `runs::create` in the launch's transaction, state `queued`, with cause, requester, task,
   thread, settings and title. The launch row gets `run_id` and state `started`. For a task
   launch, the same transaction sets the task's delegate to the agent through T1's task
   command (the hook T1.22 relies on; its `task.delegated` event is T1's).
3. When the queue lets it go (blockers, start time, host capacity from heartbeats; the
   built-in host's account slots until (c)), dispatch. Dispatch first calls H1's
   `metering::admit` for the workspace (or, in the direct scope, the launching account); a
   refusal leaves the run `queued` with "Paused: spending limit reached", never fails it.
   - **A host**: a `start` host command (`state = starting`).
   - **No host**: deliver `run.start` to the agent through A2's event stream (`why:
     "assigned"`; payload `{runId, accountId, cause, startKey, inputRef:
     "/api/v1/runs/{id}/start-input"}`, the input fetched by `run_start_input` with secrets
     minted at fetch, 3.7), and, during the change and for agents without A2's stream, **post the same
     hand-off message as today** (`src/launch.rs:2140-2179`: the mention, the prompt, the
     settings line), so existing connected runners behave exactly as before. The run's
     `cause_message_id` is that message, which is how `reply_claim` finds the run (3.7).
     The run starts in `accepted`; liveness applies, so a runner that never answers shows
     "not responding" instead of nothing.
4. Tag dispatch (`src/launch.rs:2676-2743`) stops branching on `execution_worker`: any agent
   account with `runs.launch`-reachable routes can be launched by a tag, with the existing
   agent-limit checks.

`launches.state = 'handed_off'` stays valid for old rows; new launches use `started` with a
`run_id`. The web's "Handed off" label (`web/launch/launchModel.ts:40`, `:59`) keeps working
for old rows and new ones show the run.

### 3.13 One agent identity across hosts

What A1 needs from K1:

1. **One agent account kind** replacing `execution_worker` and `external_agent`, so the same
   account can be run by the built-in host, by any other host or by itself, and moving it
   only changes its `host_agents` row. The 0033 trigger `validate_external_agent_connection`
   (agent must be an `external_agent` member or viewer, created by a human;
   `migrations/0033_m9_external_agent_connections.sql:60-101`) is a kind rule too and is
   dropped in K1.6; the `runtime_kind = 'hermes'` check was already relaxed by
   `migrations/0037_direct_conversations.sql:183-191`.
2. **App accounts for hosts**, global with an owner (R19: a person or a workspace), with
   client credentials, revocation, and "acts for" grants limited to the agent accounts in
   `host_agents` (R7). A host acts for an agent through K1.23's `Via::ActingFor`; A1's
   `hosts::acting_for(host_account, agent_account)` is the one place that builds that `Via`
   from an active `host_agents` row, so no host operation checks bindings by hand.
3. **Run-scoped short-lived tokens** for an agent account ("may work on this run's task,
   thread and repository results for the next hour"), minted by A1 at command fetch.
4. **The outside reference** on agent accounts (issuer, external id, lineage); A1 uses it to
   suggest a `host_agents` binding when a host describes an agent K1 already knows.
5. **Catalogue entries**: `runs.report`, `requests.raise`, `requests.answer`,
   `hosts.manage` (added to K1's catalogue in K1.14); K1's `requests.answer_sensitive`
   (replacing `human_required`) and `replies.steer` (Stop and Continue) used as they are;
   existing `runs.launch`, `runs.approve`, `runs.interrupt` kept.
6. **Kind rules removed** (2.6), each replaced: approvals by `runs.approve` held or not by a
   role; asking an agent by `requests.answer`; `run_start` and controls by `runs.launch` and
   `runs.interrupt`; `run_request` and `session_bind` by `runs.report`; reply tools by
   "the request is addressed to you"; `made_through_app` for any account; tag launches for
   any agent account.

Before K1 lands: `runs.account_id` is whatever actor does the work today (the
`execution_worker` actor for built-in runs, the `external_agent` actor otherwise);
`host_agents` binds `external_agent` actors to hosts; hosts sign in as `integration` actors
with an interim token; the kind rules in 2.6 that sit inside A1's modules are rewritten as
permission checks using the existing `Permission` enum (`src/policy.rs:195-221`) with new
variants added beside related ones.

### 3.14 The code-host seam

GitHub-specific today: `repositories.github_repository_id` and installation columns
(`migrations/0002_walking_skeleton.sql:52-67`), URL building and validation
(`src/repositories.rs:312`, `:779-798`), the push-remote check (`src/git_runtime.rs:461`),
`github_pull_requests` with a `github.com` URL check (`migrations/0023:13-16`), read
directly in 18 files, PR-driven task moves with their own `UPDATE work_items`
(`src/github.rs:4096-4335`), `HarnessHost::repository_access` ("the GitHub repository a
run's worktree belongs to", `src/provider/host.rs:16-46`), and the GitHub job queue
(`src/github.rs:2321-2480`).

```rust
// src/code_host/mod.rs
pub struct RepoRef { pub code_host: String, pub external_id: String, pub full_name: String }
pub enum TokenAccess { Read, ReadWrite }
pub struct ScopedToken { pub value: SecretValue, pub expires_at: DateTime<Utc> }

#[async_trait::async_trait]
pub trait CodeHost: Send + Sync {
    fn kind(&self) -> &'static str;
    fn parse_url(&self, url: &str) -> Option<RepoRef>;
    fn is_safe_remote(&self, url: &str) -> bool;            // replaces git_runtime.rs:461
    async fn repository_token(&self, repo: &Repository, access: TokenAccess, ttl: Duration) -> Result<ScopedToken, AppError>;
    async fn open_pull_request(&self, repo: &Repository, spec: PullRequestSpec) -> Result<PullRequestSnapshot, AppError>;
    async fn read_pull_request(&self, repo: &Repository, number: i64) -> Result<PullRequestSnapshot, AppError>;
    async fn pull_request_by_url(&self, url: &str) -> Option<(RepoRef, i64)>;
    async fn verify_webhook(&self, headers: &HeaderMap, body: &[u8]) -> Result<CodeHostEvent, AppError>;
}
```

`code_host::github` implements it by calling today's functions in `github.rs` (no behaviour
change). `repositories` gains `code_host TEXT NOT NULL DEFAULT 'github'` and
`external_id TEXT` (copied from `github_repository_id`). A pull request read model
`code_host::pull_requests` (`get`, `for_run`, `for_task`, `for_conversation`, `open_for_run`)
reads `github_pull_requests` and is what the 18 files use. PR-driven task moves go through
`work_item_commands` (T1's task commands) instead of raw `UPDATE`.

**Who opens the pull request.** The host pushes its work, using the short-lived repository
token Workspace hands it on `start` (`repository.token`) or its own credentials. It reports
`result {kind: "pull_request", url}` if it opened one, or `result {kind: "branch"}` if not.
Workspace records: a PR URL is resolved through `pull_request_by_url` and synced into
`github_pull_requests` with a workspace link to the run, so checks, reviews, task automation
and notifications work as today. Workspace's own "Open pull request" and "Publish" actions
(`run_publish`, `pull_request_create`) stay: they open the PR through the code host's App
from a reported branch. No GitHub feature is removed; the built-in host keeps pushing exactly
as now until step (b), when it pushes with the start token itself.

### 3.15 The Runs page and run view

W1 supplies the list, table and detail pieces; A1 supplies data and the run-specific parts.
New parts register in `web/experience/registry.json` next to the existing `area.runs`,
`workspace.runs`, `task.runs`, `tech.run_usage` entries (registry lines 113, 225, 309, 358).

- **Runs list** (`web/runs/RunsPage.tsx`, lazy as today, `web/app/App.tsx:50-59`): every run
  in the workspace or space; columns: agent, title and cause (task key, thread, schedule,
  "started by its host"), host ("reports itself" when none; backend only if disclosed),
  state chip, liveness badge ("Not responding for 12 min"), waiting on (request title, with
  an Answer button when the viewer may answer), cost (exact, or "≈" with "estimated from
  usage windows" on hover), started, duration. Filters: state, agent, host, needs me, not
  responding.
- **Run view** (`web/runs/RunPage.tsx`): header with state, liveness, cancel (shows
  "Stopping…" for `cancel_requested`), Open session (`external_url`); the plan as a checklist;
  the activity timeline drawn from `run_activities` (step groups with their tool calls, text
  messages, errors, gaps drawn as "No reports from 09:01 to 09:03: process restarted");
  requests inline with answer controls; results (PR card through the code host, pages,
  files, diff); usage per model with its confidence. The "start uncertain" banner with
  Check again and Mark as not started.
- **Run state labels** (`web/runs/runModel.ts:14-25`) gain the new states, with the old ones
  kept for cached data.
- **Hosts** (Settings, new lazy page, registry `workspace.hosts`; the same page under a
  person's own settings lists the hosts they own, registry `account.hosts`): list, Link a host
  (code), capabilities, agents and their bindings, last seen, capacity, the host's
  host-level actions drawn as buttons or panels, Unlink. A host's offers are drawn by A2's
  add-an-agent flow and its agent actions on the agent's page (A2.19, A2.20), not here.
- **Agent sidebar and thread run cards** read `runs::thread_cards`, so a self-reporting
  agent's run shows in its thread like a built-in one.

### 3.16 Events

All written through K3's `events::emit(tx, …)` in the same transaction as the change (outbox until then): `run.opened`, `run.updated`, `run.activity`,
`run.finished`, `run.liveness_changed`, `run.result_added`, `run.usage_updated`,
`run.start` and `run.cancel` (to an agent that runs itself, 3.6), `request.opened`,
`request.answered`, `request.expired`, `request.cancelled`, `host.enrolled`,
`host.described`, `host.status_changed`, `host.revoked`, `host.notice`, `host.command`
(thin, for A2's stream; audience the host's app account). Every event carries its
`workspace_id` (the direct scope for DM-raised work), so a connection that reaches several
workspaces (R19) can tell them apart. K3's catalogue lists `request.created`; A1's name is
`request.opened` (requested of K3). Each names the run or request; visibility is the run's or the
request's subject's. Webhook and automation names for runs stay `run.updated` and
`run.finished` (`src/events.rs:37-38`).

---

## 4. Migration path

The app is live with one server and every migration since 2026-10-31 still to apply in one
go. Order of the data moves:

1. **Expand (A1.1).** Create `runs`, `run_activities`, `run_usage`, `usage_reports`. In the
   same migration, insert one `runs` row per `agent_runs` row (small: runs are created by
   hand, inferred to be thousands at most) with the state mapping of 3.2, `account_id` from
   `execution_workers.actor_id`, `workspace_id` from the project. Add triggers on
   `agent_runs` (`AFTER INSERT`, `AFTER UPDATE OF state, failure_reason, started_at,
   finished_at, …`) that upsert the matching `runs` row, and on `agent_run_events` and
   `agent_run_usage` (`AFTER INSERT/UPDATE`) that upsert `run_activities` and `run_usage`.
   This keeps every legacy writer in step without touching them. The triggers are written so
   they do nothing if the `runs` row was written by the run service in the same transaction
   (compare `version`), which lets writers move one at a time.
2. **Copy history (A1.1).** `run_activities` from `agent_run_events` is copied by a resumable
   job (`runs.backfill_activities`, batches of 5,000 by `agent_run_events.id >` the last id
   stored in a job checkpoint; `legacy_event_id UNIQUE` makes re-runs safe). `run_usage` from
   `agent_run_usage` in the migration (one row per run and model).
3. **Re-point foreign keys (A1.3, A1.8).** Once every `agent_runs` row has a `runs` row (true
   after A1.1's migration, which has an earlier timestamp), `notifications.agent_run_id`,
   `launches.agent_run_id` (plus a new `launches.run_id`), `agent_sessions.agent_run_id` and
   `messages (conversation_id, agent_run_id)` are re-pointed at `runs` with
   `NOT VALID` then `VALIDATE CONSTRAINT`. `notifications.approval_id` is re-pointed at
   `requests` in A1.7 after its copy. `agent_runs.id REFERENCES runs(id)` is added.
4. **Switch readers (A1.2, A1.6).** Readers move to the run service; behaviour is identical
   because the mirror keeps built-in rows equal.
5. **Switch writers (A1.3, A1.10, A1.14).** New reporters write only through the run service.
   Built-in writers move with the built-in host slices; while they still write `agent_runs`,
   the triggers mirror them.
6. **Requests (A1.7, A1.8).** Copy, then dual-write through `requests::legacy`, then move
   readers, then make old tables views (A1.22).
7. **Contract (A1.22).** Drop the mirror triggers once no code writes `agent_runs.state`
   outside the built-in host (which then writes `runs` through the service); replace
   `agent_approvals` and `person_questions` with views; stop writing `agent_run_events` (keep
   the table and its rows for old `conversation_entries`); delete the `cfg(any())` blocks in
   `api.rs` (D report F9); `agent_run_usage` becomes a view over `run_usage`.
8. **Built-in host data (A1.17).** Moved by `SET SCHEMA`, not copied.

Old and new coexist: the old tools (`run_get`, `approval_*`, `question_*`, `reply_progress`,
`session_bind`) keep their shapes; the old webhook names stay; `handed_off` launches stay
readable. Nothing is deleted before its last caller has moved, and each deletion is named in
A1.22.

### 4.1 Triggers A1 adds (R16)

| Table | Trigger | Writes | Added | Removed |
|---|---|---|---|---|
| `agent_runs` | `agent_runs_mirror` (`AFTER INSERT`, `AFTER UPDATE OF state, …`) | `runs` | A1.1 | A1.22 |
| `agent_run_events` | `agent_run_events_mirror` (`AFTER INSERT/UPDATE`) | `run_activities` | A1.1 | A1.22 |
| `agent_run_usage` | `agent_run_usage_mirror` (`AFTER INSERT/UPDATE`) | `run_usage` | A1.1 | A1.22 (the table becomes a view) |
| `organizations` | `organizations_builtin_host` (`AFTER INSERT`) | `actors` (one `integration` account), `hosts` (the built-in host row) | A1.11 | A1.17 (the built-in host is made where the server enrols itself) |

No other A1 table gets a trigger: `requests::legacy` dual-writes in code, not by trigger.
The built-in host trigger only inserts, once per workspace (`hosts_ensure_builtin`), so a
workspace made by any path (sign-up, bootstrap, a template, a test) has its built-in host.
Each mirror writes only its own target, and no other plan's trigger writes `runs`,
`run_activities` or `run_usage`. K3's thing sync for kind `run` must sit on `runs` (and for
kind `request` on `requests`), not on `agent_runs`, so that a legacy write does not reach
`things` by two routes (mirror then sync, and sync on `agent_runs`); this is requested of K3.
H1's `<table>_fill_workspace` triggers do not apply: A1's tables are born with
`workspace_id`.

---

## 5. Slices

Ordered for early value: outside agents get a visible run record in A1.1, A1.3, A1.4 and
A1.5 (the D report's steps 1 and 2), before the host protocol and the built-in host work.

### A1.1 Run record tables and mirror (M)

Create `runs`, `run_activities`, `run_usage`, `usage_reports` with the SQL of 3.2 to 3.4;
copy runs and usage in the migration; mirror triggers on `agent_runs`, `agent_run_events`,
`agent_run_usage`; the resumable activities backfill job; `src/runs.rs` becomes
`src/runs/mod.rs` with `state.rs` (new `RunState`, mapping from the old one, reducer) and
`store.rs` (row types). No reader changes.
- **Changes:** `migrations/<ts>_a1_runs.sql`, `src/runs/*`, `src/jobs.rs` registration
  (next to related lines).
- **Depends on:** nothing.
- **Acceptance:** after migration every `agent_runs` row has a `runs` row with mapped state
  (`tests/integration/a1_runs_record.rs`: seed old-shape runs in each of the 10 old states,
  migrate, assert the mapping); inserting and updating `agent_runs` updates `runs`
  (same file); the backfill job copies events once and is safe to run twice; the reducer
  unit tests in `src/runs/state.rs` cover every edge in the table of 3.2 and refuse others.

### A1.2 Run service reads, part 1, and the guard (M)

`runs::store::VISIBLE` and the read functions of 3.6 used by `conversations.rs`,
`notifications.rs`, `home.rs`, `projects.rs`, `mcp/admin.rs`, `repositories.rs`,
`work_items.rs`, `onboarding.rs`, `metrics.rs`, `events.rs`, `webhooks.rs`, `outbox.rs`,
`automations.rs`, `mcp.rs` (run scope). Replace pasted state literals. Add guard G1 with the
allow-list and G4.
- **Depends on:** A1.1. **Beside:** A1.3, A1.7.
- **Acceptance:** existing integration tests for threads, archiving, notifications, Home and
  webhooks pass unchanged; G1 and G4 pass; a new test in `a1_runs_record.rs` checks a run in
  a private thread is not counted or listed for a non-member.

### A1.3 Run operations for any agent (L)

`run_open`, `run_update`, `run_activity`, `run_usage_report`, `run_result`, `run_cancel`,
`run_mark_not_started` as operations (K2 if merged; otherwise tools plus routes in K2's
shape), with `Reporter` permissions, idempotency, limits and redaction; liveness sweeper
job; `run.*` events; `run_log::store_tokens` writes through `runs::usage`; re-point
`notifications`, `launches`, `agent_sessions`, `messages` foreign keys to `runs`; add
`launches.run_id`. Permission variants `runs.report`, `requests.raise` added next to the run
permissions in `src/policy.rs`. Isolation cases and agent-parity entries.
- **Changes:** `src/runs/{ops,activities,usage,results,liveness}.rs`, `src/mcp.rs` (tools
  next to `run_get`), `src/mcp/tool_permissions.rs`, `src/policy.rs`,
  `src/policy_inventory.rs`, `src/agent_parity.rs`, migration for FKs.
- **Depends on:** A1.1; K2.2, K2.3 (soft); K3 (soft, outbox until then); K5.6 shared limits
  (soft, in-process limiter until then).
- **Acceptance (`tests/integration/a1_run_reporting.rs`):** an `external_agent` opens a run,
  reports a batch, repeats the batch and gets `duplicates` with no new rows; another agent
  cannot report on it (`not_found`); an agent in workspace B cannot see or report on A's
  run; an `interrupt` opens a request and the run becomes `waiting`; `run_finished` makes it
  `done`; secrets in a payload are redacted; a run with no reports for three intervals
  becomes `not_responding` and back to `ok` on the next report; `workspace_isolation.rs` and
  `write_isolation.rs` cases for every new tool and route; G5 passes for `src/runs/`.

### A1.4 Connected runners get runs (M)

Launch to a runner harness creates a run (cause message = the hand-off message, state
`accepted`) and keeps posting the hand-off message; `reply_claim` returns `run`;
`reply_progress`, `reply_send`, `reply_release` and Stop map onto the run as in 3.7;
`session_bind` sets `external_url`/`backend` and its `harness` becomes optional;
`agent_sessions::harness_name` stops hard-coding names; SDK helpers and tests in CI;
`docs/agent-runner.md` section 6.
- Fixes the bug P0 hands over: an agent-initiated launch to a runner no longer fails in
  `hand_off` (`launch.rs:2160-2167`), because the hand-off message is posted by the run
  service as the system on the run's behalf.
- **Changes:** `src/launch.rs` (`hand_off`), `src/mcp/replies.rs`,
  `src/mcp/launch_tools.rs`, `src/agent_sessions.rs`, `sdks/runner-*`,
  `docs/agent-runner.md`, `tools/ci/checks.sh`.
- **Depends on:** A1.3.
- **Acceptance (`tests/integration/a1_launch_paths.rs`):** a launch to a runner gives a
  launch `started` with `run_id` and the same message text as before (compare against the
  existing hand-off test's expected text); a runner that only uses `reply_*` tools ends with
  a run `done` and a `step_started` per progress line; a runner using the old SDK version
  (no run calls) still works and its run shows `not_responding` if it never claims;
  `session_bind` without `harness` succeeds; SDK unit tests run in CI.

### A1.5 Runs page and run view for every agent (M)

Web pieces of 3.15 for runs (not hosts): list with host, cause, liveness, waiting on, cost
confidence; run view timeline from activities, plan, gaps, requests inline, results; new
states in `runModel.ts`. Uses W1's list and detail pieces where merged, the current ones
otherwise. API: `GET /api/v1/runs` and `/api/v1/runs/{id}` (new paths beside the existing
`/api/v1/agent-runs/{id}`, which stays), `GET /api/v1/runs/{id}/activities`. W1.24 later
extracts `RunView` from this run page into W1's shared piece without a rewrite (R17, R28) and
depends on this slice, so A1.5 does not wait on W1.24 (the cycle is broken by building in
place first).
- **Depends on:** A1.2, A1.3; W1 list and detail pieces (soft).
- **Acceptance:** `web/runs/runModel.test.ts` covers state, liveness and confidence labels;
  a browser check (`check-in-browser` skill) of a self-reported run with a gap and an open
  request, light and dark, narrow and wide; isolation cases for the new GET routes.

### A1.6 Run service reads, part 2, and neutral wording (M)

Move `run_views.rs`, `mcp/runs.rs`, `mcp/code.rs`, `agent_profiles.rs`, the reads in
`launch.rs` and `usage/history.rs` to the run service (built-in-specific account load stays
behind `hosts::builtin::capacity`). Rewrite run, approval and stage text without "Codex"
(`src/mcp.rs:3114-3116`, `:3152`, `:3260`, `:3577-3579`, `:3608`, `:3629`;
`src/mcp/runs.rs:141`, `:257-288`; `src/api.rs:288`), skipping any already reworded by
P0.32 (P0 does tool descriptions; A1 does the rest and the config names);
`HARNESS_HOST_CONCURRENCY_LIMIT` and `PROVIDER_HOME_ROOT` with the old names as aliases for a
release.
- **Depends on:** A1.2. **Beside:** A1.3 to A1.5.
- **Acceptance:** `run_list` and `run_get` return a self-reported run with its stage text;
  existing run tool tests pass; G6 passes over tool descriptions; config alias unit test.

### A1.7 Requests record (M)

`requests` table; copy `agent_approvals` and `person_questions` with the same ids; re-point
`notifications.approval_id`; `requests::legacy` dual write; every writer (`provider.rs`
approvals, `external_execution.rs`, `questions.rs`) goes through `requests::open` /
`requests::answer`.
- **Depends on:** A1.1. **Beside:** A1.2 to A1.6.
- **Acceptance (`tests/integration/a1_requests.rs`):** copied rows match field by field;
  answering through the old approval route and the old `question_answer` tool updates both
  rows; a built-in run's approval still unblocks the run (existing approval test passes).

### A1.8 Request operations and answering by role (L)

`request_open`, `request_answer`, `request_cancel`, `request_get`, `request_list`;
`may_answer` by permission, named accounts and roles; remove the kind rules for approvals
and questions (2.6) in favour of `runs.approve` / `requests.answer`; `ask_person` may
address any account; expiry sweeper; `request.*` events and inbox notifications (W1.24's request card draws
`requests::card` in C2.12's inbox; Accept and Reject on a proposal call C1's
`proposal_decide`); old tools become wrappers; move `conversations.rs`,
`notifications.rs`, `external_events.rs`, `home.rs`, `push/actions.rs`, `mcp/approvals.rs`
readers to `requests`.
- **Depends on:** A1.7; K1.13 (soft). The permission variants go into `policy.rs` here and
  K1.14 moves them into the catalogue; K1.14 depends on this slice, so A1.8 does not wait for
  it (cycle broken by the `policy.rs` stopgap); C1 `proposal_decide` (soft: proposal requests answer through the old route until
  then); K1.27 groups (soft, for `addressed_group_ids`). **Beside:** A1.9 to A1.13.
- **Acceptance (`a1_requests.rs`):** an agent whose role holds `runs.approve` can approve; a
  person whose role lacks it cannot; a question addressed to an agent can be answered by that
  agent; a request in a private thread is invisible and unanswerable for non-members; an
  answer produces one `request.answered` event and an `interrupt_resolved` activity; expiry
  with `deny` answers deny; isolation cases; default roles reproduce today's behaviour
  (owner answers built-in approvals; agents cannot unless granted); a request raised in a
  direct-scope DM is visible and answerable only by its participants, not by a workspace
  admin (R20).

### A1.9 Run leases and recovery (M)

What K5.3 does not: `fail_run` failures no longer ignored (`src/provider.rs:442`, handed
over by P0); recovery takes detail rows with `FOR UPDATE SKIP LOCKED`. The single-holder
`builtin-host` lease, recovery under it and repository preparation as a host job are K5.3's
and are not repeated here.
- **Depends on:** K5.3 (soft: if it has not merged, the stopgap columns
  `agent_runs.driver_instance_id`, `driver_lease_expires_at`, removed when K5.3 lands).
  **Beside:** A1.2 to A1.8.
- **Acceptance (`tests/integration/m1_recovery_provider_sessions.rs` extended):** a failed
  `fail_run` is logged and retried, not dropped; with K5.3, two simulated instances: the one
  without the lease recovers nothing; the run is recovered once when the lease changes hands.

### A1.10 One run creation path (M)

`runs::create` used by `start_run_in`, `retry_run_in`, `launch_sponsored_run` and
`work_with_worker` (routed through Launch, not removed); one cap and defaults rule for all
(the sponsored path gets the merged defaults, D report F4); `domain::transition_run` becomes
`hosts::builtin::transition`, calling `runs::update` and no longer calling `launch::kick`
or `agent_sessions` directly (it emits an event that those listen to).
- **Depends on:** A1.2, A1.3.
- **Acceptance:** existing start, retry and sponsored tests pass; a new test asserts a
  sponsored run gets the project's run defaults (the bug P0 hands over); G1 allow-list loses `api.rs`,
  `external_execution.rs`, `domain.rs` for `runs` writes.

### A1.11 Hosts and enrolment (L)

`hosts`, `host_enrolments`, `host_agents` (global tables with an owner, R19); `host_enrol`,
`host_describe` (stores capabilities, offers, actions, `issuer`, module manifest),
`host_heartbeat`, `host_notice` (the three notices), `host_agent_bind`/`unbind`,
`host_unlink` operations; `hosts::acting_for` building K1.23's `Via::ActingFor`; M1's `connected_module_install` at enrolment
when a manifest is named; interim host credential (until K1); Settings, Hosts page for a
workspace and for a person's own hosts (lazy, registered); `host.*` events; built-in host row
per workspace; foreign keys from `runs.host_id`, `run_usage.host_id`, `usage_reports.host_id`
and `requests.raised_via_host_id` to `hosts`.
- **Depends on:** A1.1; K1.22, K1.23 app credentials (soft: interim token); K1.7 outside
  references (soft: `agent_ref_changed` waits for it, stored and replayed); M1.12 (soft:
  panels drawn as plain forms until then).
- **Acceptance (`tests/integration/a1_host_protocol.rs`):** a code works once and expires
  after 15 min; a person-owned host and a workspace-owned host both enrol and name their
  owner; describe stores capabilities, offers and actions; binding needs `hosts.manage` (or
  owning the host) and `agents.manage` on the agent; a person-owned host cannot act for an
  agent not bound to it, nor reach a workspace where none of its agents are members;
  `agent_ref_changed` with a matching lineage keeps the account, with a different one opens a
  request; `agent_retired` interrupts active runs; unlink revokes and the next call is
  `401 host_revoked`; isolation cases.

### A1.12 Host command channel (L)

`host_commands`; long-poll and SSE on the same path; push with Standard Webhooks signing
and retries (through H1.2's `outbound` once merged); fetch with minted short-lived secrets
(`workspaceCredential` through K1.24's `issue_scoped`, revoked by `revoke_for_run` when the
run ends); acks; `start_uncertain` handling with same-id re-offers; `inspect`; `action`
commands; heartbeat-driven liveness and offline; versioning header; `Host` trait with
`RemoteHost`; `crates/host-protocol` (wire types and client); a test host in
`tests/support/test_host.rs` that A1.23 grows into the conformance suite.
- **Depends on:** A1.11, A1.3; A2.2 event stream (soft); K1.24 (soft: an interim run token
  with the same scope until then).
- **Acceptance (`a1_host_protocol.rs`, using the test host):** start → accepted → working →
  done; an ack lost twice leads to the same `commandId` offered three times and exactly one
  run on the host; a non-idempotent host is never re-offered and `run_mark_not_started`
  works; a late report revives the run; cancel goes `cancel_requested` → `cancelled` only
  when the host confirms; a host stops listing a run for three heartbeats and it becomes
  `interrupted`; no secret appears in `host_commands.body` or the event log (G11).

### A1.13 One launch path (M)

3.12: launches resolve the agent account and route; `runs::create` then host command,
built-in host, or `run.start` event plus the hand-off message; tag launches for any agent
account; `launches.run_id` required for `started`.
Sets the task's delegate through T1's task command (the hook T1.22 needs); calls H1's
`admit` at dispatch.
- **Depends on:** A1.10, A1.12, A1.4; A2.2 `run.start` delivery (soft: the hand-off message
  alone until then); T1.22's delegate field (soft: skipped until it exists); H1.25 `admit`
  (soft: no check until then).
- **Acceptance (`a1_launch_paths.rs`):** the same task launched to a built-in agent, to an
  agent on the test host and to a self-connecting runner produces one run each with the
  same cause and title shape; the hand-off message text is unchanged; tagging an
  `external_agent` bound to a host starts a host run; a launched task names the agent as
  its delegate; a workspace over its spending limit leaves the run `queued`, not failed.

### A1.14 Built-in host, step (a) (L)

`BuiltinHost` implements `Host` in process; `drive_turn` reports through `runs::append`;
approvals through `requests`; answers and Stop as in-process commands (no polling);
`account.rs`, `agent_sessions::record_run_turn`, `scheduler.rs`, `recovery.rs`,
`retention.rs` reads move under `src/hosts/builtin/`; G1 allow-list then lists only that
folder.
- **Depends on:** A1.8, A1.9, A1.10, A1.12.
- **Acceptance:** the whole existing run suite passes (`provider/contract_tests.rs`, the
  demo adapter integration tests); a demo-adapter approval reaches the run without the
  300 ms poll (assert the answer is delivered within one channel send); G1 allows run SQL
  only in `src/runs/` and `src/hosts/builtin/`.

### A1.15 Usage by window with confidence (M)

`attribute_window_usage` job; usage page and run view show confidence; budget and metering
read totals (touch point).
- **Depends on:** A1.3. **Beside:** A1.6 onwards.
- **Acceptance (`a1_run_reporting.rs`):** a 15-minute window over two runs that overlap 10
  and 5 minutes splits 2:1 with `estimated`; a window with no run stays unattributed;
  re-running the job changes nothing.

### A1.16 Built-in host, step (b) (L)

`crates/builtin-host` on A1.12's `crates/host-protocol`; the embedded host is a protocol
client over loopback, run only by the holder of K5.3's lease; diff as a result file plus a
live "Current changes" panel action; switch `AGENT_WORKSPACE_BUILTIN_HOST_TRANSPORT`.
- **Depends on:** A1.14.
- **Acceptance:** the run suite passes with `BUILTIN_HOST_TRANSPORT=protocol`; G10: the
  server crate has no `use crate::provider` outside the spawn point, checked by a test that
  scans `src/`; `cargo tree` check in CI that `builtin-host` does not depend on the server
  crate.

### A1.17 Built-in host, step (c) (L)

`agent-workspace-host` binary; `host_builtin` schema and migrations; automatic enrolment on
self-host; account and backend screens as the host's own offers, actions and panels;
deploy units; `AGENT_WORKSPACE_BUILTIN_HOST=off` default.
- **Depends on:** A1.16; H1.13 (the compose `host` profile and image that ship this binary);
  M1.14 panel blocks (soft: plain forms until then).
- **Acceptance:** a test brings up server and host as separate processes against one
  database, runs the demo adapter end to end, restarts the server mid-run and the run
  completes; restarts the host mid-run and the run recovers or is interrupted honestly.

### A1.18 Code-host seam, part 1 (M)

`CodeHost` trait and GitHub implementation; `repositories.code_host`, `external_id`; push
remote check through the trait; repository token on `start` commands.
- **Depends on:** nothing in A1 for the trait; A1.12 for the token on `start`. **Beside:**
  everything up to A1.14.
- **Acceptance:** existing GitHub tests pass; a unit test shows `is_safe_remote` refuses the
  same remotes as `git_runtime.rs:461` did; a start command's repository token has the
  requested access and expiry and is absent from stored rows.

### A1.19 Code-host seam, part 2 (L)

Pull request read model used by the 18 files; PR-driven task moves through task commands;
host-reported PR URLs resolved and linked to runs.
Builds on P0.11 (GitHub-driven task moves already go through task commands): A1.19 moves
them behind the `CodeHost` seam rather than rewriting them.
- **Depends on:** A1.18, A1.3; T1 task commands; P0.11.
- **Acceptance:** `notifications`, `conversations`, `mcp/code` tests pass unchanged; a test
  host reports a PR URL and the run view shows its checks; a merged PR moves its task through
  the task command layer (an activity row appears, which the raw `UPDATE` did not write).

### A1.20 A2A binding (L)

`src/hosts/bindings/a2a.rs` per 3.10; enrol an A2A agent by its card URL.
- **Depends on:** A1.12, A1.8.
- **Acceptance:** a recorded A2A fixture server drives `submitted → working →
  input-required → completed`; the question appears as a request and its answer is sent
  back; an artifact becomes a result; `unknown` becomes `start_uncertain`.

### A1.21 ACP in the built-in host, step (d) (L)

ACP adapter inside `crates/builtin-host`, held to `provider/contract_tests.rs`.
- **Depends on:** A1.16.
- **Acceptance:** the contract tests pass against a recorded ACP agent fixture, including a
  permission request becoming a request and a plan becoming a `plan` activity.

### A1.22 Contract (M)

Drop mirror triggers; `agent_approvals`, `person_questions` and `agent_run_usage` become
views; stop writing `agent_run_events`; delete the `cfg(any())` blocks in `api.rs`; G1
allow-list empty except `src/hosts/builtin/` (or nothing after A1.17).
- **Depends on:** A1.6, A1.8, A1.14 (and A1.17 for the last line).
- **Acceptance:** full suite; G1 and G2 have no legacy entries.

### A1.23 Example host and conformance suite (M)

Workspace's proof that the protocol is enough (R21). `examples/minimal-host`: a small
program on `crates/host-protocol` that enrols, describes one echo agent, one offer whose
panel ends in `agent_register`, one agent action and one host action, runs `start`,
`cancel`, `steer` and `answer`, reports activities and usage, and sends each of the three
notices; under 500 lines, documented in `docs/hosts/writing-a-host.md`.
`crates/host-conformance`: the protocol script from A1.12's test host made runnable both
ways: against a server (any host author's CI can use it as a fake Workspace) and against a
host (`host-conformance --server <url> --code <code>` drives a real host through enrol,
describe, start, an uncertain start, cancel, an answer, an action and the notices, and prints
what it lacks). The minimal host and the built-in host both pass it in CI.
- **Depends on:** A1.11, A1.12; A2.19 for the `agent_register` step (soft: the offer's panel
  stops at "proposed" until then).
- **Acceptance:** the minimal host passes the suite in CI; the suite run against the server
  passes; a host that drops acks is reported as "start not idempotent", not as a pass.

---

## 6. Guard rails

- **G1 Run SQL only in the run service.** `src/runs/mod.rs` test walks `src/` (same pattern
  as `src/policy_inventory.rs:1817-1850`) and fails when a file outside `src/runs/` (and,
  until A1.17, `src/hosts/builtin/`) contains SQL touching `runs`, `run_activities`,
  `run_usage`, `usage_reports`, `agent_runs`, `agent_run_events`, `agent_run_usage`
  (patterns `FROM|JOIN|INTO|UPDATE <table>\b`). A `LEGACY_RUN_SQL` allow-list names the
  files not yet moved and the slice that removes each; the test also fails when a listed file
  no longer matches, so the list only shrinks.
- **G2 Request SQL only in `requests`**, same shape, covering `requests`, `agent_approvals`,
  `person_questions`.
- **G3 Host tables only in `hosts`.**
- **G4 No active-state literals.** Fails on `'queued', 'starting'` style lists outside
  `src/runs/state.rs`.
- **G5 No account-kind checks in A1 modules.** Fails on `ActorKind::` comparisons and
  `actor.kind =` in `src/runs/`, `src/requests/`, `src/hosts/` (except the built-in host's
  copy of today's code until A1.14 rewrites it) and on any new `people_only` permission in
  the run, request or host groups.
- **G6 No stack names in core.** Fails when an MCP tool description, operation description,
  error string or file under `src/runs/`, `src/requests/`, `src/hosts/` (outside
  `hosts/bindings/` and `hosts/builtin/`) names a provider, model family, harness or code
  host from a deny-list kept in the test.
- **G7 State follows activities.** A property test generates random valid activity sequences,
  applies them through `runs::append`, then replays the stored log through the reducer and
  must reach the stored state.
- **G8 Protocol conformance.** The test host of A1.12 runs the whole protocol script
  against the server; from A1.23 that script is `crates/host-conformance` and also runs
  against `examples/minimal-host`; from A1.16 against the embedded built-in host, and from
  A1.17 against the binary.
- **G9 Isolation.** Every new route and tool has cases in `workspace_isolation.rs` and
  `write_isolation.rs` (enforced by their existing guard tests).
- **G10 The server does not run agents.** From A1.16, a test fails on `crate::provider`,
  `git_runtime` or child-process spawning (`Command::new`) in the server crate outside the
  embedded-host spawn point; from A1.17 the server crate does not depend on
  `crates/builtin-host` at all.
- **G11 Secrets never stored.** Tests assert that `host_commands.body`, events and
  `run_activities.payload` never contain the minted repository or workspace token (the test
  host captures them and searches the database).
- **G12 SDKs in CI.** `sdks/runner-*` tests run in `tools/ci/checks.sh`.

---

## 7. Risks and open questions

| Risk | How we'd notice | Fallback |
|---|---|---|
| Mirror triggers and the run service write the same row and fight (state flips back) | G7 and the A1.1 tests; a `runs.version` going backwards in logs | Triggers skip rows whose `version` was bumped in the same transaction; if that fails, move built-in writers sooner and keep reads on `agent_runs` until then |
| The activities backfill is larger than expected and slows the live database | Job duration metric | Batches are small and resumable; run it off-hours; reads fall back to `agent_run_events` for runs not yet copied (`legacy_event_id` tells) |
| Uncertain starts lead to duplicate work on hosts that don't dedupe | Two host run refs reported for one run | Never re-offer without `idempotentStart`; `UNIQUE (host_id, start_key)`; a second `hostRunRef` for one run is refused and logged |
| Liveness noise: busy agents marked "not responding" | Many `not_responding` flips per run | Per-run `report_interval_seconds` set by the reporter; `quiet` before `not_responding`; heartbeat listing counts |
| Changing approvals from `human_required` to a permission lets an agent approve something a person expected to | Audit of answers by agents | Default roles reproduce today's behaviour; the change is visible on the Roles page; A1.8's test pins the defaults |
| The built-in host split (A1.16-A1.17) is bigger than three L slices | Slice overruns | (a) already gives the seam; (b) and (c) can stop at (b) for a release without losing anything |
| Hosts that can't report steps make the Runs page look empty | Runs with only lifecycle activities | Show "This host doesn't report steps" from capabilities; gaps are explicit |
| K1, K2, K3, K5 land later than A1 needs | Slices blocked | Every dependency here is soft with a named interim (tools in today's style, outbox, interim host token, lease columns) |

Questions only the owner can answer, with a recommendation:

1. **Should a launch to an agent that connects by itself keep posting the chat hand-off
   message after A2's `run.start` event exists?** Recommend yes: people see in the thread
   what was asked, and old runners keep working.
2. **Who answers built-in approvals by default?** Recommend the owner and admin roles
   (`runs.approve`), with agents excluded by their default role, which reproduces today and
   is changeable.
3. **Keep Workspace opening pull requests through its own App beside host-pushed PRs?**
   Recommend yes, both: hosts push and may open PRs; the existing Publish and Open pull
   request actions stay.
4. **Should the built-in host's private store live in the same PostgreSQL by default?**
   Recommend yes for self-hosters (one database to back up), in its own schema, with a
   separate database as an option.
5. **ACP adapters replacing the per-provider adapters?** Recommend coexistence; never
   retire an adapter that works.

---

## 8. Touch points

**A1 needs:**

- **K1**: the single agent account kind; app accounts for hosts, global with an owner (R19),
  acting for agents through K1.23's `Via::ActingFor`, limited to `host_agents`;
  run-scoped short-lived tokens (K1.24 `issue_scoped`, `revoke_for_run`); the outside
  reference (K1.7 `agent_outside_refs`, `agent_ref_rebind`); catalogue entries `runs.report`,
  `requests.raise`, `requests.answer`, `hosts.manage` (K1.14 adds them) beside K1's
  `requests.answer_sensitive` and `replies.steer`; groups for addressed requests (K1.27); the
  access function and SQL filter A1's `VISIBLE` fragment switches to; removal of the kind
  rules in 2.6 that live outside A1's modules (`src/policy.rs:257` `people_only`). The 0033
  `runtime_kind = 'hermes'` check was already relaxed by 0037; the remaining trigger
  `validate_external_agent_connection` must be dropped in K1.6 (requested).
- **K2**: the operation registry and envelope, idempotency store, error list (`rate_limited`,
  `limit_reached`, `host_revoked`), generated MCP tools, routes and TypeScript types for every
  operation in 3.7, 3.8 and 3.9.
- **K3**: the event log for `run.*`, `request.*`, `host.*` (with `events.run_id`,
  `request_id`, `audience_account_id`), written only through `events::emit(tx, …)`; the link
  store for results (`thing made_from run`, an existing kind);
  the thing registry entry for runs and requests (kind `run`, `request`), with the sync
  trigger on `runs` and `requests`, not `agent_runs` (4.1); `request.opened` as the catalogue
  name.
- **K4**: `space_id` columns are `project_id` values until K4's rename.
- **K5**: K5.3's single-holder `builtin-host` lease and recovery under it, K5.6 shared rate
  limits for report calls, K5.9 live fan-out of run events across processes, the file
  storage seam for diffs and attachments; `AGENT_WORKSPACE_BUILTIN_HOST` is K5's variable.
- **A2**: delivery of `run.start`, `run.cancel`, `request.opened`, `request.answered`,
  `host.command` events by webhook, stream and poll (A2.2, A2.3); agent status lines fed from
  run state (per workspace, R19); budgets as H1 spend-limit rows reading `run_usage`; the
  bridged connector, which speaks the host protocol and takes the owner-specific bridge out of
  the repository (A2.25); `agent_register` and the drawing of offers and actions (A2.19,
  A2.20), which create `host_agents` bindings.
- **C1**: `proposal_decide`, called from `request_answer` for `kind = 'proposal'`.
- **C2**: the inbox (C2.12) that holds W1.24's request card, phone answering; C2.35 reads
  `messages.agent_run_id` (C2's `messages.run_id`) and A2's `reply_stop_all`.
- **T1**: task commands for PR-driven moves; the delegate set at launch (T1.22); task fields
  for run titles.
- **W1**: list, table and detail pieces; W1.24 request card and run view (after A1.5 and
  A1.8); generated types.
- **M1**: Runs and Code as modules; `connected_module_install` at enrolment (M1.12); panel
  blocks for host panels, offers and actions (M1.14).
- **H1**: the compose `host` profile and image for the host binary (H1.13); `admit` at
  dispatch and `metering::record` from usage (H1.25 to H1.27); the hosted agent service as an
  ordinary host (H1.38).
- **P0**: P0.32 rewords tool descriptions (A1.6 does the rest), P0.11 moves GitHub-driven task
  changes through task commands (A1.19 builds on it), P0.10 posts wake-ups after commit; P0
  hands A1 the `hand_off` bug (A1.4), sponsored-run defaults (A1.10) and `fail_run` (A1.9). If
  P0 removes the `cfg(any())` blocks or the dead spikes first, A1.22 skips them.

**A1 provides:**

- To everyone: `runs::*` reads (3.6), `requests::*`, `hosts::*`, the run and request events.
- To A2: the host protocol and its operations, `run.start` semantics, `action` commands,
  offers and the three notices, the run record agent status derives from.
- To C2 and W1: `requests::card`, request notifications.
- To H1: `run_usage` totals with confidence; the host binary; `hosts` and `host_agents` for
  the global tables list.
- To every host author: `crates/host-protocol`, `examples/minimal-host`,
  `crates/host-conformance` (A1.23).
- To K4: `runs::move_conversation_space` (A1.2).
- To W1: run, activity, request and host API shapes.
- To K3: run and request as thing kinds.

---

## 9. Changes to the end state

1. **Hosts are app accounts.** Add to "Host" in section 3: "A host signs in as an app
   account that may act for the agent accounts bound to it; it gets no other rights."
2. **"Interrupted" as an outcome.** Add to "Honest states" in section 4: "A run whose host
   was lost or unlinked, or that could not be proved finished after a restart, ends as
   interrupted, never as done or failed."
3. **Liveness is separate from state.** Add to "Activity": "'Not responding' is a mark on
   the run, not a state: the run keeps what it was doing."
4. **A chat reply is not a run.** Make explicit in "Run": "A reply in chat is not a run
   unless a launch created one or the agent opens one; every launch creates one."
5. **The built-in host's store.** Add to "The built-in host": "It keeps its own data (sign-ins,
   worktrees, sessions) in its own store, which on a self-hosted install may be a schema in
   the same database server; the server never reads it."
6. **Command names.** The host protocol's fixed part is enrol, describe, start, cancel,
   steer, answer, report and three notices (R21); **inspect** (does the host know this run)
   is an optional part of start, and **action** carries a host-published action back to the
   host. Add both to "Hosts".
7. **Hosts have owners.** Add to "Host": "A host is owned by a person or a workspace; a
   person's host may run their agents in every workspace they belong to (R19)."


---

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| A1.1 | Run record tables and mirror | M | none | A1.18 |
| A1.2 | Run service reads, part 1, and the guard | M | A1.1 | A1.3, A1.7, A1.9, A1.18 |
| A1.3 | Run operations for any agent | L | A1.1; K2.2, K2.3, K3, K5.6 (soft) | A1.2, A1.7, A1.9 |
| A1.4 | Connected runners get runs | M | A1.3 | A1.5, A1.6, A1.7, A1.8 |
| A1.5 | Runs page and run view for every agent | M | A1.2, A1.3; W1 list and detail pieces (soft); W1.24 follows it | A1.4, A1.6, A1.8 |
| A1.6 | Run service reads, part 2, and neutral wording | M | A1.2 | A1.3, A1.4, A1.5, A1.7 |
| A1.7 | Requests record | M | A1.1 | A1.2 to A1.6, A1.9 |
| A1.8 | Request operations and answering by role | L | A1.7; K1.13, K1.27, C1 `proposal_decide` (soft); K1.14 follows it | A1.9 to A1.12 |
| A1.9 | Run leases and recovery | M | K5.3 (soft, stopgap columns) | A1.2 to A1.8 |
| A1.10 | One run creation path | M | A1.2, A1.3 | A1.7, A1.8, A1.11 |
| A1.11 | Hosts and enrolment | L | A1.1; K1.22, K1.23, K1.7, M1.12 (soft) | A1.4 to A1.10 |
| A1.12 | Host command channel | L | A1.11, A1.3; A2.2, K1.24 (soft) | A1.8, A1.10, A1.15 |
| A1.13 | One launch path | M | A1.4, A1.10, A1.12; A2.2, T1.22, H1.25 (soft) | A1.14, A1.15 |
| A1.14 | Built-in host, step (a) | L | A1.8, A1.9, A1.10, A1.12 | A1.13, A1.15, A1.18 |
| A1.15 | Usage by window with confidence | M | A1.3 | A1.4 onwards |
| A1.16 | Built-in host, step (b) | L | A1.14 | A1.19, A1.20 |
| A1.17 | Built-in host, step (c) | L | A1.16, H1.13; M1.14 (soft) | A1.19 to A1.21 |
| A1.18 | Code-host seam, part 1 | M | none (A1.12 for the start token) | A1.1 to A1.14 |
| A1.19 | Code-host seam, part 2 | L | A1.18, A1.3; T1 task commands; P0.11 | A1.14 to A1.17 |
| A1.20 | A2A binding | L | A1.12, A1.8 | A1.16, A1.17, A1.19 |
| A1.21 | ACP in the built-in host | L | A1.16 | A1.17, A1.19, A1.20 |
| A1.22 | Contract | M | A1.6, A1.8, A1.14 (A1.17 for the last step) | A1.20, A1.21 |
| A1.23 | Example host and conformance suite | M | A1.11, A1.12; A2.19 (soft) | A1.13 to A1.22 |

## Review changes

- 2.2: the 0033 `hermes` check was already lifted by 0037; named the trigger that remains (`validate_external_agent_connection`) and K1.6 as its remover. Why: the plan asked K1 or A2 to lift something already gone.
- 3.2: `messages.agent_run_id` is the run link C2 calls `messages.run_id`, and how it is set; `runs::move_conversation_space` for K4.15 and K4.16. Why: C2.35 and K4 assumed them.
- 3.4: one counting mechanism, `runs::record_usage` calls H1's `metering::record` with dedupe keys, and `admit` at dispatch. Why: H1 and A2 each had their own counters (R13).
- 3.6, 3.16: added `run.start` and `run.cancel`, `host.notice`, and that every event names its workspace; noted `request.opened` against K3's `request.created`. Why: A2 delivers these; R19's merged stream.
- 3.7: `run_start_input` operation, and the working indicator never opens a run. Why: A2's `run.start` payload needs an input address; R6.
- 3.8: `addressed_group_ids` (K1.27); answering by `runs.approve`, `requests.answer_sensitive` and `requests.answer`; `request.opened` per addressed account; proposals through C1's `proposal_decide`; the card is W1.24's piece in C2.12. Why: R14, R17 and K1's catalogue.
- 3.8: runs and requests raised from a DM live in the direct scope, visible to participants only. Why: R20.
- 3.9: hosts are owned by a person or a workspace and are global tables; enrolment names the owner; person-owned hosts are linked from personal settings. Why: R19 and R7.
- 3.9: the fixed protocol is enrol, describe, start, cancel, steer, answer, report and three notices; `describe` gains `offers`, `actions`, `issuer` and the module manifest; the `call` command becomes `action`; `host_notice` added; binding happens through A2's `agent_register`. Why: R21.
- 3.9: named what proves the protocol: built-in host, example host, conformance suite, with slices. Why: R21.
- 3.9: enrolment installs the M1 connected module and uses K1.22 and K1.23 credentials. Why: R7.
- 3.11: variables renamed to K5's `AGENT_WORKSPACE_BUILTIN_HOST`; recovery uses K5.3's single-holder lease, not per-run leases; `diff.get` and the account and backend screens became the built-in host's own actions and panels. Why: K5.3 already does it; R21 removes fixed call names.
- 3.12: launch sets the T1.22 delegate, dispatch calls `admit`, and the `run.start` payload is given. Why: T1 and H1 assumed these.
- 3.13: K1 needs the owner, `acting_for` until K1 models it, missing catalogue entries, and the 0033 trigger drop. Why: unmet in K1.
- 3.15: Hosts page also under a person's own settings, drawing host-level actions. Why: R19 and R21.
- 4.1: added the trigger table. Why: R16; asked K3 to sync `run` from `runs`.
- Slices A1.3, A1.4, A1.5, A1.6, A1.8, A1.9, A1.10, A1.11, A1.12, A1.13, A1.16, A1.17, A1.19: dependencies given with slice ids; P0 hand-overs placed (A1.4 `hand_off`, A1.9 `fail_run`, A1.10 defaults, A1.6 after P0.32, A1.19 on P0.11); A1.9 shrunk to what K5.3 leaves; the cycle A1.5 and W1.24 broken by building in place first. Why: real dependencies and R10.
- New slice A1.23, example host and conformance suite. Why: R21 asks for both and says which slice builds each.
- Touch points, "A1 provides" and changes to the end state updated to match. Why: kept in step with the above.
- 3.8: `request_answer` on a proposal calls `proposal_decide` and nothing else. Why: R26 (content and web review, accepted).
- A1.5: says W1.24 extracts `RunView` from the run page A1.5 builds. Why: R28.
- 3.5, 3.13, 3.16, 8, A1.11 (kernel review, accepted): results are `made_from` links from the result to the run (K3's kinds stay closed); `run.*` and `request.*` go through `events::emit`; hosts act through K1.23's `Via::ActingFor`, built in one place; revocation follows R34's manage rule. Why: accepted requests, R34.
