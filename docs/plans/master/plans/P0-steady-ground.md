# P0 Steady ground: the plan

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Workstream P0 of the Agent Workspace master plan. Planner's read of the code is at main
`fe95a74` (pull request #251 not included). Every `path:line` below was re-read in that tree;
"inferred" marks what was reasoned from code rather than seen running.

## 1. Goal

When P0 is done, the silent bugs the health check confirmed are gone: the conversation sidebar
no longer shows the names of other people's DM and group threads, an agent's @mention in a
task comment reaches the person, reading a conversation through an app clears its
notifications, a database blip no longer stops the server, an event's webhooks and inbox
notifications are never dropped after one transient error, and sign-in and agent traffic can't
be throttled for everyone by one anonymous caller. CI fails on a race that passes on retry and
on a web change that breaks a Rust test. Every other workstream starts from a shared fixture
layer with fast test databases, a privacy net inside a workspace, contract tests that keep
today's connected agents working, rules in `AGENTS.md` with guard tests that stop new code
from adding to the five structural problems, and a repository whose front door (README,
`AGENTS.md`, `docs/plans/master/`) describes the product and plan as they are.

## 2. Where the code is today

### 2.1 Confirmed bugs and correctness risks (each re-read)

| # | What goes wrong | Where | Severity | Fixed in |
|---|---|---|---|---|
| 0 | The conversation sidebar checks only `ProjectView`, then lists every `kind = 'thread'` row in the project with no visibility filter. Threads sit under DMs and groups and take their visibility from the parent (`0037_direct_conversations.sql:160-181`). Everyone who has opened Messages, and every agent in a DM, is a member of the workspace's hidden `kind = 'direct'` project (`direct.rs:1082-1112`, `ensure_space_in`). So `GET /api/v1/projects/<direct project>/conversation-sidebar` returns the id, name (taken from message text), unread count and latest run state of every DM and group thread in the workspace. The channel list in the same handler and `list_channels` have no filter either; harmless while every channel is project-wide, a leak the day private channels exist. Found by another planner, confirmed by reading (not run). | `conversations.rs:3408-3414` (only `authorized_project`), `:3446-3529` (thread query, `WHERE thread.project_id = $1 AND thread.kind = 'thread'`), `:3419-3444` (channels), `:786-823` (`list_channels`) | Private data leak inside a workspace | P0.0 |
| 1 | Outbox event is marked published after `notify_people` or `queue_event_route` failed; both swallow their error inside a savepoint and only log. The event's inbox notifications, push, email, webhooks and automation firings are lost for good. | `apps/server/src/realtime.rs:724-726` (calls), `:762-807` (`notify_people` logs and returns), `:812-844` (`queue_event_route` same) | Silent data loss | P0.1 |
| 2 | Any error from the run scheduler's or GitHub worker's drain ends the loop with `?`; `main.rs` then shuts the whole server down with "stopped unexpectedly", and restart interrupts every live run. The outbox publisher and reply worker log and continue instead. | `scheduler.rs:80-92` (`drain(&state).await?`), `github.rs:141-155` (`drain_jobs(&state).await?`), `main.rs:481-519` | Outage from a blip | P0.2 |
| 3 | CI retries every failed Rust test once and passes a fail-then-pass as FLAKY; Playwright retries once in CI. A race bug that fails half the time goes green. | `.config/nextest.toml:5`, `apps/web/playwright.config.ts:16` | Hides races | P0.3 |
| 4 | A pull request that only changes web files runs only web checks, but Rust tests read web files: `registry.json` (`modules/mod.rs:246`, `layouts.rs:125`), Markdown parity JSON (`page_blocks.rs:745`, `page_docs/write.rs:1149-1150`, `page_docs/markdown.rs:1140`), page parity JSON (`page_docs/parity.rs:18,22`) and `settings/workspace/roles.ts` (`roles.rs:2375`). The break appears on `main` after merge. | `tools/ci/changed-areas.sh:20-22` | Breaks main | P0.3 |
| 5 | MCP `task_comment` builds content with `MessageContent::from_plain_text` (one text node) and inserts no `work_item_comment_mentions` or `work_item_comment_references`; `task_comment_edit` does the same. An agent's "@alice" notifies nobody and "see AWD-12" isn't a backlink. The web path resolves both (`work_items.rs:2938-3020`), and the edit path already has the shared helpers `resolve_comment_content` and `insert_projections` (`work_items/comments.rs:95`, `:185`). | `mcp/writes.rs:1255`, `:1345`; `mcp/comment_tools.rs:150`; notifications read only the projection (`notifications.rs:902`) | Silent parity failure | P0.4 |
| 6 | The policy inventory declares the task-comment route as `MessageSend` while the handler checks `WorkItemEdit`. | `policy_inventory.rs:1444-1449` vs `work_items.rs:2884-2889` | Misleading audit aid | P0.4 |
| 7 | MCP mark-read has its own copy of the read-state upsert and never calls `notifications::read_for_conversation` (the only caller is the HTTP handler). Mentions read through an app stay unread, and push or email for them can still go out. | `mcp/talk.rs:1395-1433` vs `conversations.rs:2786-2793` | Silent parity failure | P0.5 |
| 8 | "Who may manage or act as an agent" is written four ways. The OAuth consent screen and connected-apps list accept a project owner/admin (custom role keeping `agents.manage`) who is any enabled workspace member; the web requires workspace owner/admin plus `ExternalAgentManage` through `authorize`. The OAuth path then mints a grant acting as the agent, which reaches every project the agent is in. | `mcp/oauth.rs:1272-1297` (`agent_choices`), `:1300-1340` (`managed_agent`); `connected_apps.rs:100-114`, `:131-155`; `external_agents.rs:1282-1314`; `mcp/agents.rs:439-480` | Privilege escalation (edge roles; reach inferred) | P0.6 |
| 9 | The module gate only finds a scope in `/api/v1/projects/{id}/…` and `/organizations/{id}/…`; page and whiteboard routes named by id skip it, and `GET /api/v1/whiteboards` lists boards where Whiteboards is off. MCP refuses the same calls. | `modules/gate.rs:72-91`; `memory.rs:1353`, `:1374`; `whiteboards/http.rs:112` | Policy bypass (not a cross-tenant leak) | P0.7 |
| 10 | `roles::role_allows_sql` and `search::projects_allowing` are documented as "the role keeps permission" but only check `custom_role_allows`, which is true for every built-in role (`role_id` NULL), viewer and worker included. Every caller passes `tasks.view` or `code.view` today, so nothing is live. | `roles.rs:381-398`, `search.rs:470-490`; `custom_role_allows` in `migrations/20261030020000_workspace_roles.sql:121-127` | Latent grant to viewers and workers | P0.8 |
| 11 | The generic tool refusal emits `retryCategory` + `message`, but `repository_get` and `pull_request_get` return `ReadResult`, whose schema requires `retry_category` (snake case). Clients that validate results against the output schema reject the refusal. Both tools are on the Hermes allowlist. | `mcp.rs:3967-3995`; `mcp/reads.rs:26-33`; `mcp.rs:3521`, `:3547`; `tools/hermes-connector/verify-fixtures.py:20-39` | Agent client breaks (inferred from schemas) | P0.9 |
| 12 | The public API maps unknown outcomes to HTTP 200, so a rejected batch (`BatchResult::Rejected`), `busy` and `cursor_expired` are 200. | `public_api.rs:118-127`; `mcp/task_batch.rs:79-90` | Failure read as success | P0.9 |
| 13 | Agent wake-up webhooks are POSTed (8 s timeout each, up to 10) inside an open transaction that holds row locks and a pool connection; the lease sweep waits behind it. The documented production pool is 5 connections. | `replies.rs:1315-1438`, `:1244-1262`; `docs/self-hosted-red-hat.md:210` | Pool starvation, then item 2 | P0.10 |
| 14 | GitHub PR automation moves tasks with its own `UPDATE`, its own payload dialect (`newStatusId`), calls `insert_work_item_activity` and `enqueue_work_item_updated` directly, and skips `finalize_change`: assigned or watching agents aren't told, the webhook has no `changedFields`, and the card keeps its old rank. It also locks tasks without the project lock first. | `github.rs:56-61`, `github.rs:4239-4335`; compare `work_item_commands.rs:2834-2843` | Silent: agents not told | P0.11 |
| 15 | Board drag, `task_mark_blocked` and `task_mark_ready_for_review` bypass `update_locked_work_item`; their activity payloads lack `changedFields` or before/after, so `task.updated` webhooks don't say what changed. | `work_items.rs:2158-2322`; `mcp/writes.rs:1640-1745`, `:1747-1897`; `events.rs:517-541` | Silent event gaps | P0.12 |
| 16 | `check_login` charges one server-wide bucket (60 a minute) before anything else, for any well-formed name; `check_mcp_auth` charges one server-wide bucket (900 a minute) on every request, valid ones included. One anonymous client can refuse sign-in to everyone; all agents share 15 requests a second. The per-address limit trusts `cf-connecting-ip` only. | `rate_limit.rs:109-117`, `:164-172`, `:420-425`; `auth/mod.rs:186-195`; `mcp.rs:4334-4337` | Denial of service | P0.13 |
| 17 | OAuth dynamic registration refuses everyone once 1,024 clients exist server-wide; a client with any grant (even revoked) is never pruned. Grants can't simply be deleted: six history tables reference them for "via app" attribution. | `mcp/oauth.rs:182-205`; `retention.rs:340-355`; FKs in `migrations/0047_message_edits.sql:15,43`, `20261017070000_whiteboards.sql:111`, `20261112193000_sheets.sql:129`, `20261017160000_later_and_scheduling.sql:53`, `20261017170000_agent_skills_and_handoffs.sql:25` | Hosted blocker | P0.14 |
| 18 | 83 of 113 write tools take no `clientRequestId`, including creates that duplicate on retry (`page_write` create, `decision_record`, `whiteboard_create`, `whiteboard_duplicate`, `project_create`, `file_upload`, `message_schedule`, `task_from_template`, `milestone_write`), while `SERVER_INSTRUCTIONS` tells agents a stable `clientRequestId` makes retries safe. | `health-check/E` F5 (measured by parsing arguments); `mcp.rs:135` | Duplicates on agent retry | P0.15, P0.16 |
| 19 | Three first-build race tests run on migrations 0001-0008 or 0001-0010 only, so they prove "gap-free sequences" and "archive race closed" for functions since redefined. | `tests/integration/m2_conversations_database.rs:556`, `:672`, `:1443-1467`; `m3_execution_foundation_database.rs:831`, `:1069-1079` | Tests prove the wrong schema | P0.19 |
| 20 | Rust maps run-admission refusals by matching the trigger's exact exception text; a reworded `RAISE` silently turns a friendly conflict into a 500. | `domain.rs:178-219`; latest body of `validate_agent_run_scope_and_admission` in `migrations/20261016090500_any_harness_per_run.sql` | Silent degradation | P0.29 |
| 21 | `conversation_visible_to` answers only "project-wide, or on the member list"; it doesn't check workspace or project membership, yet `AGENTS.md` presents it as the conversation access rule. No leak was found; nothing enforces the pairing. | `migrations/0037_direct_conversations.sql:158-180`; `AGENTS.md` "Architecture rules" | Future cross-tenant leak | P0.28 |
| 22 | Page comment and page suggestion notifications copy text into the row: `detail.preview` holds the first 140 characters of the comment and `detail.title` the page's title at the time, against the table's own rule "Never message text" (`20261016120000_notifications.sql:38-42`) and `notifications.rs:12-16` ("Rows hold IDs, not text"). Re-read: editing or deleting a comment through `comments::change` already strips the copied preview (`notifications::forget_page_comment`, `notifications.rs:703-746`, called at `comments.rs:517`, `:545`), so the case C1's planner named is covered on that one path. What still leaks: the copy is the only defence, so any other removal path (C1.5 moving page comments into conversations, retention, a workspace deletion, a moderator delete added later) leaves the words in the inbox; an archived or renamed page keeps its old title and the comment text in the inbox, because `VISIBLE` checks only project access (`notifications.rs:1321-1342`) and the reader prints `detail.title`/`detail.preview` as stored (`notifications.rs:1880-1888`). Found by C1's planner (C1.1). | `page_docs/comments.rs:451-456`, `page_docs/suggestions.rs:434-437`, reader `notifications.rs:1880-1888` | Private text survives deletion and archive | P0.36 |
| 23 | Password-reset and address-confirmation mail goes through the SMTP server of the person's *default* workspace (`mail/mod.rs:305-337`), and accepting an invitation to another workspace silently makes that workspace the default (`workspace.rs:1835-1839`, `accept_existing_user`). The code's own comment says the person's chosen default is trusted because it was a choice; the invitation path makes the choice for them. So an owner of workspace B who invites someone already in A, and has set B's SMTP to a server they control, receives that person's reset link after asking for a reset in their name (the request form answers anyone, `password_reset.rs:1-13`). Two-factor stops the takeover (`password_reset.rs:11-12`); without it the account, and every workspace it reaches, is taken. Found by reading H1's inventory ("account mail borrows the SMTP of one of the person's workspaces", H1 2.2) and the invitation code; not run. `tests/integration/bug_sweep.rs:149` covers "chose or owns" but not the invitation path. | `mail/mod.rs:305-337`, `workspace.rs:1835-1839` | Account takeover across workspaces | P0.37 |
| 24 | Not a live leak, pinned: under R19 accounts become global and one person or agent sits in many workspaces. Today `people_search` lists only memberships in the caller's own projects (`mcp/talk.rs:2014-2070`, `membership.project_id = ANY(scope_projects)`, `mcp/common.rs:207-216`) and the Messages people list reads one direct project (`direct.rs:253-268`), so nothing reveals another workspace by reading. No test says so, and K1's account slices rewrite exactly these queries. | as cited | Future cross-workspace disclosure | P0.38 |

Re-checked and **not** P0's to fix (the minimal safe step, if any, is named; the owner is the
workstream that finishes it):

| Finding | Where | Owner |
|---|---|---|
| Audit log, mail and server status gate on `WorkspaceRole` by name, ignoring custom roles (a narrowed admin still reads the audit log) | `audit_log.rs:192-206`, `mail/http.rs:62-73`, `server_status.rs:218-228` | K1 (needs an `audit.view` permission; P0 adds the guard so no new `WorkspaceRole` use appears) |
| `external_agent_connections.webhook_secret` plaintext column still read when no keyring is configured | `external_agents.rs:745`, `replies.rs:1311` | K1 (credentials) |
| Realtime fan-out per process; every socket checks every event | `realtime.rs:131-170`, `:339-421` | K5 |
| Startup recovery takes every non-terminal run; sign-in ceremonies in memory | `recovery.rs:29-57`, `provider/registry.rs:109` | K5 / A1 |
| Agent-initiated launch to a connected runner: `hand_off` posts with `mcp_credential: None` (`launch.rs:2160-2167`), which `lock_message_conversation` refuses for agents (`conversations.rs:2303-2309`). Reachable only through an automation made by an agent (`automations.rs:2452-2460`; `task_launch` refuses agents at `mcp/launch_tools.rs:248`). It fails loudly with "conversation not found", not silently. | as cited | A1 (launches to runners become runs) |
| Sponsored runs ignore run defaults | `external_execution.rs:916-923` | A1 |
| `provider.rs:442` ignores a failed `fail_run` | `provider.rs:442` | A1 |
| Local harnesses advertise `workspace_write` without enforcing it | `provider/command_line.rs:197`, `provider/claude_code.rs:95` | A1 (owner question D-Q1 decides sandbox or switch) |
| Thread creation written six times; agent-made threads never `auto_named` | `conversations.rs:1083`, `mcp/talk.rs:2208`, `mcp/replies.rs:1027`, `work_items.rs:1555`, `launch.rs:1808`, `:2964` | C1 (P0's guard stops a seventh) |
| Agents mentioned in task comments get no wake-up (people do once P0.4 lands) | `notifications.rs:12`, `external_events.rs:87-140` | A2 |
| Agents tagging agents in channels create no reply request | `replies.rs:105` | A2 (owner question) |
| Three SSRF guards; the wake-up client pins only the first address | `replies.rs:1637-1710`, `outbound.rs:1-9`, `link_previews/guard.rs:11-95` | H1.2 ("one outbound guard", owner-now); A2's pusher uses it. Not A2 and H1 both || Sheet writes hold the project lock during recalculation | `sheets.rs:15-18`, `:1485` | C1 |
| `task_update` doesn't mention another agent's claim; Done doesn't release a claim | `mcp/task_batch.rs:1668-1700` | T1 |
| Public API body cap 1 MiB vs MCP 6 MiB | `public_api.rs:43`, `mcp.rs:133` | K2 |
| Timeline cache key filled by two query functions | `threads/threadTimeline.ts:238-246` | W1 |
| `bug_reports.rs:968`, `:1108` and `automations.rs:2410` call `mcp::call_tool_direct`, which builds fake HTTP parts and an in-memory MCP peer (`mcp.rs:4250-4300`) | as cited | K2. No live bug was found: the calls act as the link's maker or the rule's maker and every tool rechecks access. Rewriting them now onto `work_item_commands` would add a third hand-written edge, the exact thing K2 removes. P0 freezes the caller list (rule R4). |

### 2.2 Compatibility with agents connected today

- **MCP revision.** `MCP_PROTOCOL_VERSION = "2025-06-18"` (`mcp.rs:125`); `get_info` sets no
  version, so rmcp `=3.9.0` (`apps/server/Cargo.toml:31`) echoes a supported client version.
  Resumed sessions are rebuilt as `2025-06-18` (`mcp.rs:4157`). Tests only ever initialize with
  `2025-06-18` (`tests/integration/m6_mcp_foundation.rs:185`, `:247`). Hermes 0.20 negotiated
  `2025-11-25` (`docs/compatibility/hermes-agent.md:117`). The owner's always-on runtime
  sends `2025-06-18`, refuses any other answer and depends on the `mcp-session-id` header
  (R2 part 1, item 2). Nothing in CI fails if an rmcp upgrade drops either revision.
- **Wake format.** Connections with `wakeup_format = 'inbox'` get
  `{"event":"inbox.available","deliveryId","agentId","replyRequestId"}` and no stop events
  (`replies.rs:1380`, `:1444-1451`), covered by `tests/integration/reply_requests.rs:1032-1162`.
  The runtime above accepts only `inbox.available` and 404s anything else (R2 part 1, item 1).
  Saving the webhook on the Agents page silently switches the connection to typed wakeups
  (`external_agents.rs:691`), which would stop that runtime's wakes. The host command path can
  keep the old format (`external_agents.rs:471-505`, `typed_wakeups`).
- **Tools the connected runtimes call by name.** The always-on runtime wraps `briefing`,
  `inbox_list`, `message_send`, `reply_claim`, `reply_pass`, `reply_release`, `reply_send`,
  `task_list`, `task_open`, `task_create`, `task_update`, `task_comment`, `task_claim`,
  `task_release` and reads `outcome` (and `value` when it is `ok`). Hermes's allowlist is the
  18 tools in `tools/hermes-connector/verify-fixtures.py:20-39`, including `external_event_poll`.
- **Agent-facing URLs.** `/api/v1/mcp`, `/.well-known/oauth-protected-resource/api/v1/mcp`,
  `/.well-known/oauth-authorization-server` (`mcp/oauth.rs:38-42`), `/agent-runner.md`
  (`replies.rs:60`, built from `docs/agent-runner.md`, `replies.rs:76`), `/connect.md`
  (`connected_apps.rs:29`), `/api/public/v1/{tool}` and its `openapi.json`
  (`public_api.rs:47-48`).

K2 (new MCP revision, one envelope), K1 (actors to accounts), K4 (workspace in URLs) and A2
(one event stream) all touch these. P0 pins them with tests (P0.17) so a break is a red test,
not a silent loss of an agent.

### 2.3 Tests and CI

- 100 integration files, 95,025 lines, 369 tests in one binary
  (`tests/integration/main.rs:4-103`). Each PostgreSQL test makes a schema and runs all 130
  migrations (`mcp_admin.rs:24-64` is the common copy). 30 files define their own
  `schema_pool`, 40 build `AppState` by hand. The de facto shared helpers live in feature files:
  `agent_interface.rs:1069-1149` (`login`, `test_app`, `test_app_with`, `schema_pool`) and
  `mcp_admin.rs:16-80` (`Setup`, `setup`, `create_project`); the two-workspace world is in
  `workspace_isolation.rs:453-590` (`World`, `TwoWorkspaces`, `two_workspaces`).
- `src/test_support.rs` holds only `drop_schema`. Source scanners exist three times with
  different cut rules (`policy_inventory.rs:1837`, `modules/mod.rs:435`,
  `write_isolation.rs:657`, `workspace_isolation.rs:350-356`).
- CI makes one database per run (`tools/ci/checks.sh:418-424`) whose role can create
  databases; the cloud setup role has `CREATEDB` (`tools/cloud/setup.sh:114`).
- No timings are recorded anywhere (no JUnit output in `.config/nextest.toml`).
- Keep exactly as they are: the two isolation nets, the policy inventory, the agent-parity
  guard, the no-silent-skip guard (`tests/integration/main.rs:108-118`), the migration naming
  guard (`m0_database.rs:17-57`) and `drop_schema`'s lock-aware teardown.

### 2.4 Documents and names

- `README.md:3-7` calls `PROJECT_SOURCE_OF_TRUTH.md` (8,297 lines, "Status: M0 through M10
  accepted") the canonical plan; `README.md:33-44` describes the M0 seed and walking skeleton.
  `docs/task-platform/` (14 files) describes a separate task app with a `packages/` contract
  that doesn't exist. `docs/acceptance/m1.md`-`m10.md` are acceptance-evidence documents
  `AGENTS.md` forbids. Pointers to the old canonical plan: `README.md`, `AGENTS.md`,
  `docs/plans/workspace-merge-plan.md`, `docs/task-platform/{README,HANDOFF,decision-log}.md`.
- Files code reads, which must not move: `docs/agent-runner.md` (`replies.rs:76`),
  `docs/plans/roadmap.json` (`ai/plans.rs:1414`), `docs/self-hosted-red-hat.md`
  (`tools/verify-self-hosted-guide.sh:4`).
- `AGENTS.md` points to the AWD project pages and `docs/plans/roadmap.md` as the direction; it
  tells authors `conversation_visible_to` is the access rule (item 21).
- Vendor names in agent-facing text: `run_start` "Start Codex on your own Codex account"
  (`mcp.rs:3113-3116`), `run_get`, `run_list`, `run_request`, approvals and archive refusals
  (`mcp.rs:687`, `:1331`, `:2555`, `:3152`, `:3260`, `:3577-3579`, `:3608`, `:3629`, `:3696`),
  stage texts (`mcp/runs.rs:23`, `:257-288`, `:443`, `:479`), the run cap error
  (`api.rs:288`, `:593`), `error.rs` action `connect_personal_codex_account`.
- Personal machines and repositories: `tools/ci/run.sh:67,93` (private network addresses),
  `link_previews/guard.rs:16` (User-Agent naming a personal repository), `seed.rs:182-250`
  (dev seed names a personal GitHub repository and id), `docs/local-codex-mcp.md:12,98,120,126`.
- Dead code agents keep reading: ten `#[cfg(any())]` blocks in `api.rs` (`:132`, `:193`,
  `:196`, `:1069-1446`, `:1448`, `:1465`, `:1483`, `:1547`, `:1565`, `:2531`); three spikes
  that are workspace members and are built and tested on every run but used by nothing
  (`Cargo.toml:2-9`; one fixture read at `github.rs:5451`); the `jobs.rs:11-17` header
  describing an idempotency key nobody passes (`jobs.rs:223-232`); `seed.rs:358-383` writing a
  dropped column behind a dead branch. Public runner SDKs (`sdks/runner-*`) aren't run by CI.

## 3. The target design

P0 adds no new concepts. Its design choices are small and are given here so slices can cite
them.

### 3.1 Outbox side effects are part of the acknowledgement

`notify_people` and `queue_event_route` return `Result<(), AppError>`. In
`publish_ready_outbox_batch` (`realtime.rs:713-760`):

```rust
let side_effects = notify_people(&mut transaction, &claim).await
    .and(queue_event_route(&mut transaction, &claim).await);
if let Err(error) = side_effects {
    let _ = transaction.rollback().await;
    record_outbox_failure(pool, &claim, worker_id, OutboxFailureKind::SideEffectsUnavailable).await;
    continue;
}
// then mark_published as today
```

`OutboxFailureKind::SideEffectsUnavailable` is not permanent (`outbox.rs:711-713`), so the
event is retried with the existing backoff up to `DEFAULT_MAX_OUTBOX_ATTEMPTS` (10,
`outbox.rs:10`) and then marked failed with its summary. Retry is safe: notifications dedupe on
`dedupe_key` (`notifications.rs:162-255`), routing on its job unique key, and browsers dedupe
the re-broadcast on `event_id`. Alternative considered: enqueue one `events.fan_out` job and do
both from the job queue. Better long term and exactly K3's direction, but it moves code K3 will
rewrite; the five-line change is enough now.

### 3.2 Background loops log and carry on

`scheduler::run` and `github::run_jobs` match the outbox publisher (`realtime.rs:651-657`):
`if let Err(error) = drain(&state).await { tracing::error!(error_category = error.log_category(), "…"); }`.
A returned `Err` from the loop function itself then only means shutdown or a programming error,
and `main.rs` keeps treating an unexpected return as fatal. Panics still surface through the
`JoinError` path.

### 3.3 One rule for managing an agent

```rust
// external_agents.rs
/// The agent connection and its home project, when `manager` may manage it:
/// an active person who is an enabled owner or admin of the agent's workspace
/// and whose role in the home project allows ExternalAgentManage.
pub(crate) async fn may_manage(
    connection: &mut PgConnection,
    manager: Uuid,
    connection_id: Uuid,
) -> Result<Option<ManagedAgent>, AppError>;

pub(crate) struct ManagedAgent { pub connection_id: Uuid, pub project_id: Uuid,
    pub agent_actor_id: Uuid, pub organization_id: Uuid }
```

It is the web rule (`authorize_management`, `external_agents.rs:1282-1314`), unchanged, moved
behind one function that reads the rows itself. The OAuth consent list, `managed_agent`, the
connected-apps list and `owned_grant` call it (lists fetch at most 100 candidates, then filter in
Rust). The SQL copies are deleted. The stricter of the two rules is kept because acting as an
agent is an escalation path; K1 later replaces the body with its access function and a
workspace-scoped `agents.manage`. The people-only part already comes from the catalogue
(`policy.rs:512-520`, `people_only(…AgentsManage…)`), so this adds no kind check.

### 3.4 Gate scopes for content named by id

`modules/gate.rs` gets a second way to find a scope: a table of id-routed families, each with
the SQL that resolves the id to its project (or workspace), cached like `PROJECTS`:

```rust
const ID_FAMILIES: &[(&str, &str, &str)] = &[
    // (path prefix, parameter, lookup returning (project_id))
    ("/api/v1/pages/", "page_id", "SELECT project_id FROM project_pages WHERE id = $1"),
    ("/api/v1/whiteboards/", "whiteboard_id",
     "SELECT project_id, organization_id FROM whiteboards WHERE id = $1"),
];
```

(Exact parameter names and the whiteboard scope columns are read from the routes and
`20261017070000_whiteboards.sql` when the slice is built.) An id that resolves to nothing passes
through to the handler, as an unknown project does today. `GET /api/v1/whiteboards` passes the
caller's switched-on set instead of `on_in: None` (`whiteboards/http.rs:112`). Role narrowing
(`modules/state.rs:291` `narrowed`, `:319` `for_member`) is applied in the gate only if the
owner says a role's `modules_off` is a permission (question 1); the slice is shaped so that is
one call.

### 3.5 Rate limits that don't let one caller block everyone

Using the limiter's existing non-spending check (`FixedWindowLimiter::spent_at`,
`rate_limit.rs:466-476`) and the existing "recent good keys" pattern used for token refresh
(`RECENT_OAUTH_CLIENT_TTL`, `rate_limit.rs:49-52`):

- Sign-in: per-sender and per-name buckets as today. The global bucket becomes
  `global_login_failures`, charged only after a failed password check. While it is spent, only
  names that signed in successfully on this process in the last 7 days (`recent_logins`, a
  bounded `RecentKeys`) are let through to their per-name limits; everyone else is refused.
  The Argon2 work stays bounded by `CONCURRENT_PASSWORD_KDFS` (`rate_limit.rs:72`).
- MCP and public API: per-token buckets as today. `global_mcp_auth_failures` is charged only
  when the token doesn't resolve. While it is spent, only tokens that authenticated in the last
  24 hours (`recent_mcp_tokens`, digests only) are looked up.
- The client address header is a setting, `AGENT_WORKSPACE_CLIENT_ADDRESS_HEADER`, default
  unset (no per-address limits), with `cf-connecting-ip` documented as the value for that proxy.
  Deploy note: the live server sets it, or loses per-address sign-in limits.

These are per-process, as today; K5 moves limiters to shared storage. Alternative considered:
drop the global ceilings entirely. Rejected because, with no trusted address, they are the only
bound on credential stuffing across names.

### 3.6 OAuth registration

The cap counts only unclaimed registrations (no code ever issued, no grant), which is the only
part an anonymous caller can grow; registered-and-authorized clients are bounded by real people
authorizing them. A per-sender registration bucket is added when the address header is set.
Revoked grants are kept (history attribution); K2 moves sign-in to Client ID Metadata Documents,
where registration goes away.

### 3.7 Contract fixtures for connected clients

`apps/server/tests/fixtures/compat/connected-clients.json`:

```json
{
  "mcpRevisions": ["2025-06-18", "2025-11-25"],
  "urls": ["/api/v1/mcp", "/.well-known/oauth-protected-resource/api/v1/mcp",
           "/.well-known/oauth-authorization-server", "/agent-runner.md", "/connect.md",
           "/api/public/v1/openapi.json"],
  "clients": {
    "always-on-runtime": {
      "wake": "inbox.available",
      "tools": {"briefing": [], "inbox_list": [], "message_send": ["conversationId", "text"],
                "reply_claim": ["requestId"], "reply_send": ["requestId", "text", "generation"],
                "...": []}
    },
    "hermes-0.20": { "wake": "poll", "tools": {"external_event_poll": [], "...": []} }
  },
  "retireWhen": "docs/compatibility/connected-clients.md"
}
```

Argument names per tool are copied from the clients' current calls when the slice is built
(the runtime's calls are in its `crates/workspace/src/{api,tasks}.rs`; Hermes's from its fixture).
The fixture names no person or machine.

### 3.8 Structure guards with shrinking allowances

One test module, `apps/server/src/structure_guards.rs` (`#[cfg(test)]`), and one web test,
`apps/web/src/structure.test.ts`, each run a list of rules over the source tree using the shared
scanner from P0.24. Each rule counts matches per file, skipping comment lines and the
`#[cfg(test)] mod tests` tail. Allowances live in a data file, one entry per line so parallel
edits merge:

```json
// apps/server/src/structure_guards/allowances.json
{
  "raw_role_checks": {
    "account_security.rs": 2,
    "api.rs": 2,
    "workspace.rs": 5
  },
  "agent_runs_sql": { "api.rs": 21, "provider.rs": 19 }
}
```

The test fails when a file's count is **above** its allowance (or a file with no entry has any
match), and when the sum of a rule's allowances is above that rule's ceiling (a constant in the
test, set when P0 lands). It never fails when a count is below its allowance; it prints "you can
lower X to N". This keeps the list mergeable in any order (the steward rule "never pin a count
other branches change" holds: two branches that each remove an offender never break `main`),
while new offenders always fail. Moving code between files moves allowance in the same pull
request; the ceiling stops the total from growing.

Rules and their baselines at `fe95a74` (counted with the matchers below; the slice records the
exact numbers it finds):

| Rule id | Matches | Files today | Lines today |
|---|---|---|---|
| `raw_role_checks` | SQL `role IN ('owner'`, `role = '<builtin>'`, `role <> '<builtin>'` | 21 | about 40 |
| `custom_role_allows` | `custom_role_allows(` in Rust | 18 | 29 |
| `workspace_role_enum` | `WorkspaceRole::`, `.manages()` outside `workspace.rs` | 3 | small |
| `account_kind_checks` | `actor_kind ==/!=`, `ActorKind::Human` comparisons, SQL `kind = 'human'`/`'external_agent'`/`'execution_worker'`, unless the line carries `// kind-check: mechanical (<reason>)` | about 50 | about 215 |
| `agent_runs_sql` | `FROM`/`JOIN`/`INTO`/`UPDATE` + `agent_runs`, `agent_run_events`, `agent_run_usage`, `agent_approvals` | 34 for `agent_runs` alone | about 130 |
| `active_run_states_literal` | the literal `'created','queued','starting'` list | 12 | 18 |
| `conversation_inserts` | `INSERT INTO conversations` | 6 | 11 |
| `message_inserts` | `INSERT INTO messages` | 4 | 5 |
| `task_core_updates` | `UPDATE work_items` outside `work_item_commands.rs` | 8 | 10 |
| `direct_tool_calls` | `call_tool_direct(` outside `mcp.rs` | 3 | 4 |
| `process_state` | `static … Mutex`/`RwLock`, `Mutex<HashMap` | 10 | 15 |
| `background_spawns` | `tokio::spawn(` outside `main.rs` | 10 | 11 |
| `link_tables` | a migration with a timestamp after P0's baseline that creates a table named `*_links`, `*_references` or `*_relations` | 0 | 0 |
| `comment_tables` | a migration after the baseline that creates a table whose name contains `comment`, `annotation`, `remark` or `note_thread` | 0 | 0 |
| `notification_detail_text` | a `NewNotification` `detail` (or `notification.detail =`) whose JSON has a `preview`, `body`, `text` or `quote` key built from anything but a constant or an id (P0.36) | after P0.36: `scheduled_messages.rs`, `automations.rs`, `usage/alerts.rs` (system text, each with a reason) | 4 |
| `vendor_names_core` | `Codex`, `Claude` (and the other adapter display names) inside string literals outside `provider/`, `provider.rs`, `ai/providers/`, `harnesses.rs` | 15 | about 60 |
| `personal_hosts` | private-network address literals (100.64.0.0/10) and `/home/<name>/` paths in tracked files outside `docs/history/` | as found | as found |

Web rules (`apps/web/src/structure.test.ts`, same allowance format in
`apps/web/src/structure.allowances.json`):

| Rule id | Matches | Files today |
|---|---|---|
| `web_role_name_gates` | `role === 'owner'`, `=== 'admin'`, `=== 'viewer'`, `!==` forms | 19 |
| `web_vendor_names` | `Codex`, `Claude` in non-test source | 10 |

Matchers are substring checks after whitespace normalisation (the server crate has no regex
dependency, and none is added for a test).

## 4. Migration path

P0 changes no data model except two small, independent migrations:

- P0.36 strips copied text from existing page-comment and page-suggestion notification rows
  (`UPDATE notifications SET detail = detail - 'preview' - 'title' WHERE kind IN
  ('page_comment','page_suggestion')`): one statement, idempotent, safe to rerun, no order
  dependency.
- P0.28 adds one SQL function, `conversation_readable_by(conversation, actor)`, whose body is
  a single call to `conversation_visible_to` (ruling R32): a name for callers, not a second
  rule. No data changes.
- No other slice needs a migration. P0.4's comments write the existing projection tables; old
  comments written by agents stay plain text (no backfill: their text never named a mention
  node, and guessing mentions after the fact would notify people about old comments).

Old and new paths: P0.11 and P0.12 change the activity payload written for GitHub moves and
board drags to the `update_locked_work_item` shape. Readers keep their branches for the old
dialects (`work_item_history.rs:377-394`, `mcp/history.rs:179`, `:681`, `:777`) because old rows
stay; those branches are deleted only by T1 when task history is converted.

Docs moves (P0.31) keep every file that code reads in place, and leave a one-line stub at each
moved path's old location only where an external link is likely (`PROJECT_SOURCE_OF_TRUTH.md`).

## 5. Slices

Order: silent correctness bugs first, then shared limits, compatibility pins, tests and CI,
rules and guards, then documents. Slices touching the same files are marked "not beside".

### Group A: silent correctness

**P0.0 The sidebar shows only conversations the caller can see** (S, first)
Numbered P0.0 so the other slice ids stay as they were; it lands before everything else.
- Test first, failing on today's `main`: `tests/integration/direct_conversations.rs` new test
  `sidebar_never_lists_other_peoples_direct_threads` (its `visible` helper and setup at
  `:25`, `:611` are the model). People A, B and C in one workspace, all of whom have opened
  Messages (so all are members of the direct project); a DM between A and B with a message and
  a thread started from it, named from that message's text; a group of A, B and an agent with
  a thread too. C calls `GET /api/v1/projects/<direct project>/conversation-sidebar` with
  `includeArchived=true`, and also with `selectedChannelId` and `expandedChannelId` set to the
  DM's id. Assert the response contains none of the threads' ids, none of their names (search
  the raw body), no unread or run state for them. A calls the same route and sees the DM
  thread, so the fix doesn't hide too much. Also call `GET /api/v1/projects/<id>/channels`
  for the direct project as C and assert no DM or group appears.
- Fix: in `project_conversation_sidebar`, add `AND conversation_visible_to(thread.id, $2)` to
  the thread query (`conversations.rs:3520`) and `AND conversation_visible_to(channel.id, $2)`
  to the channel query (`:3434`); the same on `list_channels` (`:813`). `$2` is already the
  caller's actor id in all three. This is correct here because `authorized_project` has
  already checked membership. The function's body is not touched (ruling R3).
- Guard (rule 7): a new `conversation_listings` rule in the server structure guard. It scans
  every SQL string literal in `apps/server/src` that reads `FROM conversations`,
  `JOIN conversations`, `FROM messages` or `JOIN messages` and doesn't contain
  `conversation_visible_to(` (or, once K1 lands it, K1's generated filter), and counts them
  per file. This is the one guard for this rule: C1's guard 3, C2's "listing lint" and K4's
  G2 (`conversation_queries_check_visibility`) are this guard, not three more. Today's count is the allowance (for
  example `conversations.rs`, `api.rs`, `projects.rs`; the planner's rough count is about 40
  such literals across 30 files, most of them by-id lookups after an access check, workers and
  inserts). A query that is safe without the rule carries a SQL comment
  `-- visibility: <why>` (for example `-- visibility: by id, caller checked by conversation_access`)
  and is not counted. The allowance only shrinks. If P0.26's shared guard file isn't merged
  yet, this slice adds the rule as a standalone test in `tests/structure_guards.rs` (or
  wherever P0.24 puts the scanner) and P0.26 absorbs it.
- Also: add the sidebar and channel-list routes to the in-workspace privacy net's route list
  when P0.22 builds it (P0.22 names them; this slice's test stays as the specific regression).
- Changes: `conversations.rs` (three `WHERE` clauses), the test, the guard.
- Depends on: nothing. Beside: all (P0.28 touches no query).
- Acceptance: the new test fails on `main` and passes after; the existing
  `workspace_isolation.rs` sidebar cases (`:2762-2772`) still pass; adding a scratch
  `SELECT name FROM conversations WHERE project_id = $1` to a file fails the guard.
- Afterwards, owned elsewhere: K1 folds the listing filter into its single access path; C2 (or
  whoever adds private channels) relies on the channel filter added here.

**P0.1 Events keep their side effects** (S)
Make the outbox publisher retry an event whose notifications or webhook/automation routing
failed, instead of marking it published (design 3.1).
- Changes: `realtime.rs` (`notify_people`, `queue_event_route` return `Result`;
  `publish_ready_outbox_batch`), `outbox.rs` (`OutboxFailureKind::SideEffectsUnavailable`,
  its `operator_summary`, not permanent).
- Depends on: nothing.
- Acceptance: `tests/integration/m2_outbox_publisher.rs` new test
  `an_event_whose_notifications_fail_is_retried_not_published`: create a mention; in the test's
  schema rename `notifications` away; run `publish_ready_outbox_batch`; assert the event has
  `published_at IS NULL`, `attempt_count = 1` and a failure summary; rename back, set
  `next_attempt_at = now()`, publish again; assert one notification row exists and the event is
  published. Same shape for routing by renaming `background_jobs`. Existing outbox tests pass.

**P0.2 A database blip doesn't stop the server** (S)
- Changes: `scheduler.rs:80-92`, `github.rs:141-155` (design 3.2).
- Depends on: nothing. Beside: everything.
- Acceptance: `tests/integration/m3_scheduler_http.rs` new test
  `scheduler_keeps_running_through_a_database_error`: run `scheduler::run` on a pool whose
  schema has `agent_runs` renamed; after three ticks the task is still running; restore the
  table, queue a run, see it claimed; send shutdown, `run` returns `Ok(())`. Same for
  `github::run_jobs` with `github_jobs` renamed, in `m5_github_foundation.rs`.

**P0.3 CI tells the truth** (S)
- Changes: `.config/nextest.toml` (`retries = 0` in `profile.ci`; add `[profile.ci.junit]` with
  `path = "junit.xml"` so per-test times are kept); `apps/web/playwright.config.ts` (fail on
  flaky: `failOnFlakyTests: true` if the pinned Playwright supports it, else `retries: 0`);
  `tools/ci/changed-areas.sh` (before the `apps/web/*` case, map
  `apps/web/src/experience/registry.json`, `apps/web/src/ui/markdown/*.json`,
  `apps/web/src/memory/live/*.json` and `apps/web/src/settings/workspace/roles.ts` to both
  areas); new `tools/ci/test_changed_areas.py` (runs the script against a temporary git repo).
- Before switching retries off: grep the last two weeks of `main` CI logs for `FLAKY`, and fix
  each flaky test in this slice or give it a named override
  (`[[profile.ci.overrides]] filter = "test(=<name>)" retries = 1`) with a comment saying why.
  Race, claim, lease, bootstrap and credential tests never get an override.
- Depends on: nothing.
- Acceptance: `test_changed_areas.py` asserts a change to each listed web file yields
  `areas=rust,web`, a change to `apps/web/src/App.tsx` yields `areas=web`, and a guard in the
  same file fails if any `include_str!` or `concat!(env!("CARGO_MANIFEST_DIR"), "/../web/…")`
  in `apps/server/src` names a web path the script doesn't map (it greps the server source).

**P0.4 Agents' task comments mention and reference** (S)
- Changes: `mcp/writes.rs:1242-1400` builds content with `common::content_from_text`
  (`mcp/common.rs:517`), extended to turn task keys into `WorkItemRef` nodes the way the web
  composer does; then `work_items::comments::resolve_comment_content` and a now
  `pub(crate)` `insert_projections` (`work_items/comments.rs:95`, `:185`) write
  `work_item_comment_mentions` and `work_item_comment_references`. `mcp/comment_tools.rs:150`
  uses the same content builder. `policy_inventory.rs:1448` label becomes `WorkItemEdit`.
- Depends on: nothing. Not beside P0.12, P0.15, P0.16 (same files).
- Acceptance: `tests/integration/mcp_tasks.rs` new test
  `an_agents_task_comment_mentions_people_and_references_tasks`: agent posts "@rook see
  KEY-2" through `task_comment`; Rook has an unread mention notification for it; KEY-2's
  "Referenced in" lists the comment; `task_comment_edit` to "@owner" moves the mention.
  Replaying the same `clientRequestId` creates no second notification.

**P0.5 Reading through an app clears notifications** (S)
- Changes: extract `read_state::mark_seen(tx, conversation: &LockedConversation, actor,
  sequence) -> Result<Option<ReadState>, AppError>` (new `apps/server/src/read_state.rs`, the
  name C2.2 and K2.7 already use, so neither renames it) from `mark_conversation_seen`
  (`conversations.rs:2696-2799`): upsert, `ReadStateUpdated`, `read_for_conversation`. The
  HTTP handler and `mcp/talk.rs:1395` (`mark`, used by `mark_read` and `conversation_read`
  with `markRead`) call it.
- Depends on: nothing.
- Acceptance: `tests/integration/notifications.rs` new test
  `reading_through_a_connected_app_reads_the_mentions`: a person connected through OAuth is
  mentioned; `mark_read` through the app; their notification has `read_at` set and the inbox
  count drops; same through `conversation_read` with `markRead: true`.

**P0.6 One rule for managing and acting as an agent** (S)
- Changes: `external_agents.rs` (`may_manage`, design 3.3; `authorize_management` calls it);
  `mcp/oauth.rs:1272-1340`; `connected_apps.rs:100-155`; `mcp/agents.rs:439-480`.
- Depends on: nothing. Not beside P0.14 (`mcp/oauth.rs`).
- Acceptance: `tests/integration/oauth_agent.rs` new test
  `only_people_who_manage_an_agent_in_its_workspace_can_act_as_it`: a workspace *member* who is
  an explicit project admin (`role_explicit`) with `agents.manage` sees no agent on the consent
  screen, is refused when posting the consent with the agent's connection id, doesn't see the
  agent's grants in connected apps and can't revoke them; a workspace admin who is a project
  admin can do all four. Also asserts `agent_list`'s manage view agrees.

**P0.7 The module gate covers pages and boards named by id** (S)
- Changes: `modules/gate.rs` (design 3.4), `whiteboards/http.rs:112`; a guard in
  `modules/mod.rs` tests: every route whose manifest module can be switched off and whose
  template starts with `/api/v1/<family>/{` is in `ID_FAMILIES` or in an `ID_EXEMPT` list with a
  reason (today: conversations, channels, threads, messages, files, later, questions,
  reply-requests, agent-runs, approvals, scheduled-messages, bug-report links, forms, webhooks
  each get a one-line reason or a lookup).
- Depends on: nothing. Owner question 1 decides the role-narrowing call.
- Acceptance: `tests/integration/modules.rs` new tests: with Pages off in the workspace,
  `GET /api/v1/pages/{id}`, `PATCH /api/v1/pages/{id}` and the page document routes answer
  `module_off`; with Whiteboards off, `GET /api/v1/whiteboards/{id}` does and the list omits
  that workspace's boards; with both on, nothing changes. If the owner says yes to question 1,
  the same with the module off only in the caller's role.

**P0.8 SQL permission helpers respect the base role** (S)
- Changes: `roles::role_allows_sql` and `search::projects_allowing` add
  `AND <membership>.role = ANY(<roles whose built-in set holds permission>)`, the list computed
  from `PermissionSet::built_in` (`policy.rs:629-642`) in Rust and inlined as literals (they are
  ours, not input). `role_allows_sql`'s doc says what it checks.
- Depends on: nothing.
- Acceptance: unit tests in `roles.rs`: `role_allows_sql` for `tasks.edit` excludes `viewer` and
  `worker`; for `tasks.view` includes `owner`, `admin`, `member`, `viewer`. Integration in
  `roles_and_access.rs`: `projects_allowing(viewer, …, TasksEdit)` is empty for a viewer and for
  a worker membership.

**P0.9 Refusals match each tool's schema; the public API maps every outcome** (S)
- Changes: `mcp.rs:3967-3995` `refusal` emits `retry_category` for tools whose output schema
  names it (today `repository_get`, `pull_request_get`) and `retryCategory` otherwise.
  `public_api.rs:118-127` maps `rejected` to 422, `busy` to 429, `cursor_expired` to 410,
  `applied`/`found`/`ok` to 200, anything unknown to 500 (not 200).
- Depends on: nothing.
- Acceptance: unit test in `mcp.rs`: for every tool in `tool_catalog()`, the generic refusal
  validates against that tool's output schema (a small structural check: required keys present).
  Unit test in `public_api.rs`: every `outcome` value that appears in any tool's output schema
  has an explicit status (fails on a new outcome without one).

**P0.10 Wake-ups are posted outside the transaction** (S)
- Changes: `replies.rs:1315-1438`: claim up to 10 due deliveries with
  `next_attempt_at = now() + interval '30 seconds'` in a short transaction and commit; post
  outside; record each result in its own short transaction (the outbox publisher's shape).
  The lease sweep (`replies.rs:1248`) no longer waits behind posts.
- Depends on: nothing. Not beside P0.18 (same file region is close; merge is easy, but test
  together).
- Acceptance: `tests/integration/reply_requests.rs` new test
  `a_slow_runner_doesnt_hold_a_database_connection`: a webhook server that sleeps 5 s; while it
  sleeps, a `SELECT … FOR UPDATE NOWAIT` on that delivery row succeeds and the pool's idle count
  is unchanged; the delivery is recorded afterwards. Existing wake-up tests pass, including the
  `inbox.available` one.

**P0.11 GitHub-driven task moves go through the task command** (S)
- Changes: `github.rs:4239-4335` locks the project first (`work_item_commands::lock_membership`
  as the integration actor) and calls `update_locked_work_item` with
  `CommandActor { audit: AuditAttribution::Job { job_id, source: "github_automation" }, … }`
  (`work_item_commands.rs:39-66`); `APPLY_AUTOMATION_STATUS_SQL` (`github.rs:56-61`) goes.
- Depends on: nothing. Not beside P0.12 (both change task history output).
- Acceptance: `tests/integration/m5_github_foundation.rs` new test
  `a_merged_pull_request_moves_its_task_like_a_person_would`: the task's activity has
  `changedFields: ["status", "position"]`, the assigned agent gets its `work_item.changed`
  event, a watching agent is notified, the card is at the bottom of the new column, and the
  `task.updated` webhook carries `changedFields`.

**P0.12 Board drag, mark-blocked and ready-for-review go through the task command** (M)
- Changes: `work_items.rs:2158-2322` (`move_work_item` uses `update_locked_work_item` with
  `board_placement`, `work_item_commands.rs:820-845`, keeping its response shape);
  `mcp/writes.rs:1640-1745` and `:1747-1897`. `task_reorder` (`mcp/tasks.rs:2966-3020`) stays
  as it is (deliberately versionless).
- Depends on: P0.11 (same readers), P0.4 (same file).
- Acceptance: `tests/integration/w3_tasks_http.rs` new test
  `every_status_change_says_what_changed`: board drag, `task_mark_blocked`,
  `task_mark_ready_for_review` and `task_update` with the same change each write an activity
  with `changedFields` and before/after, and each `task.updated` webhook carries them.
  Existing board and MCP task tests pass unchanged.

### Group B: limits that every workspace shares

**P0.13 Valid callers never share a global budget** (S)
- Changes: `rate_limit.rs` (design 3.5: `global_login_failures`, `recent_logins`,
  `global_mcp_auth_failures`, `recent_mcp_tokens`, `note_login_failure`,
  `note_login_success`, `note_mcp_auth_failure`, `note_mcp_auth_success`; `client_address`
  reads the configured header); `auth/mod.rs:186-260`; `mcp.rs:4334-4345`;
  `public_api.rs:79`; `config.rs` (new setting); `docs/self-hosted-red-hat.md` and
  `.env.example` (the setting); the PR description names the deploy step.
- Depends on: nothing.
- Acceptance: unit tests in `rate_limit.rs`: 60 failed sign-ins for junk names don't refuse a
  name that signed in yesterday; they do refuse a never-seen name; 1,000 valid MCP requests
  across 20 tokens in a minute pass; junk tokens that spend the failure budget don't refuse a
  token that authenticated earlier. Integration in `m1_http_projects.rs` (sign-in) that the
  header setting is honoured and absent means no per-address limit.

**P0.14 OAuth registration counts only unclaimed clients** (S)
- Changes: `mcp/oauth.rs:182-205` (count only clients with no code and no grant; per-sender
  registration bucket in `rate_limit.rs`).
- Depends on: P0.13 (address setting). Not beside P0.6.
- Acceptance: `tests/integration/oauth_agent.rs` new test
  `registration_stays_open_with_many_authorized_clients`: insert 1,100 clients each with a
  revoked grant in one statement; registration succeeds. Insert 1,024 fresh unclaimed ones;
  registration is refused with today's message.

**P0.15 Safe retries on creates, part 1** (S)
- Changes: optional `clientRequestId` on `page_write` (create path), `decision_record`,
  `whiteboard_create`, `whiteboard_duplicate`, using `idempotency::reserve`/`complete` with a
  resource-bound scope as `task_comment` does (`mcp/writes.rs:1281-1298`). Files:
  `mcp/memory_tools.rs`, `mcp/whiteboard_tools.rs`, their argument structs.
- Depends on: nothing. Beside P0.16.
- Acceptance: one test per tool in the matching integration file (`awd53_live_pages.rs`,
  `w16_whiteboards.rs`, `agent_interface.rs` for decisions): the same call twice with one
  `clientRequestId` creates one thing and the second answer is the first's, with
  `idempotentReplay: true`; a different body with the same id is refused.

**P0.16 Safe retries on creates, part 2, and honest instructions** (S)
- Changes: same for `project_create`, `file_upload`, `message_schedule`, `task_from_template`,
  `milestone_write` (create). `SERVER_INSTRUCTIONS` (`mcp.rs:135`) says "tools that create
  something take a clientRequestId". A guard in `mcp.rs` tests: every tool whose name ends in
  `_create`, `_duplicate`, `_upload`, `_schedule`, `_record` or whose description starts with
  "Create" has a `clientRequestId` property in its input schema, or is in an allowance list that
  only shrinks (format of 3.8).
- Depends on: nothing.
- Acceptance: replay tests as in P0.15 in `mcp_admin.rs`, `w20_files.rs`,
  `w21_later_and_scheduling.rs`, `w13_payoff.rs` (templates), `mcp_planning.rs`; the guard test.

### Group C: keep connected agents working

**P0.17 Contract tests for connected clients** (S)
- Changes: new `tests/integration/compat_clients.rs` and
  `tests/fixtures/compat/connected-clients.json` (design 3.7); new
  `docs/compatibility/connected-clients.md` listing each contract, which client relies on it,
  and the condition and owning workstream for retiring it (the wake format: when A2's typed
  stream ships and the always-on runtime accepts typed events; `2025-06-18`/`2025-11-25`: not
  before 12 months after K2 serves `2026-07-28`, and only with the owner's sign-off).
  The existing `inbox.available` test (`reply_requests.rs:1032`) gets a doc comment pointing
  at the contract.
- Tests in the new file:
  - `old_mcp_revisions_still_shake_hands`: for each revision, `initialize` answers that same
    `protocolVersion` and an `mcp-session-id` header; `tools/list` and one `tools/call` work in
    that session; a call with an unknown but well-formed session id still works (resume).
  - `connected_clients_find_their_tools`: each listed tool is in `tool_catalog()`, every listed
    argument is a property of its input schema, and its output schema has `outcome`.
  - `agent_facing_urls_answer`: each URL answers 200 (or 401 with the resource-metadata header
    for the MCP endpoint without a token).
- Depends on: nothing.
- Acceptance: the three tests pass on today's code; an rmcp bump that drops a revision fails
  the first.

**P0.18 Saving a webhook keeps its wake format** (S)
- Changes: `external_agents.rs:680-700` keeps `wakeup_format` unless the request says
  `"wakeupFormat": "typed"`; the web Agents page (`apps/web/src/external-agents/ExternalAgentSettings.tsx`
  and its `api.ts`) shows the format for an `inbox` connection with a "Switch to typed
  wake-ups" control that sends it. New connections stay typed (`external_agents.rs:301`).
- Depends on: nothing.
- Acceptance: `tests/integration/reply_requests.rs` new test
  `saving_an_old_webhook_keeps_its_wake_format`: an `inbox` connection whose URL is saved again
  still receives `inbox.available`; with `wakeupFormat: "typed"` it receives `reply.requested`.

### Group D: tests and CI that every workstream builds on

**P0.19 Race tests run on today's schema** (S)
- Changes: `m2_conversations_database.rs` tests at `:556` and `:672` and
  `m3_execution_foundation_database.rs:831` use the full `sqlx::migrate!` chain and current
  helpers, keeping their concurrency logic; the migration naming guard moves from
  `m0_database.rs:17-57` to new `tests/integration/migrations_guard.rs` (unchanged); the
  `add-migration` skill points at the new file.
- Depends on: nothing.
- Acceptance: the three tests pass against the full chain; deliberately reverting the latest
  `validate_conversation_parent` body in a scratch branch makes the archive test fail (checked
  once by the slice author, noted in the PR).

**P0.20 Shared integration fixtures** (M)
- Changes: new `tests/integration/support/` (`mod.rs`, `db.rs`, `app.rs`, `callers.rs`,
  `world.rs`, `two_workspaces.rs`); move, unchanged, `test_app`, `test_app_with`,
  `schema_pool`, `login`, `Browser`, `send`, `tool`, `initialize`, `create_agent`,
  `connect_app` from `agent_interface.rs`, `Setup`/`setup`/`create_project` from
  `mcp_admin.rs`, and `World`/`TwoWorkspaces`/`two_workspaces` from `workspace_isolation.rs`;
  leave `pub(super) use` re-exports in the old files so the ~50 importers compile unchanged.
  Add what other workstreams will need:
  ```rust
  pub struct Db { pub pool: PgPool, pub url: String /* drop on finish */ }
  pub async fn db(name: &str) -> Option<Db>;                // None = no database configured
  pub fn app(db: &Db) -> AppBuilder;                         // one AppState builder
  pub struct Person { pub actor: Uuid, pub browser: Browser }
  pub struct Agent  { pub actor: Uuid, pub connection: Uuid, pub token: String }
  pub struct ConnectedApp { pub person: Uuid, pub token: String } // a person through OAuth
  pub trait Caller { async fn call(&self, app: &Router, op: Call) -> (StatusCode, Value); }
  // Call::Http { method, path, body } | Call::Tool { name, arguments } | Call::PublicApi { .. }
  pub struct WorldBuilder; // .project(key) .person(role) .agent(role) .custom_role(perms)
                           // .private_group(&[members]) .dm(a, b)
  pub async fn drain_outbox(app_state: &AppState);           // publish_ready_outbox_batch until empty
  pub fn assert_absent(body: &Value, markers: &[&str]);
  ```
  `Caller` lets a test run the same operation as a person, an agent and a connected app, which
  K1, K2 and A2 will use for "no difference by account kind" tests. Add one sentence naming
  `support/` to the `add-mcp-tool` and `steward` skills and to `AGENTS.md` (P0.26 text).
- Depends on: nothing. Every later test slice in every workstream depends on it.
- Acceptance: the whole suite passes with the moves; three files (`mcp_admin.rs`,
  `notifications.rs`, `roles_and_access.rs`) switch to `support::` directly as examples; a new
  test `support::world::a_world_has_every_kind_of_caller` builds a world and calls one read as
  each caller.

**P0.21 One migrated template per run; fast test databases; timings** (M)
- Changes: `support/db.rs`: the first test to need a database takes
  `pg_advisory_lock(<hash>)` on the base URL, creates `aw_tpl_<12 hex of the migration
  checksums>` if missing and migrates it, then releases (a session lock on a dedicated test
  connection outside the pool, because `CREATE DATABASE` can't run in a transaction; test
  code, the one listed exemption in K5's `session_advisory_locks` guard, which ruling R24
  rule 3 otherwise forbids); every test runs
  `CREATE DATABASE aw_t_<uuid> TEMPLATE aw_tpl_<…>` and connects to it, with
  `SET synchronous_commit = off` in `after_connect`; teardown is
  `DROP DATABASE … WITH (FORCE)`. A test that can't create databases (no `CREATEDB`) falls back
  to today's schema-and-migrate path with one log line. `tools/ci/checks.sh` drops
  `aw_tpl_*` and `aw_t_*` databases older than the run at start, like stale run databases
  (`checks.sh:415-421`). Files that still build their own schema keep working; they move to
  `support::db` as they are touched (no mass edit).
- Depends on: P0.20.
- Acceptance: `support::db` tests: two concurrent first calls create one template; a changed
  migration set gets a new template name; a test database has every migration recorded in
  `_sqlx_migrations`. The PR description reports the integration run time before and after from
  the JUnit file (P0.3), on the same host. Expected (inferred, not measured): most of the
  per-test setup cost goes.

**P0.22 A privacy net inside a workspace** (M)
- Changes: extend `TwoWorkspaces`' world A (in `support/`) with `outsider` (a project member
  in no DM or group with the private content) and `narrowed` (a member on a custom role without
  `tasks.view` and `code.view`), and a second marker in private content (a DM, a group, a thread
  under the group, files attached there) and a third in task and code content. New
  `tests/integration/privacy_inside_workspace.rs` walks every GET route (from the shared scanner,
  P0.24) and every read-only tool (the same list `workspace_isolation.rs` uses) as `outsider`
  and as `narrowed`, with A's own ids, asserting the private marker and private ids never
  appear for `outsider`, and the task/code marker never appears for `narrowed`. Covers
  `ai_summarize` on the DM (today untested inside a workspace, `src/ai/summaries.rs:102-137`).
  Every project-scoped route is walked with the direct project's id too, not only the main
  project's, since `outsider` is a member of it (the gap P0.0 fixed was there).
  A failure found here gets an entry in a `KNOWN_GAPS` list with a task reference, which only
  shrinks; the slice does not fix them.
- Depends on: P0.20, P0.24.
- Acceptance: the net runs, its coverage guard fails when a new GET route or read-only tool
  isn't walked (same as `workspace_isolation.rs:273-333`), and `KNOWN_GAPS` is reported to the
  lead.

**P0.23 Read-only tools change nothing** (S)
- Changes: in the reads net, take `write_isolation`'s row digest of workspace A before and
  after each tool marked `read_only_hint = true`, excluding columns that reads legitimately
  touch (`last_used_at` on credentials and grants, handshake logs); fail with the tool name.
- Depends on: P0.20.
- Acceptance: the test passes today or lists each mislabelled tool in a shrinking allowance.

**P0.24 One source scanner for every guard** (S)
- Changes: `src/test_support.rs` gains `pub mod source` with `rust_files(root)`,
  `strip_tests(source)` (one rule: cut at the first `#[cfg(test)]` that starts a module),
  `routes(source)` and `count(rule, source)`; `policy_inventory.rs:1820-1883`,
  `modules/mod.rs:435`, `write_isolation.rs:657`, `workspace_isolation.rs:350-356` use it.
- Depends on: nothing.
- Acceptance: the four guards find the same routes as before (the slice asserts the route sets
  are equal to the old scanners' on the current tree, then deletes the old scanners).

**P0.25 Retire upgrade replays for data that no longer exists** (S, needs owner question 2)
- Changes: delete only the tests whose code path can run nowhere today:
  `m0_to_m2_upgrade_preserves_history_and_enforces_conversation_invariants`
  (`m2_conversations_database.rs:28`), `m0_to_m1_identity_and_project_upgrade_enforces_invariants`
  and `m1_upgrade_rejects_incompatible_legacy_actor_roles_without_rewriting_them`
  (`m1_identity_projects_database.rs:8`, `:93`),
  `m2_to_m3_execution_foundation_adopts_rows_and_enforces_scope` and
  `m3_upgrade_rejects_ambiguous_legacy_profile_ownership`
  (`m3_execution_foundation_database.rs:12`, `:559`). Re-point instead of deleting anything that
  exercises live code: `bootstrap_*` (`m1_bootstrap_database.rs:13`, `:266`, bootstrap is live),
  `pre_m3_upgrade_repair_*` and `m3_repository_mirror_upgrade.rs` (they test
  `migration_preflight`, which runs at every start, `main.rs:42-60`),
  `m3_hardening_rejects_duplicate_host_runtime_paths` and
  `walking_skeleton_schema_enforces_core_invariants` (constraints that still exist).
- Depends on: P0.19, owner question 2.
- Acceptance: suite passes; the PR lists each removed test and why its path can't run.

### Group E: rules that stop the copying

**P0.26 Rules in `AGENTS.md` and the server structure guards** (M)
- Changes: `AGENTS.md` gains the "Rules while the foundations are rebuilt" section (text in
  section 6 below) and the fixture pointer; `apps/server/src/structure_guards.rs` and
  `structure_guards/allowances.json` (design 3.8) for every server rule; `lib.rs` declares
  the test module. The `add-mcp-tool` skill gets one paragraph pointing at the rules.
- Depends on: P0.24. Lands after P0.4, P0.11, P0.12 if possible so their removed offenders
  aren't in the baseline (not required: allowances above the count never fail).
- Acceptance: unit tests in `structure_guards.rs`: each rule's matcher has a small positive and
  negative sample; the full scan passes on the tree; adding one `role IN ('owner','admin')` to
  a scratch copy of a file not in the list fails with a message naming the rule and the
  `AGENTS.md` heading; a file below its allowance passes and prints the hint.

**P0.27 Web structure guards** (S)
- Changes: `apps/web/src/structure.test.ts` and `structure.allowances.json` for
  `web_role_name_gates` and `web_vendor_names` (same rules as 3.8, using `import.meta.glob` or
  `fs` over `src/`, skipping `*.test.*`).
- Depends on: nothing.
- Acceptance: the test passes; a new `role === 'admin'` in a scratch file fails it.

**P0.28 Conversation access: the wording, a thin wrapper and the pairing test** (S)
- Ruling R32: `conversation_readable_by` stays, as a thin wrapper whose body is one call to
  `conversation_visible_to` and nothing else. R3 still holds: `conversation_visible_to` is the
  one gate, its body owned by K1 and composed of helpers each owned by one workstream; K4.4
  makes it check space and workspace membership itself. The wrapper adds no rule, so it can't
  drift. C2.1 uses this function and does not add its own (C2.1 points here).
- Changes: migration `apps/server/migrations/<UTC timestamp>_conversation_readable_by.sql`:
  `CREATE FUNCTION conversation_readable_by(conversation_id uuid, actor_id uuid) RETURNS boolean
  LANGUAGE sql STABLE AS $$ SELECT conversation_visible_to(conversation_id, actor_id) $$;`.
  `AGENTS.md` "Architecture rules": "Conversation access goes through
  `conversation_visible_to`" is replaced by rule 7 in section 6. No query changes.
- Guard `conversation_readable_by_is_one_call` (in P0.26's server structure guard): reads the
  latest migration that defines `conversation_readable_by` and fails unless the body is exactly
  one `SELECT conversation_visible_to(...)` with the same two arguments. A later migration
  that redefines it is held to the same check.
- Depends on: P0.26 (the text sits in its section).
- Acceptance: `tests/integration/direct_conversations.rs` new test
  `conversation_visible_to_alone_does_not_check_membership`: for a project-wide channel,
  `conversation_visible_to` is true for a member of another workspace (documents today's
  trap, so K4.4 flips this assertion when it makes the rule whole), and the pairing
  `conversation_visible_to(c, v) AND actor_can_access_project(v, NULL, project)` is false;
  for a member both are true; for a DM non-member both are false. `conversation_readable_by`
  returns the same as `conversation_visible_to` in every one of these cases.

**P0.29 Trigger refusal texts are pinned** (S)
- Changes: a unit test in `domain.rs`: find the last migration (by version) whose text contains
  `FUNCTION validate_agent_run_scope_and_admission`, and assert each key in
  `admission_refusal_reason`'s table (`domain.rs:193-219`) appears in that body. No behaviour
  change.
- Depends on: nothing.
- Acceptance: the test passes; changing a `RAISE` text in a scratch migration fails it.

### Group F: documents, names, dead code

**P0.30 The master plan lives in the repository** (S)
- Changes: a new folder `docs/plans/master/`, copied from the lead's final files with the
  owner's machine names and personal details removed (question 3):

  | Path | From | What it is |
  |---|---|---|
  | `docs/plans/master/README.md` | written in this slice | The index (outline below) |
  | `docs/plans/master/end-state.md` | `END-STATE.md` draft 3 | The target. Vocabulary and principles are fixed |
  | `docs/plans/master/rulings.md` | `LEAD-RULINGS.md` | R1 to R25 and later rulings; binding over plan text |
  | `docs/plans/master/build-order.md` | `BUILD-ORDER.md` | Waves, milestones, the dependency table, decisions that block slices, how to run a slice |
  | `docs/plans/master/plans/p0-steady-ground.md` … `h1-hosting-and-release.md` | `plans/*.md` | The fourteen workstream plans, file names lower-case: `p0-steady-ground`, `k1-accounts-and-access`, `k2-operation-layer`, `k3-things-links-events`, `k4-spaces`, `k5-any-number-of-servers`, `t1-fields-views-tasks`, `a1-runs-requests-hosts`, `a2-connecting-agents`, `m1-modules`, `c1-discussion-and-documents`, `c2-chat-inbox-daily-feel`, `w1-web-app`, `h1-hosting-and-release` |
  | `docs/plans/master/decisions.md` | the owner questions in each plan | One row per question: id, question, recommendation, answer, date, slices it unblocks. Unanswered rows say "assumed: <recommendation>" |
  | `docs/plans/master/interfaces.md` | written in this slice, empty table | The interface change log (definition of done item 14) |
  | `docs/plans/master/guards.md` | section 6.3 of this plan | The guard registry: one name per guard, its rule, who built it, who tightens it |
  | `docs/plans/master/integration-agent-os.md` | the appendices R21 creates | Work that waits on Agent OS; outside the build order |

  `research/` is added only after the lead strips owner-specific detail (question 3).

  The index, `README.md`, has these sections, in this order: (1) what this is, three
  sentences; (2) "Read before any slice": end state, rulings, your workstream's plan, the
  slice; (3) the phase table from the end state; (4) the fourteen workstreams, one line each:
  id, name, plan file, what it owns (the brief's ownership table); (5) milestones and the
  current wave, pointing at `build-order.md`; (6) how a slice id maps to an AWD task title
  (`P0.4: Agents' task comments mention and reference`) and to a pull request title; (7) the
  definition of done (section 7 of this plan, copied); (8) which older plans each workstream
  supersedes (`docs/plans/roadmap.md`, `modules.md`, `collections.md`, `agent-interface.md`,
  `mcp-tools.md`, `agent-tag.md`, `sheets.md`, `whiteboards.md`) and the banner they carry.

  `AGENTS.md`'s first paragraph is replaced by the text in section 6.1. The steward skill's
  "Title `W<n>: <name>`" line becomes "Title `<slice id>: <name>`", and its roadmap checkbox
  line points at the workstream plan's slice table. Later edits to these files happen in the
  slice that changes the design (definition of done item 12), never in a separate docs PR.
- Depends on: the lead's final plans.
- Acceptance: links in the index resolve, and every slice id named in `build-order.md`
  exists in a plan's slice table (a small check in `tools/ci/test_docs_links.py`, run by
  `checks.sh` with the other script tests, over `docs/plans/master/` only).

**P0.31 The front door describes today** (S)
- Changes: `README.md` rewritten: what the product is (two paragraphs), how to run it (the
  existing commands), where the plan is (`docs/plans/master/`), where the rules are
  (`AGENTS.md`). Move to `docs/history/` with a one-line banner each:
  `PROJECT_SOURCE_OF_TRUTH.md` (leave a 3-line stub at the root pointing to the new place),
  `docs/acceptance/`, `docs/task-platform/`, `docs/compatibility/walking-skeleton.md`,
  `docs/plans/m10-work-item-mutation-inventory.md`, `docs/plans/redesign-pass-{1,2}.md`,
  `docs/plans/workos-identity-proposal.md`, `docs/plans/workspace-merge-plan.md`,
  `docs/frontend/overhaul-{product-spec,technical-architecture}.md`,
  `docs/frontend/verification.md`, `docs/operations/abuse-controls.md` (single-process text).
  Add a banner "superseded where `docs/plans/master/` says otherwise" to `docs/plans/roadmap.md`,
  `modules.md`, `collections.md`, `agent-interface.md`, `mcp-tools.md`, `agent-tag.md`,
  `sheets.md`, `whiteboards.md`. Fix the `jobs.rs:11-17` header to say what handlers do
  (dedupe on a row keyed by the cause) and drop the unused helpers (`jobs.rs:223-232`).
  Don't move `docs/agent-runner.md`, `docs/plans/roadmap.json`, `docs/self-hosted-red-hat.md`.
- Depends on: P0.30.
- Acceptance: `cargo check` and the docs link check pass; `grep -r PROJECT_SOURCE_OF_TRUTH`
  finds only `docs/history/` and the stub.

**P0.32 No vendor in tool descriptions and core messages** (S)
- Changes: rewrite the strings listed in 2.4 to say "run", "agent run" or the harness's display
  name from its manifest where it is known (`mcp.rs`, `mcp/runs.rs`, `api.rs:288`, `:593`,
  `mcp/history.rs`, `usage/alerts.rs`, `agent_sessions.rs:75-84` fallback names come from
  manifests); rename the error action `connect_personal_codex_account` to
  `connect_provider_account`, keeping the old value accepted where the web reads it
  (`apps/web` grep) for one release. Config variable renames
  (`CODEX_HOST_CONCURRENCY_LIMIT`, `CODEX_HOME_ROOT`) are left to A1, which moves harness
  config to the host.
- Depends on: P0.26 (the allowance for `vendor_names_core` is lowered in the same PR).
- Acceptance: a unit test in `mcp.rs`: no tool title or description in `tool_catalog()`
  contains an adapter's display name (read from the adapter manifests, so the test itself names
  none); `SERVER_INSTRUCTIONS` likewise.

**P0.33 Personal machines and repositories out of config, seed and docs** (S)
- Changes: `tools/ci/run.sh` reads its hosts from an untracked `~/.config/aw-ci/hosts` (or
  env), with a clear error when missing; `link_previews/guard.rs:16` User-Agent becomes
  `agent-workspace-link-preview/<version>`; `seed::dev_seed` uses a neutral demo repository
  name and no real GitHub id (tests that relied on the old values read them from the seed
  struct); `docs/local-codex-mcp.md` uses placeholders. History and `docs/history/` are left
  alone (owner's choice).
- Depends on: P0.26 (lower `personal_hosts`).
- Acceptance: `tools/ci/test_*.py` still pass (stub the hosts file); the integration suite
  passes with the new seed; the `personal_hosts` allowance drops by the files fixed.

**P0.34 Dead code out of the build** (S)
- Changes: delete the ten `#[cfg(any())]` blocks in `api.rs` (never compiled, so no behaviour
  changes); take `tools/github-spike`, `tools/git-worktree-spike` and `tools/mcp-spike` out of
  `Cargo.toml` `members` (move `github-spike/fixtures/merged-close-rest-null-sha.json` to
  `apps/server/tests/fixtures/` and point `github.rs:5451` at it; the directories stay until the
  owner says otherwise); delete the dead pre-`provider_accounts` branch in `seed.rs:358-383`;
  add the runner SDK tests to `checks.sh` (`sdks/runner-python` with `python3 -m unittest`,
  `sdks/runner-typescript` with its test script) so the public SDKs can't rot.
- Depends on: nothing. Not beside P0.33 (`seed.rs`).
- Acceptance: CI passes; `cargo metadata` lists two workspace members besides the server; the
  SDK steps run in `checks.sh`.

**P0.35 A schema snapshot on demand** (S)
- Changes: `tools/db/schema-snapshot.sh` creates a scratch database, runs migrations and writes
  `pg_dump --schema-only --no-owner` to `target/schema.sql` (not checked in); CI uploads it as
  an artifact; the `add-migration` skill tells agents to read it instead of hunting for the last
  `CREATE OR REPLACE`. Not checked in because every migration pull request would rewrite the
  same file and fourteen workstreams would conflict on it daily.
- Depends on: nothing.
- Acceptance: the script runs in `checks.sh` after the Rust tests and the artifact appears on a
  CI run.

### Group G: privacy and data-loss bugs other planners found (added in review, ruling R10)

**P0.36 Page comment notifications hold ids, not text** (S)
Bug 22. Moved here from C1.1 by ruling R10.
- Changes:
  - `page_docs/comments.rs:449-456`: `detail` becomes
    `{"pageCommentId": id, "threadId": thread_id, "href": …}` (ids only; the href already is).
    `page_docs/suggestions.rs:434-437`: `{"pageSuggestionId": id, "href": …}`.
  - `notifications.rs` `list_sql` (`:1501-1575`): `LEFT JOIN project_pages page ON
    n.subject_kind = 'page' AND page.id = n.subject_id` and `LEFT JOIN project_page_comments
    page_comment ON page_comment.id = (n.detail->>'pageCommentId')::uuid AND
    page_comment.deleted_at IS NULL`, selecting `page.title`, `page.archived_at IS NOT NULL`
    and `left(page_comment.body, {PREVIEW_CHARACTERS})`. The `PAGE_COMMENT` and
    `PAGE_SUGGESTION` kinds get their own arm in the title/preview match (`:1785-1890`):
    "New comment on <live title>" with the live preview, or no preview when the comment is
    gone; an archived page shows "(archived)" and no preview. `VISIBLE` (`:1321`) adds
    `AND (n.subject_kind <> 'page' OR EXISTS (SELECT 1 FROM project_pages p WHERE p.id =
    n.subject_id AND p.project_id = n.project_id))`, so a page moved out of the project stops
    showing.
  - `forget_page_comment` (`notifications.rs:703`) stays (it also retracts occurrences and
    recounts) but no longer has text to strip; its `detail - 'preview'` line goes.
  - Migration `<ts>_notifications_page_text.sql`: the one-statement scrub in section 4.
  - Guard: `notification_detail_text` (3.8), so no new kind copies content into `detail`.
- Depends on: nothing. Not beside P0.5 (both edit `notifications.rs`, different functions;
  merge either first).
- Acceptance: `tests/integration/notifications.rs` new test
  `page_comment_notifications_never_hold_the_words`: Bea comments "the budget is 40k" on
  Ada's page; Ada's notification row's `detail` contains neither the words nor the page
  title (read the row with SQL); the inbox shows the words live; the comment is removed by a
  plain `UPDATE … SET deleted_at = now()` that bypasses `comments::change` (stands for any
  future removal path) and the inbox shows no preview; the page is renamed and the inbox
  shows the new title; the page is archived and the preview is gone. A migration case seeds
  an old row with `preview` and `title` and checks the scrub removes both. C1.1 keeps only
  the agent event for comment mentions and points here.

**P0.37 Account mail never goes through a workspace someone merely joined** (S)
Bug 23. Found in review (from H1's inventory); privacy and account takeover, ruling R10.
- Changes:
  - `workspace.rs:1835-1839`: accepting an invitation no longer sets
    `users.default_organization_id` when the person already has a default or another
    membership (it still sets it for a brand-new person, whose only workspace it is). The
    session's current workspace after accepting comes from the invitation, not from the
    stored default (the web already navigates to the invited workspace; checked in the slice).
  - `mail/mod.rs:305-337` `transport_for_actor`: trust a workspace's SMTP only when the
    person **owns** that workspace or it is their **only** membership. The stored default no
    longer counts, because it can be set by joining; a person who wants a non-owned
    workspace's mail server for their reset links has no way to ask for that, which is the
    safe answer. When nothing qualifies, mail is not sent and the reset page says what it
    says today when no transport exists. H1.17 (server-wide mail transport) later gives every
    server one transport for account mail and replaces this function's body.
  - The doc comment states the rule and the reason.
- Depends on: nothing.
- Acceptance: `tests/integration/bug_sweep.rs` extends
  `reset_mail_goes_only_through_a_workspace_the_person_chose_or_owns` (`:149`) with
  `joining_another_workspace_doesnt_route_reset_mail_through_it`: Ada owns A (no SMTP) and
  accepts an invitation to B (SMTP set, owned by Mallory); Ada's `default_organization_id`
  is unchanged; a reset requested for Ada enqueues no mail through B's transport (assert on
  the test transport's outbox); with A's SMTP set it goes through A. A person whose only
  workspace is B still gets reset mail through B.

**P0.38 No account reveals its other workspaces** (S)
Item 24. Requested by the coordinator after ruling R19 ("Never the list of its other
workspaces. People search returns only accounts that share a workspace with the caller.
Isolation tests cover both."). Re-read: no leak today (see item 24); this pins it before K1
makes accounts global.
- Changes: `tests/integration/workspace_isolation.rs` world gains `shared`, a person who is a
  member of workspace A (one project) and workspace B (two projects named with B's marker),
  and `shared_agent`, an agent account in A and B (two `external_agent` actors today; one
  global account after K1's R19 slice, when the test's setup changes and its assertions do
  not). New tests in `tests/integration/privacy_inside_workspace.rs` (P0.22's file; a
  standalone file if P0.22 hasn't merged):
  - `people_search_shows_only_shared_workspaces`: a member of A only calls `people_search`
    (empty query and "shared"), the Messages overview (`direct.rs` people list), the project
    members list, the agent list and directory, and every profile route that takes an actor
    id (`agent_profiles.rs`, `/api/v1/organizations/{id}/members/{actor}` for A); B's marker,
    B's project names and B's ids appear in none.
  - `people_search_never_finds_strangers`: a person in B only is never returned to a caller in
    A only, by name or handle.
  - The same two through the MCP tool as an agent in A only and as a person's connected app
    (P0.20's `Caller`).
- Depends on: P0.20 (for `Caller`; can land before it with the existing helpers).
- Acceptance: the tests pass today; the coverage list names every route that returns
  accounts (a guard: any GET route whose response type contains `ActorView`, `Person` or
  `ProfileView` is walked or listed with a reason). K1's R19 slice and C2.17 (profiles) must
  keep them green.

## 6. Guard rails: the text for `AGENTS.md`

### 6.1 Replacement for the first paragraph

> # Repository guidance
>
> The plan is in `docs/plans/master/`. Before you start a slice, read, in this order:
> `docs/plans/master/end-state.md` (what the product is becoming; its words are used the same
> way everywhere), `docs/plans/master/rulings.md` (binding where a plan disagrees), your
> workstream's plan in `docs/plans/master/plans/` (at least its goal, target design, guard
> rails and touch points), and your slice's section in it, which is the specification.
> `docs/plans/master/build-order.md` says which wave the slice is in and what must have
> merged first; if a slice it depends on hasn't merged, stop and say so in the task. Work is
> tracked as tasks in the app's AWD project; a task's title and the pull request's title
> start with the slice id (`K1.3: …`). The definition of done is in
> `docs/plans/master/README.md`. If your slice changes something another plan relies on,
> record it in `docs/plans/master/interfaces.md`. Older plans in `docs/plans/` are
> references for how existing pieces were built; where they disagree with
> `docs/plans/master/`, the master plan wins. First-build documents are in `docs/history/`;
> don't follow them.

### 6.2 New section: rules while the foundations are rebuilt

> ## Rules while the foundations are rebuilt
>
> Fourteen workstreams are replacing five patterns that grew by copying. Don't add to them.
> Each rule has a guard test (`apps/server/src/structure_guards.rs`,
> `apps/web/src/structure.test.ts`) with an allowance per file for today's code. The
> allowances only go down: when you remove an offender, lower its number; never raise one. If
> you move code between files, move its allowance in the same pull request. If a rule blocks
> real work, ask in the task instead of raising a number.
>
> 1. **Access is decided in one place.** Don't write a new role check by name: no SQL
>    `role IN ('owner','admin')`, `role = 'admin'` or `role <> 'viewer'`, no new
>    `custom_role_allows(…)`, no new `WorkspaceRole` or `.manages()`, and in the web no
>    `role === 'owner'`. Ask the policy: `policy::authorize` through
>    `work_item_commands::lock_membership` and `authorize_mutation`, `roles::standing` or
>    `roles::project_holding`, and on the web the permissions the server returns. (K1 replaces
>    these with one `access` module; use it once it lands.)
> 2. **No permission depends on the kind of account.** People, agents and apps differ only in
>    how they sign in and how they're told about things. Don't refuse or allow anything because
>    `actor_kind` is or isn't `Human`. If something is for people only, it is a permission
>    marked people-only in `policy.rs`, where roles and `my_permissions` can see it. A kind
>    check that is purely mechanical (signing in, how a wake-up is sent, a display label)
>    carries `// kind-check: mechanical (<why>)` on its line.
> 3. **Runs are read and written through `runs.rs`.** No new SQL naming `agent_runs`,
>    `agent_run_events`, `agent_run_usage` or `agent_approvals` outside the files that have it
>    today; add a function to `runs.rs` and call it. Use `runs::ACTIVE_RUN_STATES`, never the
>    literal list. (A1 turns `runs.rs` into the run service.)
> 4. **One edge per operation.** A write that both the web and agents can do is one domain
>    function, called by the HTTP handler and the MCP tool, which only parse and render
>    (`append_message_command_with`, `work_item_commands`, `collections::records::write` are
>    the models). Specifically: no new `INSERT INTO conversations` (threads go through the
>    thread function in `conversations.rs`), no new `INSERT INTO messages` outside the send
>    pipeline, no new `UPDATE work_items` outside `work_item_commands.rs`, and no new caller of
>    `mcp::call_tool_direct`. New task view, board and filter features are built in the
>    collections view engine, not the task list (until T1 merges them).
> 5. **State lives in PostgreSQL, work runs on the job queue.** No new process-local shared
>    state (`static … Mutex`, `LazyLock<Mutex<HashMap…>>`, a new map in `AppState`) and no new
>    `tokio::spawn` outside `main.rs`; background work is a job in `jobs.rs`. Any server
>    process must be able to serve any request. (K5 moves the existing ones.)
> 6. **Links and events are shared pieces.** Don't create another link table
>    (`*_links`, `*_references`); add a kind to `work_item_links` or `thing_references`
>    until K3's link store lands. A write that other people, agents, webhooks or other tabs
>    should notice enqueues an outbox event in the same transaction.
> 7. **Every conversation read goes through `conversation_visible_to`.** It is the only gate
>    (K1 owns its body; K4 and C1 each own one helper inside it; never `CREATE OR REPLACE` it
>    or a helper you don't own). Until K4 makes it whole, it only says whether the
>    conversation is project-wide or the viewer is on its member list, so pair it with a
>    membership check (`actor_can_access_project` or the membership join). A query that lists
>    conversations or messages (a sidebar, a list, a search, a count) filters every row by
>    the rule, even after a project check: being in a project, including the hidden direct
>    project or the direct scope, doesn't make its DMs and groups yours. No role, admin or
>    staff path reads a DM. A by-id lookup after an access check says so in a
>    `-- visibility: <why>` SQL comment.
> 8. **Nobody's stack is hard-coded.** No provider, model, harness, code host, machine, path or
>    person by name in core code, tool descriptions or copy, outside `provider/`,
>    `ai/providers/` and adapter manifests. Read display names from manifests.
> 9. **Every new write is an operation with an agent tool, a `clientRequestId` if it creates
>    something, and an isolation case** in both `write_isolation.rs` (another workspace) and
>    `privacy_inside_workspace.rs` (a non-member of a private conversation, a narrowed role).
>    Once K2's registry lands, follow the `add-operation` skill: the web route, tool, public
>    endpoint and automation action are generated, never hand-written.
> 10. **Content stays in its record.** Events, notifications, job payloads, delivery bodies
>     and logs carry ids, kinds and field names, never message, comment, page or cell text.
>     Readers fetch the text live, with the reader's access.
> 11. **Every row belongs to a workspace.** A new table has `workspace_id` (and `space_id`
>     where it lives in a space), named so even while their foreign keys still point at
>     `organizations(id)` and `projects(id)` (ruling R1), or it is on the published list of
>     global tables with a reason. It has an export decision once H1's declarations exist.
>     Never insert into `projects` (or `spaces`) outside the spaces module, and never add a
>     hidden kind of project.
> 12. **Modules are registered, not matched.** Never compare a module id by hand
>     (`== "pages"`, `module_off(…, "Pages")`); register a filter. A new module is a manifest,
>     a server folder and a web `module.ts`. A user-facing part checks `shows(part)`.
> 13. **Addresses are built in one place.** Build links to things with the links module
>     (server) and `app/href.ts` (web), never by pasting `"/projects/…"` or `"/w/…"`.
> 14. **Built for cheap hosting** (ruling R24). Database use is pooler-safe: settings only by
>     `SET LOCAL` inside a transaction, no session advisory locks (take a lease), no hand
>     `PREPARE`, no transaction held across a network call. Nothing runs per workspace: no
>     timer, loop, cache warm-up or in-memory map that exists because a workspace exists;
>     schedulers query due rows. Limits are declared in one list and counted in shared
>     storage; plan checks happen only in metering. `LISTEN`/`NOTIFY` is never the only way
>     to learn of an event.
> 15. **Outbound calls go through `outbound.rs`.** No new HTTP client built elsewhere; any
>     address a workspace can set is resolved, checked and pinned there.
> 16. **History is never cut.** No `DELETE FROM` a content or history table outside the
>     owning module's delete operation, workspace deletion and the listed retention jobs.
> 17. **Web data comes from the generated client.** No new hand-written API type or
>     `requestJson` call in a module that has moved to `callOp`; invalidate exactly the things
>     an event names, never a whole project.
>
> Shared test fixtures are in `apps/server/tests/integration/support/`: a test database from
> the run's template, one `AppState` builder, a world with people, agents and connected apps,
> two workspaces, and a `Caller` that runs one operation as each kind of account. Use them
> instead of copying another file's setup.

Rule-to-problem map: problem 1 (agents second-class) → rules 2, 3; problem 2 (modules don't
build the app; links in ten stores) → rules 6, 12, 13; problem 3 (single-process state) →
rules 5, 14; problem 4 (access decided seven ways) → rules 1, 2, 7, 11; problem 5 (built
twice) → rules 4, 9, 17. Rule 8 is principle 6 of the end state, rule 10 principle 10 and the
event envelope (R8), rules 14 to 16 are R24 and H1's hosting rules.

Each rule is short on purpose: `AGENTS.md` is read by every session. The owning workstream's
guard says the detail; `docs/plans/master/guards.md` lists them all.

### 6.3 The guard registry: one name per guard

Several plans proposed the same guard under different names. Each guard has one id, one
file and one allowance format (3.8: a per-file count that never fails when it drops, so
branches merge in either order). P0 builds the first version and owns the rule text; the
workstream in the "Tightens" column owns making it stricter later (new patterns, a reason and
a removing slice per allowance entry, folding a ratchet to zero). The other plans' names in
the last column are not separate guards: those slices point here and extend this one.

| Guard id | Rule | Built by | Tightens | Was also proposed as |
|---|---|---|---|---|
| `raw_role_checks`, `custom_role_allows`, `workspace_role_enum` | 1 | P0.26 | K1.1 (adds `access_bypasses`: `agent_limit_hides(`, `agent_limit_projects_allowing(`, `Subject::new(`, `policy::authorize(`; reason and removing slice per entry) | K1 guard 1 (`access/guard.rs`, `access/allowlist.rs`) |
| `account_kind_checks` | 2 | P0.26 | K1.1, then each area slice of K1 | K1 guard 1 kinds; A1 G5; A2 "no rule by account kind"; C1 guard 4; K2 guard 5. A new folder (`runs/`, `delivery/`, `discussion/`, `ops`) has no allowance entry, so it is held at zero automatically |
| `web_role_name_gates`, `web_account_kind_gates` | 1, 2 | P0.27 (the second added in review: `kind === 'external_agent'`, `kind === 'human'` in a condition) | K1.26, W1.37 | K1's `apps/web/src/access.guard.test.ts` |
| `agent_runs_sql`, `active_run_states_literal` | 3 | P0.26 | A1.2 (adds the new run tables and `LEGACY_RUN_SQL` reasons) | A1 G1, G4 |
| `requests_sql`, `hosts_sql` | 3 | A1.7, A1.11 | A1 | A1 G2, G3 |
| `conversation_inserts`, `message_inserts`, `task_core_updates`, `direct_tool_calls` | 4 | P0.26 | C1.2 (threads only in `conversations/threads.rs`), K2.6 (`call_tool_direct`) | C1 guards 1, 2; K2 guard 3 |
| `task_view_code` | 4 | T1.1 | T1.39 | T1 guard 1 |
| `process_state`, `background_spawns` | 5 | P0.26 | K5.1 (adds `broadcast::channel`, `tokio::fs`, reason and bound per entry) | K5 guard 2 (`process_local.rs`) |
| `session_advisory_locks`, `pooler_safe_sql` | 14 | K5.2 (phase 1, ruling R24) | K5 | K5 guard 3; R24 rule 3 |
| `nothing_per_workspace` (10,000 idle workspaces) | 14 | H1 (its R24 slice) | H1 | R24 rule 6 |
| `link_tables`, `comment_tables` | 6 | P0.26 | K3.21, C1.3 | K3 guard 4; C1 guard 2 |
| `conversation_listings` | 7 | P0.0 | K4.3 (adds the direct scope: no membership row may reference it), then K1.20 | C1 guard 3; C2 "listing lint"; K4 G2 |
| `vendor_names_core`, `web_vendor_names`, `personal_hosts`, tool-description test | 8 | P0.26, P0.27, P0.32 (P0.32 rewrites the vendor names; K2.1 is soft on it) | K2.1 (adds kind-rule wording: "people only", "agents can't") | A1 G6; A2 "no vendor names"; K2.1 `descriptions_name_no_vendor_or_kind_rule` |
| create guard (`clientRequestId` on creates) | 9 | P0.16 | K2.6 (conventions over the registry) | K2 guard 4 |
| in-workspace privacy net | 9 | P0.22 (+ P0.38's account cases) | each slice adds its cases | C2 "private-conversation net" |
| `notification_detail_text`, `event_payload_text` | 10 | P0.36, K3.21 | K3, A2 | K3 guard 3; A2 "thin payloads" |
| `tenant_tables`, `new_table_names` | 11 | H1.3, K4.3 | H1, K4 | H1 6.2; K4 G4 |
| `one_space_creator`, `space_kind_literals` | 11 | K4.3 | K4.17 | K4 G1 |
| `module_ids_by_hand` | 12 | M1.1 | M1.2 onward | M1 guard 1 |
| `hand_built_links` | 13 | K4.3 (server), W1.14c (web) | K4.18, W1.14c | K4 G5; W1 guard 3 |
| `outbound_clients` | 15 | H1.2 | H1 | H1.2's builder guard |
| `history_never_cut` | 16 | H1 (phase A) | H1 | H1 6.2 |
| `handwritten_api`, `whole_project_refetch` | 17 | W1.3, W1.4 | W1 | W1 guards 1, 2 |

The old `conversation_visible_to_raw` guard is withdrawn. In its place is
`conversation_readable_by_is_one_call` (P0.28, ruling R32), which keeps the wrapper to one
call. The ids `conversation_listings` (P0.0) and the P0.26 ids in this table are stable: K4.3's
G2 and K1.1's ratchet build on them by these names, so no slice renames them. Two plans can't create a guard with an id already in this table; a slice that adds a
new guard adds its row to `docs/plans/master/guards.md` in the same pull request.

Other guard rails P0 adds beyond the rules: no flaky passes (P0.3), web changes run the Rust
tests that read them (P0.3), connected-client contracts (P0.17), trigger texts (P0.29), every
tool refusal matches its schema and every outcome has an HTTP status (P0.9), id-routed families
are gated or exempt with a reason (P0.7).

## 7. Definition of done for a slice

Every workstream's pull request meets all of these. The master plan's `README.md` carries this
list; the steward skill links to it.

1. **One slice, one pull request**, titled `<slice id>: <name>`, matching its plan's
   description. Anything the slice found but didn't do is a new task, named in the PR.
2. **Checks green**: `tools/ci/run.sh` (or `checks.sh` in a cloud session) passes on the
   working tree before push, and CI passes with no flaky result.
3. **The acceptance checks in the plan exist as tests and pass.** Bugs that would be silent and
   costly (permissions, private conversations, claims and leases, reply rules, credentials,
   lost events) have a test that fails without the change. No UI-detail matrices, no
   acceptance-evidence documents.
4. **New writes**: one domain function; an MCP tool in the same PR; `clientRequestId` on
   creates and `expectedVersion` on changes; a case in both isolation nets; an outbox event if
   others should see it; a permission from the catalogue, never a kind check.
5. **New reads**: through the access path; a case in both read nets.
6. **No guard allowance raised.** Lowered where the PR removed offenders.
7. **Database**: a timestamp-named migration that merges in any order; expand, copy, switch,
   contract; any backfill resumable and said so in the PR; nothing assumes a pending migration
   already ran in production.
8. **Web**: new parts registered in the layout registry and their module's manifest; new pages
   lazy; budgets hold.
9. **Compatibility**: `compat_clients.rs` still passes, or the PR is the retirement slice that
   `docs/compatibility/connected-clients.md` names, with the owner's sign-off.
10. **No names**: no provider, model, harness, machine, path or person in core code or copy.
11. **The PR description** lists deploy steps (migrations, settings, anything to set after
    deploying), the old path it leaves and the slice that deletes it, and what it measured if
    the slice was about speed.
12. **Docs**: the workstream's plan is updated if the slice changed its design; nothing is added
    to `docs/history/`.

Added in review, so the list covers what the other thirteen plans assume:

13. **No permission by account kind.** The PR adds no check that allows or refuses by
    `actor_kind`/account kind (rule 2). A mechanical difference carries the
    `// kind-check: mechanical (<why>)` marker. Where the slice touches a hard-coded agent
    rule, it says which role setting replaces it.
14. **Interfaces other plans rely on.** If the slice changes a name, shape or behaviour that
    another plan's "Touch points" or "assumes" lists (a function, table, column, event,
    operation, error, wire format), the PR (a) updates the owning plan's text, (b) adds a row
    to `docs/plans/master/interfaces.md` (date, slice, interface, old → new, plans that rely
    on it), and (c) says so under "Interfaces changed" in the description, naming the
    relying slices. A rename keeps the old name working until the relying slices move.
15. **Names (ruling R1).** New tables and columns are named `workspace_id`, `space_id`,
    `account_id`; their foreign keys point at `organizations(id)` and `projects(id)` until
    K4's flip. Existing columns are renamed only by K4's plan.
16. **Export** (once H1.8 has landed): every new table has an export declaration in its
    module (exported, or `notExported` with a reason), and every new tenant table carries its
    workspace column or is on the global list (H1.3).
17. **Triggers (ruling R16).** A slice that adds a trigger lists it in its plan's per-table
    trigger table (table, trigger, what it writes, the slice that removes it) and checks no
    other plan's trigger on that table writes the same other table.
18. **Things and events** (once K3.10 and K3.2 have landed): a new kind of thing registers in
    the thing registry with every shared service or a reason (K3 guard 1); a new write emits
    an event from the catalogue in the same transaction.
19. **Layout parts.** A new user-facing part (sidebar row, tab, task field or section,
    composer extra) is registered in `experience/registry.json` with the presets that show
    it, the page checks `shows(part)`, and it hides when its module is off. A slice that adds
    visible words checks the chat-only shape (ruling R22): no task, project, agent or run
    word appears when those modules are off.
20. **Guards.** A new guard takes an id from, and adds a row to, `docs/plans/master/guards.md`;
    it never duplicates an existing id under another name.
21. **Bugs are P0's (ruling R10).** A slice that would fix a bug already on P0's list
    depends on that P0 slice instead and does only what goes beyond it.
22. **Hosting rules (ruling R24).** The slice passes the pooler-safe, process-role and
    nothing-per-workspace guards once they exist, and its new state is in PostgreSQL or file
    storage.

## 8. Risks and open questions

**Risks**

- *The sidebar leak is live today* (P0.0): any workspace member who has opened Messages can
  read the thread names of every DM and group in the workspace through one URL. It has been
  confirmed only by reading. Notice: the failing test. Fallback: none needed beyond landing
  P0.0 first; the owner may want to know whether thread names in DMs have held anything
  sensitive, since request logs would show who called the route with the direct project's id.
  The `conversation_listings` guard may also turn up other listings with the same gap; each
  becomes its own S slice ahead of Group B.
- *Turning retries off turns real flakes red for everyone at once* (P0.3). Notice: several
  unrelated PRs red on the same test. Fallback: a named override with `retries = 1` for that
  test only, with a task to fix it; never for race, claim, lease, bootstrap or credential
  tests.
- *The privacy net finds many gaps at once* (P0.22). That is the point, but it can stall.
  Fallback: `KNOWN_GAPS` keeps the net green while each gap becomes a task; K1 takes the
  access ones.
- *The template database doesn't work on a host* (P0.21): missing `CREATEDB`, or a
  PostgreSQL that is slow to copy. Notice: the fallback log line. Fallback: the schema path
  stays, so nothing breaks; only the speed-up is lost there.
- *The stricter agent-management rule removes access someone relies on* (P0.6): a project
  admin who is only a workspace member could connect as an agent through OAuth and no longer
  can. Notice: the test names the case; the PR description says it. Fallback: a workspace
  owner promotes them; K1's workspace-scoped `agents.manage` makes it a role setting.
- *Rate-limit change locks out a real person while under attack* (P0.13), if they last signed
  in over 7 days ago on that process (or after a restart, since the recent set is in memory).
  Fallback: per-address limits when the header is set, which the live server should set; K5
  makes it shared and durable.
- *Guards produce churn*: rule 2's 215 kind checks are mostly mechanical; marking them is
  work. The allowance means no one has to mark anything to keep CI green; K1 does the
  classification as it replaces them.
- *Outbox retries amplify a deterministic fan-out bug* into ten attempts and then a failed
  event (P0.1), where today it is one silent loss. Notice: `failed_at` rows and the operator
  summary. That is better than today, but the operator status page should show failed outbox
  events (it reads outbox state already, `server_status.rs`; confirm in the slice).
- *Allowance files become merge hot spots.* One entry per line in sorted order; conflicts are
  one-line and resolved by taking the lower number.

**Questions only the owner can answer** (with the recommendation)

1. Is a role's `modules_off` a permission (people with it must not reach that module's pages
   over the API) or only a view? *Recommend: a permission, as MCP already treats it; P0.7 then
   applies role narrowing in the HTTP gate too.*
2. Is any database still running from before migration 0010 (an M0-M2 install)? *Recommend: if
   not, P0.25 retires the five upgrade replays listed there and keeps everything that runs live
   code.*
3. May the research files and the context file go into `docs/plans/master/research/` as they
   are, or only after removing the owner's own setup and names? *Recommend: only the end state
   and the plans go in now; research goes in after the lead strips owner-specific detail, since
   the repository is headed for public release.*
4. Should `tools/ci/run.sh` (routing to personal machines) stay in this repository? *Recommend:
   keep it, with hosts read from an untracked file (P0.33), so `checks.sh` stays the portable
   entry point.*
5. Until T1 merges the task and collection view engines, may new task view, board and filter
   features be built only on the collections side (rule 4's last sentence)? *Recommend: yes;
   the last week built the same board grouping twice.*

## 9. Touch points

**What P0 provides**

| To | What |
|---|---|
| Every workstream | `tests/integration/support/` (P0.20): `db`, `app`, `WorldBuilder`, `TwoWorkspaces`, `Person`/`Agent`/`ConnectedApp` and the `Caller` trait, `drain_outbox`, `assert_absent`; fast template databases (P0.21); the in-workspace privacy net to add cases to (P0.22); the shared scanner (P0.24); the rules and guards with allowances (P0.26, P0.27); the definition of done (section 7); timings in JUnit (P0.3). |
| K1 | Conversation listings filtered by the visibility rule, and the `conversation_listings` allowance as its list of unfiltered queries to review (P0.0); `external_agents::may_manage` as the one agent-management rule to absorb (K1.4 goes beyond it); the base-role-aware helpers (P0.8); the `raw_role_checks`, `custom_role_allows`, `workspace_role_enum`, `account_kind_checks`, `web_role_name_gates` and `web_account_kind_gates` allowances as K1's burn-down list; the privacy net as the safety net for the access rewrite (K1 should not start moving checks before P0.22 runs). |
| K2 | Contract tests for MCP revisions, agent tool names and arguments, and URLs (P0.17), which K2's new revision and envelope must keep green or retire explicitly; P0.9's refusal and outcome-to-status tests; the `direct_tool_calls` freeze (K2 replaces `call_tool_direct` in `bug_reports.rs` and `automations.rs`); `clientRequestId` on today's creates and the create guard (P0.15, P0.16). |
| K3 | Outbox side effects retried (P0.1), so the event log's successor starts from at-least-once fan-out; the `link_tables` guard; the activity payloads unified for GitHub and board moves (P0.11, P0.12). |
| K5 | Loops that survive errors (P0.2); rate limits that charge only failures globally, behind the same `RateLimiters` API, ready for a shared store (P0.13); the `process_state` and `background_spawns` allowances as its list. |
| T1 | All status changes through `update_locked_work_item` (P0.11, P0.12), so the move to records has one write path to port; agent comments with mention and reference projections (P0.4). |
| A1 | The `agent_runs_sql` and `active_run_states_literal` allowances as the run-service burn-down list; vendor names out of tool text (P0.32), leaving config renames to A1. |
| A2 | The wake-format contract and keep-format save (P0.17, P0.18); wake-ups posted outside the transaction (P0.10). |
| M1 | Id-routed gate families and the exemption list (P0.7). |
| C1 | `mark_seen` as the single read-state function (P0.5); the thread-insert freeze. |
| C2 | Notifications cleared by reads through apps (P0.5); channel lists already filtered by the visibility rule, so private channels need no new filter there (P0.0). |
| W1 | Web guards (P0.27). |
| H1 | Personal hosts and repositories out of config, seed and docs (P0.33), and the `personal_hosts` guard; the release-time scrub of names stays with H1. |
| Lead | `docs/plans/master/` with an index and the `AGENTS.md` pointer (P0.30). |

**What P0 needs**

- From the lead: final plan files for P0.30; the decision on question 3.
- From K1: that its first slice keeps `may_manage`'s behaviour until K1 deliberately changes
  the rule (assumed).
- From K2: that its envelope unification keeps the `outcome` field and `ok`/`value` for the
  tools in the compat fixture during the deprecation window (assumed; the end state says old
  revisions stay served).
- From A2: that `inbox.available` delivery is kept as an adapter until
  `docs/compatibility/connected-clients.md`'s retirement condition is met (assumed; matches
  the end state, section 4).
- Nothing else blocks P0; it can start at once.

## 10. Changes to the end state

1. Section 4, "What we ask of the harness", ends with "Workspace keeps the old wake format and
   old MCP revisions served until it has." Proposed addition: "These are pinned by contract
   tests (`compat_clients.rs`); retiring one is its own slice, named in
   `docs/compatibility/connected-clients.md`, with the owner's sign-off." Without it, any
   workstream can drop a revision by upgrading a library.
2. Section 6, Tenancy row ("checked by isolation tests and by the database's own row rules"):
   add "and by a privacy net inside each workspace (a non-member of a private conversation, a
   narrowed role) over every read and write". Cross-workspace isolation alone doesn't cover
   the leaks people would actually see.
3. Principle 1 says "No code path asks 'is this an agent?' to decide a permission. The only
   differences are mechanical." Proposed: keep, and add "Mechanical kind checks are marked in
   the code so they can be told apart from permission checks." This makes the principle
   checkable by a guard rather than by reading 215 lines.

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| P0.0 | The sidebar shows only conversations the caller can see | S | none (lands first) | all |
| P0.1 | Events keep their side effects | S | none | all |
| P0.2 | A database blip doesn't stop the server | S | none | all |
| P0.3 | CI tells the truth (flaky fails; web files run Rust tests) | S | none | all |
| P0.4 | Agents' task comments mention and reference | S | none | all but P0.12, P0.15, P0.16 |
| P0.5 | Reading through an app clears notifications | S | none | all |
| P0.6 | One rule for managing and acting as an agent | S | none | all but P0.14 |
| P0.7 | Module gate covers pages and boards named by id | S | none (owner Q1 for role narrowing) | all |
| P0.8 | SQL permission helpers respect the base role | S | none | all |
| P0.9 | Refusals match tool schemas; public API maps every outcome | S | none | all |
| P0.10 | Wake-ups posted outside the transaction | S | none | all but P0.18 |
| P0.11 | GitHub-driven task moves through the task command | S | none | all but P0.12 |
| P0.12 | Board drag, mark-blocked, ready-for-review through the task command | M | P0.4, P0.11 | all but P0.4, P0.11 |
| P0.13 | Valid callers never share a global budget; address header setting | S | none | all |
| P0.14 | OAuth registration counts only unclaimed clients | S | P0.13 | all but P0.6 |
| P0.15 | Safe retries on creates, part 1 | S | none | all but P0.4 |
| P0.16 | Safe retries on creates, part 2; honest instructions; create guard | S | none | all but P0.4 |
| P0.17 | Contract tests for connected clients | S | none | all |
| P0.18 | Saving a webhook keeps its wake format | S | none | all but P0.10 |
| P0.19 | Race tests run on today's schema; naming guard file | S | none | all |
| P0.20 | Shared integration fixtures | M | none | all |
| P0.21 | Template database per run; fast test databases | M | P0.20 | all but P0.20 |
| P0.22 | Privacy net inside a workspace | M | P0.20, P0.24 | all |
| P0.23 | Read-only tools change nothing | S | P0.20 | all |
| P0.24 | One source scanner for every guard | S | none | all |
| P0.25 | Retire upgrade replays for data that no longer exists | S | P0.19, owner Q2 | all |
| P0.26 | Rules in AGENTS.md and server structure guards | M | P0.24 | all |
| P0.27 | Web structure guards | S | none | all |
| P0.28 | Conversation access: the AGENTS.md wording, the thin `conversation_readable_by` wrapper (R32) and the pairing test | S | P0.26 | all |
| P0.29 | Trigger refusal texts pinned | S | none | all |
| P0.30 | The master plan lives in the repository | S | lead's final plans | all |
| P0.31 | The front door describes today; history moved | S | P0.30 | all |
| P0.32 | No vendor in tool descriptions and core messages | S | P0.26 | all |
| P0.33 | Personal machines and repositories out of config, seed and docs | S | P0.26 | all but P0.34 |
| P0.34 | Dead code out of the build; SDK tests in CI | S | none | all but P0.33 |
| P0.35 | Schema snapshot on demand | S | none | all |
| P0.36 | Page comment notifications hold ids, not text (the 140-character preview, `page_docs/comments.rs:451`; C1 and C2's "P0's new preview slice after P0.35") | S | none | all but P0.5 |
| P0.37 | Account mail never goes through a workspace someone merely joined | S | none | all |
| P0.38 | No account reveals its other workspaces | S | P0.20 (soft) | all |

## Review changes

Made by the steady-ground and build-order reviewer on 2026-11-14, after reading all fourteen
plans, `LEAD-RULINGS.md` (R1 to R25) and `HANDBACKS.md`. Only this file was edited; the
pointers below are for the reviewers of the other plans.

**1. Bugs fixed once (ruling R10).** P0 already had a slice for every bug the ruling names.
These slices in other plans should say "done by P0.n; here only what goes beyond it":

| Bug | P0 slice | Slices that should point at it | What stays in that slice |
|---|---|---|---|
| Agent task-comment mentions and references | P0.4 | C1.1, C2.5, K2.13, A2.4 | C1.1: the `comment.mentioned` agent event; C2.5: DM and thread notification kinds; K2.13: the op wrap; A2.4: waking agents |
| Lost event when notifications or routing fail | P0.1 | K3.1 (marked done), K5 3.4 text | nothing in K3.1 |
| Reading through a tool leaves notifications unread | P0.5 (`read_state::mark_seen`) | C2.2 (done), K2.7 | K2.7: the `conversation_mark_read` op on top |
| Module gate skipped for pages and boards by id | P0.7 | K1.11, M1.2 | K1.11: role narrowing through `access`; M1.2: the lookup moves to the registry, P0.7's tests kept |
| Scheduler and GitHub loops stop on a database error | P0.2 | K5.2 | leases |
| Four rules for managing an agent | P0.6 | K1.4 | `account_managers`, `agents.manage_any`, R19's owner rule |
| SQL role helpers ignore the base role | P0.8 | K1.3 | the catalogue table and `role_allows(role, role_id, key)`; P0.8's tests kept |
| GitHub moves, board drag, mark-blocked, ready-for-review bypass the task command | P0.11, P0.12 | K2.13, K3.7, A1.19, T1.11 (depends on P0.11, P0.12; reorder stays versionless) | K3.7: only `task_reorder` still needs an event; A1.19: the code-host seam, not the move |
| Saving a webhook switches the wake format | P0.18 | A2.3 (`external_agents.rs:690-691`) | Standard Webhooks |
| Wake-ups posted inside a transaction | P0.10 | A2.3, A2 3.5 text | the new pusher |
| Global sign-in and MCP buckets; trusted address header; OAuth registration cap | P0.13, P0.14 | H1.1 | per-workspace ceilings, declared scopes |
| Vendor names in tool descriptions and run text | P0.32 | K2.1 (second half), A1.6 (wording part) | K2.1: protocol matrix and kind-rule words; A1.6: run reads and config aliases |
| Personal machines in `run.sh`, seed and docs | P0.33 | H1 (phase G text) | the release-time history scrub |
| SDK tests in CI; dead `cfg(any())` blocks | P0.34 | A1 G12, A1 touch points | nothing |
| Old MCP revisions and wake format pinned | P0.17 | K2.1 (extends the same file with the new revision), A2.7 (extends the fixtures) | new revisions and formats |
| Sidebar and channel listing gaps | P0.0 | C2.1 | C2.1: refusing the sidebar for `kind = 'direct'` and the search assertions only; uses P0.28's `conversation_readable_by` wrapper, adds no function |
| Page comment notification keeps text | P0.36 (new) | C1.1 | the agent event only |

**2. New P0 slices (Group G).** P0.36 page comment and suggestion notifications hold ids, not
text (re-read: the delete path already strips the preview, the leak is every other path, the
page title, and archive; planned with a test that removes a comment without the module's own
function). P0.37 account mail never goes through a workspace someone merely joined (found in
review: accepting an invitation sets the default workspace, `workspace.rs:1835`, and reset
mail trusts the default's SMTP, `mail/mod.rs:305-337`; account takeover for people without
two-factor). P0.38 no account reveals its other workspaces (requested after R19; no leak today
by reading, pinned before K1 makes accounts global). Other finds in other plans' "where the
code is today" and "risks", not moved to P0 because they are not live today or are owned
whole by a slice already: SSRF guards pinning only the first address (owner corrected to
H1.2, not A2), agent webhook secrets in plaintext without a keyring (K1.22), startup recovery
taking another process's runs and sign-out not reaching another process's sockets (K5.3,
K5.5; only with two processes), delegation limits applied by hand at 18 sites with no leak
found (K1), thread creation over MCP writing no audit row (C1.2).

**3. `conversation_readable_by` is a one-call wrapper (ruling R32, which replaces this
review's first version of the change).** P0.28 adds it with a guard that its body is one call
to `conversation_visible_to`; rule 7 now says `conversation_visible_to` is the only gate (R3) and must be paired with a
membership check until K4.4. The `conversation_visible_to_raw` guard is withdrawn. P0.0's
guard also covers `messages` and is the one guard C1, C2 and K4 proposed under three names.

**4. Rules and guards.** `AGENTS.md` rules 10 to 17 added (content stays in its record; every
row belongs to a workspace; modules registered; addresses built in one place; built for cheap
hosting (R24); outbound calls; history never cut; web data from the generated client). Rule 7
rewritten, rule 9 points at `add-operation`. Section 6.3 is now a guard registry: one id per
guard, who builds it and who tightens it, and every other plan's name for the same guard.
The biggest overlap was K1.1's guard against P0.26's: one guard (P0's ids and file, P0's
"never fail when the count drops" rule, which K1's "fail when below" would break when two
branches each remove an offender); K1.1 becomes the tightening slice. New ids:
`comment_tables`, `notification_detail_text`, `web_account_kind_gates`; `link_tables` also
matches `*_relations`.

**5. Definition of done** items 13 to 22: no kind checks, interface changes recorded,
R1 names, export declarations, trigger tables (R16), things and events, layout parts and the
chat-only words (R22), guards, R10, R24.

**6. Where the plan lives (P0.30)** is now concrete: the folder, every file and its source,
the index's sections, and the `AGENTS.md` paragraph (6.1) telling every session to read the
end state, the rulings and its plan before starting a slice.

**7. Smaller.** P0.5's function is `read_state::mark_seen` (C2's and K2's name). P0.21's
session advisory lock is named as the one test-only exemption to R24 rule 3. The SSRF owner
in 2.1 is H1.2. Slice table rows for P0.36 to P0.38.

**8. Numbering confirmed (content and web review, P0 request 1).** The slice for the
140-character page comment preview (`page_docs/comments.rs:451`) is **P0.36**. C1 and C2,
which call it "P0's new preview slice after P0.35", should name P0.36. P0.37 (account mail
through a joined workspace) and P0.38 (no account reveals its other workspaces, R19) follow it.
Rulings R26 to R31 change nothing in P0: R31's "one personal DM per pair" is K4.24 and C2.39's,
and P0.38's people-search cases already match R19.

**9. Kernel review requests (rulings R32 to R37).** P0.28 adds `conversation_readable_by` as a
one-call wrapper of `conversation_visible_to`, guarded by `conversation_readable_by_is_one_call`
(R32). The guard ids of P0.0 (`conversation_listings`) and P0.26 are stable names that K4.3's G2
and K1.1's ratchet extend; renaming one needs a row change in `guards.md` and both plans'
agreement. P0.32 rewrites the vendor names in tool descriptions and core messages; K2.1
builds on that and is soft on it. R36: P0.21's test-only session lock and key rotation's lock
(`secrets.rs:1105-1110`, direct connection only) are the two named exemptions to
`session_advisory_locks`. R33, R34, R35 and R37 need nothing from P0.

**10. Agents and hosting review requests (rulings R38 to R42).** Already in the R10 pointer
table: P0.18 fixes the bug A2.3 also names (`external_agents.rs:690-691`), so A2.3's part is
"done by P0.18" when P0 lands first; P0.32 does the tool descriptions and A1.6 the rest of the
vendor wording; P0.11 comes before A1.19 (A1.19 depends on it). R42's ratchet on unscoped
database calls is H1's and is registered in 6.3 under `pooler_safe_sql`'s neighbour row
`tenant_tables`; P0 adds nothing for it.
