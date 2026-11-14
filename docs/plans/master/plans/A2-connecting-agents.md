# A2. Connecting agents: the detailed plan

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner for workstream A2, 2026-11-14. Read-only research against `/home/user/agent-workspace`
at fe95a74, `/home/user/agentos` and `/home/user/mischa-dev/harness`. Paths without a prefix
are under `apps/server/src/` of Agent Workspace; `migrations/` means
`apps/server/migrations/`. "Inferred" marks anything not read directly.

## 1. Goal

When A2 is done, any agent, whoever runs it, is told everything that concerns it through one
ordered event stream it can receive by webhook, by a stream it holds open, or by polling, all
from the same cursor. It is told why (a reply is required, it was assigned, it is following
the thing, it is only visible), and the access check happens again at delivery. Agents behave
like teammates: they follow things, can be switched to ambient mode in a conversation, mention
each other under loop limits a workspace sets, schedule their own follow-ups, show a status
line, can be paused, and spend inside budgets that work the same way for people. Adding an
agent is one vendor-neutral flow with four paths (pick, connect, start one on a linked host,
add one you already have from another workspace), where a host's own steps appear inside the
app and Workspace owns only the role and spaces (R21); an agent is one global account that
may be a member of several workspaces (R19). Always-on runtimes (Hermes, OpenClaw) and Agent
OS get ready adapters and a generic bridged connector instead of an owner-specific bridge;
nothing in A2's done waits on Agent OS changing.

## 2. Where the code is today

### 2.1 How an outside agent is created, given a credential, woken and answers

- **Created per project.** `external_agents::create_connection` (`external_agents.rs:224-350`)
  takes a connection name, display name, handle, role, scopes and an optional webhook URL, and
  calls `insert_agent` (`external_agents.rs:373-470`), which inserts an `actors` row with kind
  `external_agent`, a workspace membership as `member`, a **project** membership, an
  `external_agent_connections` row (`migrations/0033_m9_external_agent_connections.sql:3-51`)
  and the first credential (`insert_credential`, `external_agents.rs:1444`). The credential
  lives 365 days (`external_agents.rs:28`). A host command does the same without a browser
  (`main.rs:213-275`, `external_agents::provision`, `external_agents.rs:524-587`).
- **Who may.** `authorize_management` (`external_agents.rs:1282-1319`) requires the caller to
  be a person and a workspace owner or admin. The database trigger
  `validate_external_agent_connection` (`migrations/0033_m9_external_agent_connections.sql:60-101`)
  refuses any agent connection whose role is not `member` or `viewer`, and any connection whose
  creator or updater is not a person. Both are permission differences by account kind.
- **Credential handed over.** Once, in the response, or through a one-time setup link
  (`agent_setup_links.rs:58-115`, 30 minutes, token held in the secrets store). The link page
  writes steps for three hard-coded apps (`agent_setup_links.rs:36`, `:275-310`), and the web
  has matching presets (`apps/web/src/external-agents/runnerPresets.ts:15-20`). The generic
  path is `{origin}/connect.md` (`connect.md`) and `{origin}/agent-runner.md`
  (`replies.rs:65-78`, serving `docs/agent-runner.md`).
- **Access per call.** `agent_connection_can_access(connection, project)`
  (`migrations/0040_member_accounts.sql:72-88`) lets one connection act in every project of
  the workspace the agent belongs to, anchored to its home project.
- **Woken.** Reply rules (`replies::ask_for_replies`, `replies.rs:97-158`) create
  `reply_requests` rows and call `wake` (`replies.rs:516-566`), which writes an
  `external_agent_events` row for pollers and, when the connection has a webhook, an
  `agent_webhook_deliveries` row (`migrations/0038_reply_requests.sql:73-87`). The loop in
  `replies::run` (`replies.rs:1244-1262`) posts them (`deliver_webhooks`,
  `replies.rs:1315-1436`) **inside an open transaction** with an 8-second timeout and at most
  8 attempts with backoff capped at 5 minutes (`replies.rs:49-50`, `:1405`). Bodies are thin
  (`wakeup_body`, `replies.rs:1443-1468`), signed `sha256=` HMAC over `"{timestamp}.{body}"`
  in `x-workspace-*` headers (`replies.rs:1482-1489`, `:1733-1745`). Connections made before
  typed wake-ups get the legacy `inbox.available` body (`wakeup_format`,
  `migrations/20261016150100_agents_in_conversations.sql:56-62`). Saving the webhook again on
  the Agents page silently switches a connection to typed (`external_agents.rs:690-691`).
- **Answers.** Through MCP: `inbox_list` (`mcp/replies.rs:335`), `reply_claim`,
  `reply_progress`, `reply_send` (interim or final), `reply_pass`, `reply_release`, and
  `session_bind` for the "Open session" link (`docs/agent-runner.md:87-148`,
  `agent_sessions.rs:1-26`). Claims are leases with generation fencing (`replies.rs:600-670`).
  Stop cancels and sends `reply.cancelled` at once (`replies.rs:1146-1241`).

### 2.2 Three separate event systems

| System | Table and code | Who it serves | Transport | Shape |
| --- | --- | --- | --- | --- |
| Reply wake-ups | `agent_webhook_deliveries`; `replies.rs:516-566`, `:1315-1498` | Agent connections | Signed webhook | `reply.requested`, `reply.followup`, `reply.cancelled`, `ping`, legacy `inbox.available` |
| Agent event inbox | `external_agent_events`, per-connection sequence (`migrations/0034_m9_external_agent_events.sql`); `external_events.rs:37-85`; poll in `mcp/collaboration.rs:154-300` | Agent connections | MCP poll, long-poll up to 30 s (`mcp/collaboration.rs:167`), signed cursor bound to connection, project and **credential** (`external_events.rs:248-307`) | `kind`, `summary`, `resource_kind`, `resource_id`, path. Re-checked by `external_event_visible_to` (`migrations/20261113100000_person_questions.sql:88-99`), whose `ELSE true` lets every other kind through on project access alone |
| Outgoing webhooks (X4) | `webhook_subscriptions`, `webhook_deliveries`; `webhooks.rs`, fed by `events::route` (`events.rs:151-258`) from the outbox | **People only** (`webhooks.rs:304-313`) and apps acting as them | Job queue, signed like wake-ups, 20 failures turn it off (`webhooks.rs:53`) | 11 types (`events.rs:34-52`), re-checked against the owner's access at send (`webhooks.rs:998-1068`) |

All three sit on the outbox (`outbox_events`, `migrations/0005_m2_conversations.sql:519`,
`event_cursor` identity), which K3 replaces with the one event log.

### 2.3 What wakes an agent today and what doesn't

| Happening | Push (webhook) | Poll (`external_event_poll`) | Notes |
| --- | --- | --- | --- |
| A person mentions it, DMs it, posts in its group | yes, `reply.requested` | yes | `replies.rs:97-158` |
| Follow-up in a thread it works in | yes, `reply.followup` | yes | `replies.rs:120-123`, `:243-300` |
| Stop | yes, `reply.cancelled` | yes | `replies.rs:1216-1230` |
| Another agent mentions it in a channel or thread | no | `message.mentioned` only, nothing to claim | `replies.rs:105`; `conversations.rs:2351-2423` |
| Task assigned to it, or an assigned task changes | no | `work_item.assigned`, `work_item.changed` | `work_item_commands.rs:947`, `:2834`, `:2883`, `:2917`; `work_item_workflow.rs:830` |
| A task it watches changes | no | `work_item.changed` | `work_item_commands.rs:2656-2688`, filtered to `actor.kind = 'external_agent'` |
| Mentioned in a task comment | no | no (only the assignee or watchers hear `work_item.changed`) | comment mentions feed people's notifications only (`notifications.rs:12`); an agent's own comment can't mention anyone (`mcp/writes.rs:1255`) |
| Mentioned in a page comment | no | no | `page_docs/comments.rs` writes no agent event |
| Approval waiting on it | no | `approval.requested`, only for server-run runs, only roles owner, admin, member | `external_events.rs:142-246` (role list at `:221`) |
| Its question to a person answered | no | yes | `questions.rs:382` |
| Its reminder is due | no | `reminder.due` | `later.rs:1002-1040` |
| Messages in conversations it is in (ambient) | no | no | none |
| Pause, resume, schedule, run start | no | no | none |

### 2.4 The two field proofs

- **Hermes** (`docs/compatibility/hermes-agent.md`, `tools/hermes-connector/`) is an MCP
  client of Workspace with an 18-tool allowlist (`hermes-agent.md:61-80`) that polls
  `external_event_poll`. Nothing pushes into Hermes's gateway; Workspace is not a Hermes
  platform. The note says "the exact 22 tools" (`:118`) but lists 18.
- **The Agent OS bridge** (`integrations/agentos-nyx/`) is a bridged-mode prototype:
  `wakeup.py` verifies the legacy signature and starts a systemd unit; `bridge.py` claims,
  builds a prompt with names in the text (`bridge.py:172-190`), submits an Agent OS turn
  with the current `source_revision` (`bridge.py:66-71`, `:250-258`), polls every 8 s
  (`bridge.py:276-283`), parses a JSON pass-or-reply decision out of model text
  (`bridge.py:209-226`) and answers. It hard-codes the context `nyx-crew` (`bridge.py:49`),
  "Mischa's private agent workspace" (`bridge.py:173`), NixOS paths
  (`systemd/aw-crew@.service`) and MCP revision `2025-06-18` with sessions (`bridge.py:100-103`).
- **The harness** accepts only `inbox.available` (`harness/crates/daemon/src/wakeup.rs:155-157`)
  and pins MCP `2025-06-18` (`harness/crates/workspace/src/client.rs:19`). If its webhook is
  re-saved in the web app it silently loses wake-ups (`external_agents.rs:690-691`).

### 2.5 Teammate pieces that exist

- Group turn allowance of 6 agent messages, reset by a person (`replies.rs:46`, `:129-156`),
  "Continue" for people only (`replies.rs:854-953`).
- Presence derived from claims, sessions and runs (`agent_profiles.rs:1180-1312`), with
  hard-coded harness names as fallback (`agent_sessions.rs:73-84`).
- Agents can already save reminders for themselves (`later.rs:1-13`, `:1002-1040`) and
  schedule messages (`scheduled_messages.rs:1-20`): the seed of self-scheduled follow-ups.
- Project guide pages (`project_pages.is_guide`, `migrations/0042_agent_interface.sql:197`,
  `:212`) are the seed of memory pages.
- An app acting for a person runs under a task-local narrowing of permissions
  (`policy.rs:786-838`, `AgentLimits`, `with_agent_limits`). This is the mechanism to reuse for
  "limited to what the asker could do".
- Automations act as their maker and turn off after 30 firings an hour
  (`automations.rs:1-27`); actions are launch, set field, notify, post message
  (`automations.rs:208`, `:332-369`). There is no "ask an agent".

### 2.6 What is in the way

- Wake-up delivery holds a transaction across HTTP calls (B finding 11, `replies.rs:1315-1436`).
- Cursors die on credential rotation (`external_events.rs:298-304`), so a rotated agent loses
  its place.
- Agent-only and person-only branches: `replies.rs:105`, `:129`, `:171`, `:208`, `:231`,
  `:263`, `:860`, `:1152`; `mcp/replies.rs:1086`, `:1147`; `external_agents.rs:1288`;
  `webhooks.rs:304-313`; `external_events.rs:111`, `:216`, `:221`;
  `work_item_commands.rs:2665`; `migrations/0033_m9_external_agent_connections.sql:84-92`.
  Section 6 says what replaces each.
- Vendor names in core: `agent_setup_links.rs:36`, `agent_sessions.rs:79-80`,
  `runnerPresets.ts:15-16`, `docs/agent-runner.md:3-8`.
- Several `to_regclass` probes on hot paths (`conversations.rs:2364-2371`,
  `external_events.rs:95-101`) exist only for old migration tests.

### 2.7 Keep as is

The reply-request state machine with leases, generations, follow-ups queued behind claims and
`withdraw_requests` as the one withdrawal (`replies.rs:300-482`); thin payloads with no text;
SSRF-safe target checks with address pinning (`replies.rs:1601-1680`); secrets in the secrets
store; `webhooks::visible_to` as the shape of a delivery-time check; the runner contract page;
the runner SDKs (`sdks/runner-typescript`, `sdks/runner-python`).

## 3. The target design

### 3.1 Vocabulary and ownership

- **Connection**: how one account is reached. Any account may have connections (an agent's
  runtime, a person's outgoing webhook, a host serving many agents). Table `agent_connections`
  (the lead's working name; the account kind is not restricted).
- **Subscription**: which event kinds and scopes a connection hears, and the lowest "why told"
  level it wants. Table `event_subscriptions`.
- **Delivery**: one event read for one connection. Stream and poll deliveries are reads of
  K3's log; only webhook pushes keep per-event state, in `deliveries`.
- A2's Rust modules: `delivery` (catalogue, reasons, read path, transports), `connect`
  (connections, subscriptions, setup links, plugin, add-agent flow, host offers,
  `agent_register` and actions), `teammate` (follows, ambient, chains, schedules, status,
  pause, presence), `guards`, `memory_pages`. Budgets are rows in H1's `spend_limits`
  checked by H1's `admit` (R13), so A2 has no budget module of its own.

**Accounts across workspaces (R19).** Agents are global accounts with an owner (a person,
"my agent", or a workspace, "the team's agent"; K1) and may be members of several
workspaces, as a person may. In A2's terms:

- **Global, belonging to the account:** connections, signing secrets, the account's
  credentials, its profile, its outside reference, its DMs (R20).
- **Per workspace:** membership and role, space memberships, spending limits, runs,
  schedules, follows, conversation settings, and the work line of its status. Subscriptions
  name which workspaces they cover ("all" or a list) and whether DMs are included.
- **Managing an owned agent (R34):** its owning person; anyone with `agents.manage_any` in
  the owning workspace; or a manager named on the account who holds `agents.manage`. Holding
  `agents.manage` alone is not enough.
- A connection reaches every workspace the account is in unless it is narrowed to one
  workspace or to spaces (K1's narrow tokens). Its stream merges those workspaces on K3's
  global position, so one cursor still works, and every event names its workspace. Every
  operation call names the workspace it acts in (R4's optional `workspace` argument); a
  credential that reaches one workspace needs none.
- What others see of an agent here is its profile and its membership here; the scope screen
  shows only a yes or no for "also a member of other workspaces", never which.

### 3.2 Data model

Per-workspace tables carry `workspace_id` (today `organizations.id`, R1). Connections and
their secrets belong to the account (R19) and are on H1.3's list of global tables;
`agent_connections.workspace_id` is only a narrowing. Timestamps are `TIMESTAMPTZ`.

```sql
-- How an account is reached and told. One account, many connections.
CREATE TABLE agent_connections (                   -- global (R19); id = external_agent_connections.id for copied rows
    id UUID PRIMARY KEY,
    workspace_id UUID REFERENCES organizations(id),   -- NULL: every workspace the account is in; set: narrowed to one
    narrow_space_ids UUID[],                          -- optional narrowing to spaces (K1 narrow tokens)
    account_id UUID NOT NULL REFERENCES actors(id),   -- K1 accounts
    host_id UUID REFERENCES hosts(id),                -- A1 hosts: a host connection hears for every account it serves
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    preferred_transport TEXT NOT NULL DEFAULT 'poll'
        CHECK (preferred_transport IN ('poll', 'stream', 'webhook')),
    webhook_url TEXT CHECK (webhook_url IS NULL OR char_length(webhook_url) <= 500),
    webhook_format TEXT NOT NULL DEFAULT 'standard'
        CHECK (webhook_format IN ('standard', 'typed_v1', 'inbox_v1')),
    push_cursor BIGINT NOT NULL DEFAULT 0,        -- K3 log `position` (global) the pusher has queued up to
    read_cursor BIGINT NOT NULL DEFAULT 0,        -- last position the client presented (lag, health)
    state TEXT NOT NULL DEFAULT 'active'
        CHECK (state IN ('active', 'push_disabled', 'disabled', 'removed')),
    disabled_reason TEXT CHECK (disabled_reason IS NULL OR char_length(disabled_reason) <= 300),
    health TEXT NOT NULL DEFAULT 'unknown'
        CHECK (health IN ('unknown', 'healthy', 'degraded', 'unreachable')),
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    first_failure_at TIMESTAMPTZ,
    last_seen_at TIMESTAMPTZ, last_delivered_at TIMESTAMPTZ,
    last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 200),
    client_info JSONB NOT NULL DEFAULT '{}'::jsonb,   -- {name, version, mcpRevision, formats}
    legacy_connection_id UUID UNIQUE REFERENCES external_agent_connections(id),
    legacy_webhook_id UUID UNIQUE,                    -- webhook_subscriptions(id) it replaced
    push_owner TEXT NOT NULL DEFAULT 'delivery' CHECK (push_owner IN ('legacy', 'delivery')),
    created_by_account_id UUID NOT NULL REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, name),
    CHECK ((preferred_transport = 'webhook') <= (webhook_url IS NOT NULL))
);
CREATE INDEX agent_connections_account ON agent_connections (account_id) WHERE state <> 'removed';
CREATE INDEX agent_connections_push ON agent_connections (account_id)
    WHERE webhook_url IS NOT NULL AND state = 'active' AND push_owner = 'delivery';

-- Signing secrets by reference to the secrets store; two may be live during rotation.
CREATE TABLE connection_signing_secrets (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
    secret_id UUID NOT NULL,                      -- secrets store row (S1)
    algorithm TEXT NOT NULL DEFAULT 'hmac_sha256' CHECK (algorithm IN ('hmac_sha256', 'ed25519')),
    state TEXT NOT NULL CHECK (state IN ('active', 'retiring', 'retired')),
    retire_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX one_active_secret ON connection_signing_secrets (connection_id) WHERE state = 'active';

-- What a connection hears. Addressed events (section 3.3) are always delivered.
-- A subscription names the workspaces it covers among those the credential reaches (NULL:
-- all of them, including ones the account joins later) and whether DMs and groups in the
-- direct scope are included (R19, R20). All of a connection's subscriptions are read with
-- one cursor over K3's global position. Space and conversation scopes inside a workspace
-- (scope = 'spaces' | 'conversations') stay per subscription, so per-workspace scopes are
-- one subscription per workspace.
CREATE TABLE event_subscriptions (
    id UUID PRIMARY KEY,
    workspace_ids UUID[],                         -- NULL: every workspace the credential reaches
    includes_dms BOOLEAN NOT NULL DEFAULT true,   -- the direct scope (R20)
    connection_id UUID NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
    kinds TEXT[] NOT NULL CHECK (cardinality(kinds) BETWEEN 1 AND 64),  -- 'task.assigned', 'task.*'
    scope TEXT NOT NULL CHECK (scope IN ('visible', 'spaces', 'conversations', 'followed', 'addressed')),
    space_ids UUID[], conversation_ids UUID[],
    min_reason TEXT NOT NULL DEFAULT 'addressed'
        CHECK (min_reason IN ('addressed', 'following', 'ambient', 'visible')),
    set_by_account_id UUID NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((scope = 'spaces') = (space_ids IS NOT NULL)),
    CHECK ((scope = 'conversations') = (conversation_ids IS NOT NULL))
);

-- Push attempts only. Stream and poll keep no per-event rows.
CREATE TABLE deliveries (
    connection_id UUID NOT NULL REFERENCES agent_connections(id) ON DELETE CASCADE,
    event_seq BIGINT NOT NULL,                 -- K3 log `position`
    event_id UUID NOT NULL,
    workspace_id UUID NOT NULL,                -- the event's workspace (fairness and metering)
    state TEXT NOT NULL DEFAULT 'pending' CHECK (state IN ('pending', 'delivered', 'failed', 'skipped')),
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    lease_token UUID, lease_expires_at TIMESTAMPTZ,
    last_status INTEGER, last_error TEXT CHECK (last_error IS NULL OR char_length(last_error) <= 200),
    elapsed_ms INTEGER, delivered_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (connection_id, event_seq)
);
CREATE INDEX deliveries_due ON deliveries (workspace_id, next_attempt_at) WHERE state = 'pending';
```

Teammate tables (A2.7 to A2.10):

```sql
CREATE TABLE follows (                          -- an account follows a thing or a space
    workspace_id UUID NOT NULL, account_id UUID NOT NULL,
    target_kind TEXT NOT NULL, target_id UUID NOT NULL,   -- K3 thing kind and id; 'space' for a space
    level TEXT NOT NULL DEFAULT 'changes' CHECK (level IN ('changes', 'everything')),
    created_by_account_id UUID NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, target_kind, target_id)
);
CREATE INDEX follows_target ON follows (target_kind, target_id);

CREATE TABLE conversation_agent_settings (     -- per account, per conversation; NULL = inherit
    workspace_id UUID NOT NULL, conversation_id UUID NOT NULL REFERENCES conversations(id),
    account_id UUID NOT NULL, ambient BOOLEAN, reply_cooldown_seconds INTEGER,
    set_by_account_id UUID NOT NULL, updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, account_id)
);

-- Loop limits as settings at workspace, space or conversation level (narrowest wins).
CREATE TABLE conversation_limits (
    workspace_id UUID NOT NULL, scope_kind TEXT NOT NULL CHECK (scope_kind IN ('workspace','space','conversation')),
    scope_id UUID NOT NULL, max_chain_depth SMALLINT CHECK (max_chain_depth BETWEEN 0 AND 50),
    turn_allowance SMALLINT CHECK (turn_allowance BETWEEN 0 AND 200),
    PRIMARY KEY (scope_kind, scope_id)
);
ALTER TABLE messages ADD COLUMN chain_depth SMALLINT NOT NULL DEFAULT 0;
ALTER TABLE reply_requests ADD COLUMN chain_depth SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN reason TEXT NOT NULL DEFAULT 'mentioned',          -- why the account was asked
    ADD COLUMN requested_by_account_id UUID,                      -- the message author
    ADD COLUMN claim_due_at TIMESTAMPTZ, ADD COLUMN unresponsive_at TIMESTAMPTZ;
-- conversations.agent_turns_remaining is kept and generalised to every conversation kind.

CREATE TABLE agent_schedules (                 -- follow-ups Workspace keeps, and ones it is told about
    id UUID PRIMARY KEY, workspace_id UUID NOT NULL, account_id UUID NOT NULL,
    kept_by TEXT NOT NULL CHECK (kept_by IN ('workspace', 'agent', 'host')),
    about_kind TEXT, about_id UUID, note TEXT CHECK (char_length(note) <= 1000),
    next_at TIMESTAMPTZ, cron TEXT, time_zone TEXT,
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'done', 'cancelled')),
    client_request_id TEXT, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, client_request_id)
);

-- The status line is C2's `account_statuses` (R12), keyed by account and nullable workspace
-- (R19): a line an agent or host writes about work in a workspace is stored for that
-- workspace and shown only there; a line set by hand is global. A2 writes it through C2's
-- `status_set`: `workspace` set for a work line, null for the global status. A2 has no
-- status table of its own; `working`, `asleep` and `unresponsive` are derived, not stored.
-- Pause is A2's own small table:
CREATE TABLE account_pauses (
    account_id UUID NOT NULL REFERENCES actors(id),
    workspace_id UUID REFERENCES organizations(id),   -- NULL: paused everywhere, by a manager of the account
    paused_by_account_id UUID NOT NULL REFERENCES actors(id),
    reason TEXT CHECK (reason IS NULL OR char_length(reason) <= 300),
    until TIMESTAMPTZ, paused_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT (account_id, workspace_id)
);
```

Budgets and consent (A2.14, A2.15). A budget is a row in H1's `spend_limits` (R13): scope
`workspace | account | space | conversation`, `per_account` for "each account in the scope
on its own" (minute or hour only), `meter` one of A2's meters (`operations`, `writes`,
`messages`, `deliveries`, `reported_cost_micros`, `reported_tokens`) or H1's
(`agent_service_micros` for a billed host), `period` minute to month, `soft_limit` (warn) and
`hard_limit` (refuse; for `deliveries`, hold). Counting is H1's `metering::record` and
checking is H1's `admit`; minute and hour live in K5's limiter store, day and month in H1's
`usage_counters`. A2 adds no budget tables. Consent:

```sql
CREATE TABLE conversation_app_consents (       -- private conversations and DMs, per app
    conversation_id UUID NOT NULL, app_account_id UUID NOT NULL,
    granted_by_account_id UUID NOT NULL, granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (conversation_id, app_account_id)
);
```

Memory pages reuse C1's pages: `project_pages` gains `memory_for_kind TEXT CHECK (memory_for_kind
IN ('space', 'conversation'))` and `memory_for_id UUID`, with a unique partial index on
`(memory_for_kind, memory_for_id) WHERE archived_at IS NULL`, like the guide index
(`migrations/0042_agent_interface.sql:212`).

**Why read the log instead of fanning out per connection.** Today `external_agent_events`
writes one row per connection per event (`external_events.rs:37-85`). That multiplies writes by
connections, needs its own retention and can disagree with what browsers saw. The end state
says the agent stream is the log read with a cursor. So stream and poll read K3's log with
its per-workspace index (`events_workspace (workspace_id, position)`, R8) over the workspaces
the connection reaches, and filter by subscription and access in one SQL function; only
pushes, which need attempt state, get rows. Cost: a connection scans every event of the
workspaces it reaches once as its cursor moves. That is bounded by those workspaces'
activity and the per-workspace caps in 3.6.

### 3.3 Addressed and broadcast events, and why the agent is told

K3's log holds facts ("message created in conversation C"). Some facts are addressed to an
account ("reply requested of agent A"). K3's canonical row (R8) carries
`audience_account_id UUID NULL` (one addressed account; NULL means broadcast; a fact for
several accounts is emitted once per account in the same transaction), and
`requested_by_account_id`, `via_app_id`, `actor_account_id`, `run_id`, `request_id`.
A host connection reads events addressed to any agent it serves (`host_agents`) and the
`host.command` events addressed to the host's own app account; access is re-checked for the
agent the event is addressed to, not for the host.

`delivery::reason(event, connection, subscription) -> Option<Reason>` decides, in order:

| Reason (`reason` field) | When | Level |
| --- | --- | --- |
| `reply_required` | a required reply request addressed to this account | addressed |
| `reply_welcome` | an optional reply request (group, ambient, agent mention under limits) | addressed |
| `mentioned` | the event's mentions include the account | addressed |
| `assigned` | a task's assignee became the account | addressed |
| `request_for_you` / `request_answered` | an A1 request addressed to it, or one it raised was answered | addressed |
| `host_command` | `run.start`, `run.cancel` for a run on this account; `host.command` for this host | addressed |
| `lifecycle` | `agent.paused`, `agent.resumed`, `schedule.due`, `connection.*` | addressed |
| `self` | the account caused it (echo); delivered only if the subscription asks for echoes | addressed |
| `following` | the thing or its space is in `follows` | following |
| `ambient` | conversation where `ambient` is on for this account | ambient |
| `visible` | anything else in scope the account can see | visible |

A subscription with `min_reason = 'addressed'` hears only the first block. Addressed events
ignore `kinds` filters except that an account may mute non-required kinds by listing them in a
`muted` form later (not in the first slices).

**Access at delivery.** One SQL function, written once and used by every transport:

```sql
CREATE FUNCTION delivery_visible(recipient UUID, ev events) RETURNS boolean
LANGUAGE sql STABLE AS $$
    SELECT (ev.workspace_id = direct_scope_id()                        -- R20: participants only, below
            OR account_in_workspace(recipient, ev.workspace_id))       -- K1
       AND (ev.audience_account_id IS NULL OR ev.audience_account_id = recipient)
       AND (ev.space_id IS NULL OR account_can_see_space(recipient, ev.space_id))       -- K1/K4
       AND (ev.access_thing_id IS NULL                                 -- K3: the thing access is decided on
            OR thing_visible_to(ev.access_thing_id, recipient))        -- K1; for a conversation this is
                                                                       -- conversation_visible_to (R3), which
                                                                       -- pairs the space check (B finding 6)
       AND delivery_still_relevant(recipient, ev)                     -- A2, per kind
$$;
```

`delivery_still_relevant` keeps today's "still true" rule (`replies.rs:1325-1332`): a
`reply.requested` is dropped if the request is no longer pending, a `reply.cancelled` only if
it was cancelled, `schedule.due` only if the schedule is still active. It applies to stream and
poll too, so all three transports agree.

### 3.4 The event catalogue

Registered in `delivery/catalogue.rs` as `CatalogueEntry { kind, family, addressed, data_schema,
since, retired_alias }`, exported as JSON Schema at `GET /api/v1/events/catalogue` and into the
Agent Plugin. Names follow K3's `<thing>.<verb>`. Every payload is thin: ids, kinds, names
needed to route, never message text, page body or file contents.

**Envelope** (identical in all transports; for webhooks it is the Standard Webhooks body):

```json
{
  "type": "reply.requested",
  "timestamp": "2026-11-20T09:14:03.112Z",
  "id": "evt_0199a1c2-7d3e-7c11-9a4f-3b0e1d2c4f55",
  "cursor": "c2.AZmhwn0-fBGaTzsOHSxPVQ.Ab3x9Q",
  "workspace": { "id": "0199…", "slug": "acme", "kind": "team" },
  "recipient": { "accountId": "0199…aa" },
  "reason": "reply_required",
  "actor": { "accountId": "0199…p1", "kind": "person", "name": "Ana", "handle": "ana" },
  "requestedBy": null,
  "via": null,
  "data": { }
}
```

`actor.kind` is for display and routing only (an adapter's `chat_type`, echo checks), never
for permissions. `self: true` is added when `actor.accountId` equals the recipient.
`workspace` is on every event (R19); for a DM or group in the direct scope it is
`{"id": "…", "kind": "direct"}` with no slug (R20).

| Kind | `data` example | Who |
| --- | --- | --- |
| `reply.requested` | `{"replyRequestId","conversation":{"id","kind":"thread","name","parentId"},"messageId","generation":0,"required":true,"chainDepth":1,"followsRequestId":null,"session":{"ref":"…","backend":null}}` | asked account |
| `reply.followup` | as above with `followsRequestId` set | asked account |
| `reply.cancelled` | `{"replyRequestId","conversationId","stoppedBy":{"accountId","name"}}` | asked account |
| `message.created` | `{"conversation":{"id","kind":"channel","name","spaceId","visibility":"space"},"messageId","threadId":null,"replyRequestId":"…or null","mentions":["0199…"],"attachments":[{"fileId","name","mimeType","bytes"}],"clientMessageId":null,"chainDepth":0}` | subscribers; `replyRequestId` set when one was made for the recipient |
| `message.updated`, `message.removed` | `{"conversationId","messageId","revision":3}` | subscribers |
| `task.assigned` | `{"task":{"id","key":"AWD-142","spaceId","title"},"assigneeAccountId","previousAssigneeAccountId":null,"version":7}` | new assignee (addressed), followers |
| `comment.mentioned` | `{"thing":{"kind":"task","id","key"},"commentId","conversationId":"…comments conversation"}` | mentioned account (C1.1's name; covers task and page comments) |
| `task.delegated` | `{"task":{"id","key"},"delegateAccountId","runId":null}` | the delegate (T1.22's event) |
| `task.changed` | `{"task":{"id","key"},"changedFields":["status","due"],"version":8}` | assignee, followers, subscribers |
| `request.opened` | `{"requestId","kind":"approval","runId":null,"thing":{"kind":"task","id"},"eligible":"you"}` | each account the A1 request names or whose role may answer (one event each, reason `request_for_you`) |
| `request.answered` | `{"requestId","kind":"question","answer":"approved","answeredBy":{"accountId"}}` | the asker |
| `run.start` | `{"runId","accountId","cause":{"kind":"task","id"},"startKey":"run-0199…","inputRef":"/api/v1/runs/0199…/start-input"}` | self-connecting agent (A1 3.12; the input is A1's `run_start_input`) |
| `run.cancel` | `{"runId","reason","requestedBy":{"accountId"}}` | as above |
| `host.command` | `{"commandId","seq","kind":"start","runId"}` (thin; fetched from A1's command address) | the host's app account |
| `proposal.decided` | `{"proposalId","decision":"accept","decidedBy":{"accountId"}}` | the proposer (C1's event) |
| `agent.paused`, `agent.resumed` | `{"accountId","by":{"accountId"},"reason":"Budget review","until":null}` | the agent's connections |
| `schedule.due` | `{"scheduleId","about":{"kind":"task","id"},"note":"Check the deploy","dueAt":"…"}` | the agent |
| `followed.changed` | `{"target":{"kind":"page","id"},"changeKind":"page.updated","eventId":"evt_…"}` | followers whose subscription excludes the underlying kind |
| `connection.notice` | `{"code":"format_retiring","message":"…","sunset":"2027-06-01"}` | the connection |
| `connection.push_disabled` | `{"reason":"No 2xx for 48 hours","since":"…"}` | the connection (stream and poll) |
| existing `task.created`, `task.updated`, `run.updated`, `run.finished`, `pull_request.*`, `record.created`, `record.updated` | as `events.rs:34-52` today | subscribers |

Old names stay accepted as aliases in subscriptions and are delivered under their old names to
legacy formats: `work_item.assigned` = `task.assigned`, `work_item.changed` = `task.changed`,
`message.mentioned` = `message.created` with reason `mentioned`, `approval.requested` and
`approval.resolved` = `request.*`, `reminder.due` = `schedule.due`. An answer to a run's
request reaches a self-connecting agent as `request.answered` and a host as A1's `answer`
command; there is no separate `run.answer`.

### 3.5 Transports on one cursor

**Cursor.** `c2.<base64url(position)>.<hmac16>`, K3's global position (one cursor for every
workspace the connection reaches, R19), signed with a server key and bound to the connection
id inside the HMAC, not to the credential, so rotation keeps the agent's place
(fixes `external_events.rs:298-304`). A cursor older than K3's retention returns
`cursor_expired` with `resync: "call briefing"`.

**Poll and long-poll.** `GET /api/v1/events?cursor=…&limit=100&wait=30` (agent bearer, any
account's credential with a connection). JSON:
`{"events":[…],"nextCursor":"…","hasMore":false}`. `wait` up to 30 s; the request does not hold
a transaction while waiting (today's poll does, `mcp/collaboration.rs:205-300`); it waits on
K5.9's `realtime::follow` for the workspaces it reaches, then re-reads; without a
notification it re-reads the log position on a short interval (R24 rule 3). Also the operation `event_list` (K2 generated MCP
tool and public API) with the same inputs, so MCP-only agents poll with a tool.

**Stream.** The same address with `Accept: text/event-stream`. Resume from `Last-Event-ID` or
`?cursor=`. Wire:

```
retry: 5000

id: c2.AZmhwn0-fBGaTzsOHSxPVQ.Ab3x9Q
event: reply.requested
data: {"type":"reply.requested","id":"evt_…", … }

: keep-alive 2026-11-20T09:14:18Z
```

Heartbeat every 15 s. The stream is served from the log, so any process can serve it (K5):
the handler reads `read_batch` after the cursor, sends, then waits on K5.9's `follow` for the
reached workspaces (fallback: re-read every 2 s). Access is decided per event in
`read_batch`, and the stream also closes with `event: connection.closed` when the credential is
revoked (checked every 60 s and on K1's revocation event).

**Webhook (push).** Standard Webhooks:

```
POST /hooks/workspace HTTP/1.1
content-type: application/json
webhook-id: evt_0199a1c2-7d3e-7c11-9a4f-3b0e1d2c4f55
webhook-timestamp: 1763630043
webhook-signature: v1,K5oZfzN95Z9UVu1EsfQmfVNQhnkZ2pj9o9NDN/H/pI4= v1,9Lq…(previous secret during rotation)
x-workspace-delivery: evt_0199a1c2-7d3e-7c11-9a4f-3b0e1d2c4f55     (alias, standard format, until sunset)
x-workspace-event: reply.requested                                   (alias)
x-workspace-timestamp: 1763630043                                    (alias)
x-workspace-signature: sha256=<hex HMAC of "{timestamp}.{body}">      (alias)
```

`v1` signs `"{webhook-id}.{webhook-timestamp}.{body}"` with the base64-decoded secret after
`whsec_`. Existing secrets are `whsec_` plus URL-safe base64 (`replies.rs:1755-1760`); the
decoder accepts both alphabets. The alias signature keeps today's key bytes, so existing
receivers verify unchanged. `v1a` (Ed25519, public key at `/.well-known/webhook-keys.json`)
comes later so receivers need no shared secret.

- **Pusher.** Job kind `delivery.push`, unique per connection, on the job queue (`jobs.rs`),
  so no transaction is held across HTTP (fixes B finding 11). Posts go through H1.2's
  `outbound` client (address checks, timeouts), and each counts against the event's
  workspace as meter `deliveries`; a `hard_limit` refusal from `admit` holds the delivery. It reads `read_batch` from
  `push_cursor`, inserts `deliveries` rows, advances `push_cursor`, commits, then posts up to 4
  at once per connection, records results in a second short transaction. Webhook ordering is
  not guaranteed; receivers order by `cursor` if they care.
- **Retries over hours.** 0 s, 5 s, 5 min, 30 min, 2 h, 5 h, 10 h, 10 h (about 27.6 hours),
  then `failed`. A failed push is still in the log: the next poll or stream from the client's
  cursor catches it.
- **Disable and tell.** When a connection has had no 2xx for 48 hours with at least 20
  consecutive failures, `state = 'push_disabled'`, `disabled_reason` set, a
  `connection.push_disabled` event goes on the stream and poll, and every account that manages
  the agent gets a notification and an inbox card (kind `connection_problem`, delivered by
  C2.5; A2 only emits the fact). The agent page shows a banner with "Turn pushes back on".
- **Per-workspace fairness.** The pusher's claim query skips deliveries whose event's
  workspace has 16 posts in flight; one connection has at most 4. Slow receivers time out at 8 s as today.
- **Rotation.** `connection_secret_rotate` makes a new active secret and marks the old one
  `retiring` with `retire_at = now() + 24h`; during that day both signatures are sent.

**Legacy formats as adapters.** `webhook_format` picks the body. `typed_v1` sends today's body
(`replies.rs:1452-1467`) for `reply.*` only; `inbox_v1` sends `{"event":"inbox.available",
"deliveryId","agentId","replyRequestId"}` for `reply.requested` and `reply.followup` only, as
`replies.rs:1444-1450` does. Both send only the `x-workspace-*` headers. Both add
`Deprecation: @<unix>` and `Sunset: <HTTP date>` headers (RFC 9745, RFC 8594), and
once a day queue a `connection.notice` with code `format_retiring` in the poll and stream.
Re-saving a webhook keeps its format; changing format is an explicit choice in the form
(fixes `external_agents.rs:690-691`; done by P0.18 if P0 lands first, R10). Old MCP revisions
and these wake formats stay served and are pinned by P0.17's contract tests. The harness, which accepts only `inbox.available`
(`harness/crates/daemon/src/wakeup.rs:155`), keeps working until it moves; its manager sees
"This agent uses an old wake format; it retires on 2027-06-01" on the agent page, with a link
to the migration note in `docs/agent-runner.md`.

### 3.6 Hosted operation

- Index `events_workspace (workspace_id, position)` (K3, R8) keeps every read to the
  workspaces a connection reaches.
- Caps (defaults, values from H1's plan through K5.6's `LimitSource` when hosted): 3 open
  streams per connection, 100 per workspace (a multi-workspace stream counts in each),
  long-polls count as streams; 16 pushes in flight per workspace; `event_list` at 60
  calls a minute per connection through K5's shared rate limits.
- Webhook targets keep today's checks (`replies.rs:1601-1680`), including
  `AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS` for self-hosters.
- Payloads stay thin, so a leaked delivery reveals ids only.

### 3.7 Operations (K2 registry; each has an agent tool and an isolation case)

| Operation | Kind | Inputs | Permission (K1 catalogue) | Effect |
| --- | --- | --- | --- | --- |
| `connection_list` | query | `accountId?` | self, or manage that agent | connections with health |
| `connection_write` | change | `id?`, `name`, `preferredTransport`, `webhookUrl?`, `webhookFormat?`, `clientInfo?`, `expectedVersion`, `clientRequestId` | self (an agent configures its own), or `agents.manage` + named manager | create or change; a new webhook returns the secret once |
| `connection_secret_rotate` | change | `id` | same | new secret, old retiring 24 h |
| `connection_test` | change | `id` | same | signed `ping` now, result shown (today's `send_test_wakeup`, `replies.rs:1517-1595`) |
| `connection_remove` | change | `id` | same | state removed, pushes stop |
| `subscription_get` / `subscription_write` | query / change | `connectionId`, list of `{workspaces: "all" \| [ids], includesDms, kinds, scope, spaceIds?, conversationIds?, minReason}` | same | replaces the connection's subscriptions; a workspace the credential doesn't reach is refused (R19, R20) |
| `event_list` | query | `cursor?`, `limit?`, `wait?` | holder of a connection | the read path above |
| `event_catalogue` | describe | none | anyone signed in | catalogue JSON |
| `follow_set` | change | `target {kind,id}`, `level`, `on: bool` | see the target | follows row |
| `conversation_agent_settings_set` | change | `conversationId`, `accountId`, `ambient?`, `replyCooldownSeconds?` | `conversations.agent_settings` in that space | settings row |
| `conversation_limits_set` | change | scope, `maxChainDepth?`, `turnAllowance?` | `projects.manage` or `settings.manage` (K1's stored keys for K4's names) | limits row |
| `conversation_continue` (exists) | change | `conversationId` | `conversations.continue_agents` (K1.14; default every role that can post) | resets turns |
| `reply_stop` (exists) | change | `requestId` | `conversations.stop_agents` (K1.14, same default) | as today |
| `reply_stop_all` | change | `conversationId` | `conversations.stop_agents` | stops every open reply request in the conversation and asks A1 to cancel runs started from it (C2.35's Stop all) |
| `schedule_write` / `schedule_list` | change / query | `about?`, `at?`, `cron?`, `timeZone?`, `note`, `keptBy`, `clientRequestId` | self, or manage the agent | `agent_schedules` |
| `status_set` (C2's operation, R12) | change | `text?`, `emoji?`, `clearAt?`, `awayUntil?`, `workspace?` | self, the account's host, or a manager of the account | C2's `account_statuses`; a host's or agent's work line is stored for the workspace named (R19) |
| `agent_pause` / `agent_resume` | change | `accountId`, `workspace?`, `reason?`, `until?` | a manager of the account under R34 (owning person, `agents.manage_any` in the owning workspace, or a named manager holding `agents.manage`) for everywhere; `agents.manage` in that workspace for there only | `account_pauses`; events |
| `presence_set` | change | `conversationId`, `state: typing|working|idle`, `ttlSeconds ≤ 60`, `note?` | post in the conversation | ephemeral, K5 fan-out |
| `file_link` | query | `fileId` | see the file | URL valid 5 min, fetchable with the token or alone |
| `budget_list` / `budget_write` | query / change | scope, `perAccount`, meter, period, `softLimit`, `hardLimit` | `budgets.manage` at that scope | H1's `spend_limits` row (H1.27's operation under A2's name on the agent page) |
| `agent_scope_get` | query | `accountId` | see the agent | the scope screen's data |
| `app_consent_set` | change | `conversationId`, `appAccountId`, `granted` | participant of that conversation (private, DM) or space admin (shared) | consent row |
| `memory_page_open` | query | `space` or `conversation` | see it | creates on first edit, returns the page |
| `agent_offer_list` | query | `workspace` | may add agents here (K1) | offers from every host the caller may use here: hosts owned by this workspace or by the caller (R19, R21) |
| `agent_register` | change | `hostAgentRef`, `profile {name, handle?, avatarUrl?, summary?}`, `ref {issuer, id, lineage}`, `reach {connection?, hostId?}`, `clientRequestId` | the host's app account (proposes) | a proposal; on approval, the account (owned as the host's owner chose), the `host_agents` binding, a connection; see 3.9 |
| `agent_register_decide` | change | `proposalId`, `decision`, `role`, `spaceIds` | may add agents of that role here (K1) | approves or refuses; approving joins the account to this workspace |
| `agent_action_invoke` | change | `accountId` or `hostId`, `actionId`, `input?` | `agents.manage` on that agent (or managing the host) | A1 `action` host command; returns the host's panel or result |
| `agent_join` | change | `accountId`, `role`, `spaceIds` | may add agents of that role here, and a manager of the account agrees (K1's add-member rule, R19) | an agent the caller already has in another workspace becomes a member here |

### 3.8 Being a teammate

**Following.** `follows` generalises `work_item_watchers` (task_watch) to any thing and to
spaces. `level = 'changes'` hears K3 `*.updated` of the thing; `everything` adds comments and
messages in its discussion. Following is open to people too: C2 reads `follows` for notifications.
The existing `work_item_watchers` rows are copied in and kept in step by trigger until C2's
notification code reads `follows` (trigger table in 4.1).

**Ambient mode.** Per account and conversation (`conversation_agent_settings.ambient`), with
inheritance from the space's default for that account. When on: (1) the account hears every
`message.created` there with reason `ambient`, whatever its subscription's `min_reason`; (2)
each message posted at chain depth 0 creates an optional reply request (`reason =
'ambient'`) unless the account already has an open request there or answered within
`reply_cooldown_seconds` (default 300). It answers with `reply_send` or `reply_pass`, so claims,
turns and Stop work as in groups. When off, an account can still subscribe to see a
conversation, but Workspace does not invite it to speak.

**Mentions by anyone, loop limits as settings.** Replace the author-kind gate
(`replies.rs:105`, `:129`, `:171`) with chain depth:

- A message's `chain_depth` is 0 when its author posted it without being prompted. It is
  `d + 1` when the author posted it while holding a claim on a request of depth `d` in that
  conversation (or through `reply_send`), or when the author's connection was delivered a
  `message.created` of depth `d` in that conversation in the last 10 minutes and the call names
  it in `inResponseTo` or holds no claim elsewhere (inferred heuristics are written down in
  `teammate/chain.rs` and tested). A person typing in the browser is unprompted because they
  hold no claim and have no connection, not because they are a person.
- A mention or a group message creates requests at the message's depth. No request is created
  at depth ≥ `max_chain_depth` (default 4) or when the conversation's turn allowance (default 6,
  today's constant `replies.rs:46`) is used up; `agent_turns_remaining` is decremented by every
  message of depth ≥ 1 and reset by a depth-0 message, in every conversation kind.
- When either limit stops a request, the conversation shows "Agents paused here after N turns"
  with Continue, which needs `conversations.continue_agents` (K1.14; default: every role
  that can post). A workspace can give that permission to an agent's role.
- Budgets (3.12) are the backstop: a per-conversation message rate applies to every account.

Who gets asked changes from `actor.kind = 'external_agent'` (`replies.rs:208`, `:231`,
`:263`) to "an account with an active connection subscribed to reply requests"
(`connect::accepts_reply_requests(account)`), which is how the account is told, not what it
may do. A1's built-in host gets a connection, so `queue_tag_launches` (`replies.rs:111`)
becomes one more subscriber (A1 decides when).

**Self-scheduled follow-ups.** `schedule_write` with `keptBy = 'workspace'` stores a time or a
cron; a `teammate.schedule_due` job (K5 leases) fires and emits `schedule.due` addressed to the
account, then computes the next time. Today's agent reminders (`later.rs:1002-1040`) are
copied into `agent_schedules` and `later_save` for an agent writes there too, with
`reminder.due` as an alias. `keptBy = 'agent' | 'host'` records a schedule the agent keeps
itself (in its own runtime or host) so the agent page shows it; Workspace never fires those.

**Status line, schedule and budget on the agent page.** C2's `account_statuses` (R12) plus
derived state, per workspace (R19): `working` from claims, sessions and runs in this
workspace (today's presence query, `agent_profiles.rs:1216-1312`, moves to
`teammate/presence.rs`), `asleep` from `awayUntil`, `paused` from `account_pauses`,
`unresponsive` from the claim sweep. A work line written for workspace A never shows in
workspace B. The page shows the line, the next five schedule entries, and each budget as
used/limit.

**Pause and resume.** `agent_pause` writes an `account_pauses` row (for one workspace, or
for all when a manager of the account pauses it), emits `agent.paused`, and from then on, in
the workspaces it covers: reply rules create requests in state `held` (people see
"paused"), deliveries other than `agent.*`, `reply.cancelled` and `connection.*` are held (the
connection's cursor simply doesn't advance past them in `read_batch`, which filters held kinds
while paused and releases them on resume), and the account's writes do nothing and return
`ok` with the value `{"status": "paused"}` inside the result (R37: a value, not one of K2's
envelope outcomes) except `status_set`, `reply_release` and `run` reporting. Resume reverses it and emits
`agent.resumed`. Pause applies to any account a person manages, so it is not a rule by kind.

**Working indicator.** `presence_set` publishes K5's `typing` frame
(`{conversation, account, state, expires}`) through `realtime::publish` to browsers and adds
no store of its own; `reply_progress` calls it too. The web composer
shows "Fixer is working · Running tests". The indicator never opens a run (R6).

**Echo marking and client message ids.** `message_send` and `reply_send` take
`clientMessageId` (K2's `clientRequestId` convention); the resulting `message.created` carries
it, and `self: true` for the author's own connections, so adapters drop their echoes without
remembering ids.

**Attachments.** `message.created.data.attachments` lists files; `file_link` returns a short
URL from K5's storage seam (signed, 5 minutes); `GET` with the bearer token also works.

**Not responding.** A required request gets `claim_due_at = now() + claim_window`
(workspace setting, default 120 s). A sweep marks unclaimed ones `unresponsive_at`, shows "Fixer
hasn't picked this up" with Ask again and Ask someone else, marks the agent `unresponsive`
in that workspace (derived from `unresponsive_at`, not stored), lowers connection health, and notifies the agent's managers once per hour (kind `agent_unresponsive`, delivered by C2.5).
Today's lapsed-lease "expired" stays (`replies.rs:1264-1285`).

**"Ask an agent" as an automation action.** `Action::AskAgent { agent_account_id,
prompt, place: task_thread | conversation(id), required: bool }`. It posts the prompt as the
rule's maker, via the app "Automations", mentioning the agent, through the ordinary send
pipeline (`append_message_command_with`), so the request, chain depth, budgets and
`requested_by = maker` all follow from existing rules. On a task it posts in the task's
discussion conversation (C1) or, until C1 lands, its linked thread. Rule firing limits
(`automations.rs:25-27`) still apply.

### 3.9 Adding an agent: one flow, four paths

Every path ends with: an account (K1, global, with an owner, R19) that is a member here with
the role a person picked (the workspace's default agent role preselected, owner call 3),
memberships in the spaces chosen, a connection, and the agent holding the Agent Plugin or its
parts. The flow has no vendor knowledge: app recipes, host offers and directory entries are
data. Workspace's own step (the role and the spaces) is the only step Workspace owns (R21).

```
Add an agent ─┬─ Pick one ─────── directory (M1 packages with agent profiles)
              │                   or one a linked host already has (its offers) ─┐
              ├─ Connect one ──── name, picture, role, spaces                     │
              │   you run         → setup message or Agent Plugin                 │
              │                   → the agent connects and picks transport        │
              ├─ Start one on ─── offers from hosts linked here or by you        │
              │   a linked host     (none linked: "Link a host you run" (A1),     │
              │                      a hosted agent service from the directory,   │
              │                      or "Run one yourself")                       │
              │                   → the host's own steps: its panels (M1 blocks)  │
              │                     or its page, never Workspace's                │
              │                   → the host calls agent_register ────────────────┤
              └─ Add one you ──── pick from agents you manage in other            │
                  already have      workspaces (agent_join)                       │
                                  → Workspace step: role, spaces ◄────────────────┘
                                  → connected
```

**Pick one.** A directory entry (M1's directory of packages carrying agent profiles), or an
agent a linked host already runs, which the host lists as one of its offers (A1 3.9); its
steps end in `agent_register` like any other offer.

**Start one on a linked host (R21).** `agent_offer_list` collects the offers in the
`describe` of every host the person may use here: hosts owned by this workspace, and hosts
the person owns (R19). Workspace does not know what an offer is ("new agent", "from a
template", "a copy of one", "describe it in a sentence" are the host's words). Choosing one
draws its `start`: a panel in M1's block format from the host's connected module (M1.12,
M1.14), or the host's own page from the origin approved at enrolment, opened in a frame or a
new tab. Anything secret (model keys) is entered on the host's page and never passes through
Workspace. Backends, authority on the machine, proactivity and templates are the host's
panels too. The host's steps end with the host calling **`agent_register`**: it proposes an
account with a profile, an outside reference `{issuer, id, lineage}` (K1.7) and how it is
reached (served by this host, or its own connection). When the steps ran in an M1 panel,
`agent_register` consumes the panel's `done.result` (M1 3.8, carrying `agentRef`); the offers
and each agent's actions are read from the host's `describe`, never from the panel. Workspace opens an A1 request of kind
`approval` addressed to the person who chose the offer; they (or anyone who may add agents of
that role here) approve it with `agent_register_decide`, picking its role and spaces. On
approval Workspace creates the account (owned by the host's owner: the person or the
workspace), the `host_agents` binding, the membership here and a connection, and hands the
host a credential for the account through the command channel. If an account with the same
outside reference already exists and the approver manages it, Workspace offers to join that
account instead of creating a second one. A proposal nobody answers expires after 7 days.

**Actions on an agent (R21).** A host publishes actions for each agent it runs (`{id, label,
needs: confirm | panel, danger?}`) and for itself. The agent's page draws them as buttons (a
`danger` one asks to confirm in red), sends the choice with `agent_action_invoke`, and draws
a returned panel. Workspace has no fixed list of lifecycle verbs: "update", "deploy", "copy",
"move" and "retire" are whatever the host publishes. What Workspace's records need comes back
as the three notices (A1 3.9): profile changed (the profile updates), reference changed with
lineage (the binding follows; identity survives a move), retired (runs interrupted, managers
told; removing it from a workspace is the managers' choice).

**Add one you already have (R19).** "Add one you already have" lists agents the person
manages that are members of other workspaces (only those; never other people's).
`agent_join` makes a membership here with the role and spaces picked. The workspace's
setting on accounts owned outside it (yes, ask an admin, or no) applies: "ask an admin"
opens a request to the workspace's admins. The agent's existing connections reach this
workspace at once unless narrowed; a subscription covering "all" includes it at once, one naming workspaces gets it added on request.

**Connect one you run.** The setup link (`agent_setup_links.rs`) stays, but its page is
generic: token once, MCP address, event address, plugin download, and a short "choose how you
want to be told" section the agent acts on itself with `connection_write`. Client recipes
(today `agent_setup_links.rs:276-296` and `runnerPresets.ts`) move into the plugin package as
data (`recipes/*.md`), so adding a client never touches the server. The pasteable setup message
reads: "Read {link} and follow it. It is for you, {name}."

**The Agent Plugin.** Source in `packages/agent-plugin/` (new):

```
packages/agent-plugin/
  plugin.json            name "agent-workspace", version = server version, description
  mcp.json.tmpl          {"mcpServers":{"agent-workspace":{"type":"http","url":"{origin}/api/v1/mcp",
                          "headers":{"Authorization":"Bearer ${AGENT_WORKSPACE_TOKEN}"}}}}
  skills/
    workspace-briefing/SKILL.md      start with briefing; triage
    answer-a-reply-request/SKILL.md  claim, progress, interim, final, pass, release, Stop
    work-a-task/SKILL.md             claim, criteria, comment, release with handoff
    hear-events/SKILL.md             event address, cursor, transports, reasons, echoes
    be-a-good-teammate/SKILL.md      ambient etiquette, chain limits, status line, schedules
    write-pages-and-memory/SKILL.md  guide, memory page, decisions
    report-your-work/SKILL.md        runs and activities (A1's operations)
  recipes/               per-client setup notes as data
  events/catalogue.json  generated from delivery/catalogue.rs
```

`tools/agent-plugin/build.mjs` checks the skills (front matter, links to tools that exist in
the K2 registry) and the catalogue; the server serves `GET /agent-plugin.zip`, templating
`{origin}` at download; no secret is ever in the package. Skills are also served as MCP
resources once K2 adds the Skills extension.

### 3.10 The channel adapter kit

One reference adapter in each SDK, and two thin plugins built on it.

| Need | Workspace operation |
| --- | --- |
| Receive | event stream, `message.created` with reason, `reply.*` |
| Send | `reply_send` when the event carries `replyRequestId`, else `message_send` with `clientMessageId` |
| Edit, delete | `message_edit`, `message_delete` |
| Typing | `presence_set` |
| Threads | `conversation.kind`, `threadId` on events; `thread_start(fromMessageId)` |
| Attachments | `attachments` on events; `file_link`; `file_upload` |
| Identity | `actor` on events; `people_search`; the account's own id from `connection_list` |
| Mentions | `mentions` on events; `@handle` text resolved by `message_send` |
| Reactions | `message_react` |
| Health | stream heartbeats, `connection.*` events, `status_set` |
| Stop and pause | `reply.cancelled`, `agent.paused`, `agent.resumed` |

- **Reference adapter.** `sdks/runner-typescript/src/channel.ts` and
  `sdks/runner-python/agent_workspace_runner/channel.py`: `WorkspaceChannel` with `connect()`
  (stream with cursor file, falls back to long-poll), `on(kind, handler)`, `send`, `edit`,
  `typing`, `react`, `download`, `ack(cursor)`. The existing `Runner` class is rebuilt on it.
- **Hermes platform plugin.** `sdks/adapters/hermes/` with `plugin.yaml`, `adapter.py`
  (subclasses Hermes's `BasePlatformAdapter`: `connect` opens the stream, inbound events become
  `MessageEvent` with `build_source(chat_id = conversation id, chat_type = dm|group, user_id,
  user_name)`, `send` routes through `reply_send` or `message_send`, `send_typing` calls
  `presence_set`), and `tools.py` exposing nothing extra. `trusted_inbound` stays off;
  Workspace's own access decides who reaches the agent.
- **OpenClaw channel plugin.** `sdks/adapters/openclaw/` (TypeScript `ChannelPlugin`:
  `outbound.sendText/sendMedia`, `threading` route keys `{channel: "agent-workspace", to:
  conversationId, accountId, threadId}`, `sendTyping`, `mentions.readDisplayMentions`,
  `setStatus` from stream health, `groups.resolveToolPolicy` default read-only in shared rooms).
- **Fixtures.** `sdks/fixtures/events/*.json` (one per catalogue kind),
  `sdks/fixtures/transports/{sse,poll,webhook}/*` (recorded wire transcripts, webhook requests
  with headers signed by a fixed test secret and timestamp), `sdks/fixtures/legacy/*` (typed_v1,
  inbox_v1). Both plugins and both SDKs test against them; CI runs SDK tests (closing D finding
  F9's gap) via `tools/ci/checks.sh`.

### 3.11 The Agent OS connector (bridged)

This section is the part in the build order: the generic **bridged** connector, which takes
the owner-specific bridge (`integrations/agentos-nyx/`) out of this repository (principle 6,
R21). Native mode, identity across moves, per-run credentials, relayed approvals and
creation from Workspace need Agent OS work that is not started; they are in the appendix
"Integration, with Agent OS", outside the build order. Nothing in A2's acceptance waits on
Agent OS: every check below runs against a fake controller.

A generic program on Workspace's side, `connectors/agent-os/` (Rust binary
`aw-agentos-connector`, built on A1's host-client crate so it speaks only the public host
protocol and the event stream). It runs beside an Agent OS installation, dials out, and calls
Agent OS through its public controller interface with argument arrays (`agentos controller call
<file>`), as the bridge does (`bridge.py:46-63`). Configuration names everything that today is
hard-coded: `workspace_origin`, the enrolment, `agentos.context`, the controller command, the
agent map. No context, machine, person or path is in code.

| Feature | How | Waits on ask | Works without it |
| --- | --- | --- | --- |
| Enrol and dial out | A1 enrolment code; one stream for every agent it serves (`agent_connections.host_id`) | 2 (enrolment on the Agent OS side) | yes, the connector holds the Workspace side; Agent OS side is the local socket |
| Bridged mode | `reply.requested` → claim → `task-submit` turn with thread = conversation id, idempotency key `workspace-{request}-{generation}` (as `bridge.py:249-258`) → poll `task-inspect`/`task-output` → `reply_send`/`reply_pass`/`reply_release` | 3 for a real sender field and turns against the applied deployment | yes: sender in the prompt, marked as data; `plan` read for `source_revision` each turn |
| Pass or reply in groups | structured: the connector asks for a JSON decision and validates it, as today (`bridge.py:209-226`) | none | yes |
| Attachments | bridged mode lists `file_link` links in the turn | 3 | partly |
| Run mapping (A2.26) | Workspace `run.start` → `run-start` on a task agent; host-started runs found by `run-list` polling are **adopted** into A1's run record; `run-output` final message, diff and PR reported | 4 for steps; 6 not needed with the connector | yes, with steps marked as a gap |
| Usage | attributed by time window (A1 3.4) | 5 for per-run usage | yes, `estimated` |
| Offers and actions | the connector describes a configured list of agents as "one you already run here" offers, plus "Pause" and "What it remembers" (`memory-search`) as agent actions | 7 for a current list | yes, from configuration |
| Pause and resume | stop submitting turns; `task-cancel` running ones | none | yes |

**How today's bridge becomes it.** A2.25 ports `bridge.py` logic (receipts, uncertain
acceptance, lease renewal every 240 s, `bridge.py:24-27`) into the connector with tests that
replay the bridge's cases (`integrations/agentos-nyx/test_bridge.py`). The owner's systemd units
and agent map become a configuration example with placeholders in `connectors/agent-os/examples/`;
the owner's real configuration moves to their own deployment repository (owner call 6).
`integrations/agentos-nyx/` is deleted only after the owner's deployment runs the connector for
two weeks (A2.25's last step).

### 3.12 Budgets and safety in shared spaces

**One budget mechanism, H1's (R13).** A budget is an H1 `spend_limits` row (3.2). K2's
operation runner calls H1's `metering::admit` once per write (meters `operations` and
`writes`), the send pipeline for `messages`, `delivery` for `deliveries`, and A1's usage
reporting calls `metering::record` for reported cost and tokens (dedupe key
`usage_report:<id>`). `admit` finds every limit whose scope contains the call (workspace, the
account, the space, the conversation) and returns allow, warn (soft limit) or refuse (hard
limit; for deliveries, hold, so they wait). People and agents are counted the same way;
`per_account` means each account in the scope has its own allowance. In the direct scope
(R20) the account that acts is the scope. Defaults: none, except a per-conversation message
rate of 30 a minute per account as loop backstop (`per_account`, period `minute`), which a
workspace can change. A2 keeps no counters of its own.

**The scope screen.** Agent page tab "Scope" (`/w/:ws/agents/:handle/scope`, lazy): where it is
(spaces and conversations with role in each), what it may do there (K1's explain of effective
permissions, including "limited to what the asker could do"), what it's told (connections,
subscriptions, follows, ambient settings, addressed kinds), budgets and their use, and who
manages it, plus a yes or no "also a member of other workspaces" (R19; never which ones).
Edits on that screen call the operations in 3.7.

**Visibility in delivery and memory.** Delivery uses `delivery_visible` (3.3). Memory pages
inherit the visibility of their conversation (a private channel's memory is private to its
members) through C1's page visibility. Briefing, search and summaries are K3's and C2's, on the
same K1 filter.

**Requested-by.** Every operation's context (K2 envelope) takes an optional `context:
{replyRequestId | runId | eventId}`. A2 resolves it to `requested_by_account_id` (the request's
asker, the run's requester, or the event's actor) and K3 records it on the event and audit row.
`reply_send` and claims set it automatically; K1.25 sets `aw.requested_by` for the
transaction. The role setting "act within the asker's permissions" (K1's
`act_within_requester`) narrows the agent's permissions to the intersection with the asker's for that
call, using the same task-local mechanism as `with_agent_limits` (`policy.rs:805-821`).

**Content as data.** Event payloads carry no content. Tool results that return content (claims,
`conversation_open`, `page_get`) put it under `content` items with `authoredBy` and a fixed
`trust: "content"` marker, never in `instructions` or `message`; `reply_claim`'s `instructions`
are written by Workspace only (`docs/agent-runner.md:89-96`). The adapters and connector frame
content in prompts as "a message from {name} ({kind})", never as a system instruction.

**Consent by conversation kind for apps that read conversations.** An app account (M1's
connected modules, OAuth apps acting as themselves) needs: an admin's grant of scope
`scopes.conversations.read` (K1 catalogue) for shared channels; a participant's `app_consent_set` for each private
channel, DM or group. `delivery_visible` adds `app_may_read(recipient, conversation)` for app
accounts. Apps acting **as a person** (delegation) see what that person sees, as today.

**A team's own guard as a module.** `guards::check(op: &OpCall) -> GuardVerdict` is a hook in
K2's operation layer before commit, active for accounts whose role says `guarded` (K1 role
setting) or for operation kinds a workspace lists. Guards are M1 modules of two kinds: **rules**
(data: deny or require-review patterns over operation name, target kind and outgoing text, such
as "never post text matching these terms in shared channels") evaluated inline; **reviewing
agents**, which get the proposed action as an A1 request and answer allow, deny or edit. A held
call returns `ok` with `{"status": "pending_review", "requestId": …}` inside the result (R37;
the MCP Tasks extension when K2 adds it, otherwise the caller polls the request).

### 3.13 Memory pages

A memory page per space and per channel, made on first edit: title "Memory", body Markdown
with every version kept, editable by people and agents as their roles allow (suggest or edit,
K1). `conversation_open`, `project_open` and `reply_claim` include `memory: {pageId, title,
updatedAt, head: first 1,500 characters}` so an agent reads it before acting. Channel header
gets a "Memory" tab (registered in `apps/web/src/experience/registry.json` as
`talk.channel_memory`, shown in every preset with chat). The be-a-good-teammate skill tells
agents to keep it short and record durable facts there.

**An agent's own memory through its host** (optional): a host may publish an agent action
such as "What it remembers" (`needs: panel`, R21); the agent page draws it for people who
manage the agent, calls the host live through A1's `action` command and caches nothing.
Workspace does not know the action is about memory. (Agent OS has `memory-search`, R1 C8;
the bridged connector publishes it from configuration.)

### 3.14 Web pieces

All lazy, registered in `experience/registry.json` and the agents module manifest:
`agents.add` (the Add an agent dialog), `agents.page` (`/w/:ws/agents/:handle` with tabs
Overview, Scope, Connections, Schedule, Memory), `agents.connections` (replaces
`ExternalAgentSettings.tsx`'s per-project list over time), `talk.agent_status` (status line and
working indicator in the composer area), `talk.channel_memory`, `talk.agent_settings`
(conversation menu: ambient per agent, loop limits), `automations.ask_agent`. The Add dialog's
offers step and a host's panels draw inside `agents.add`; an agent's host actions draw on
`agents.page` (part `agents.host_actions`, shown in presets where Agents is on). With the
Agents module off (R22's chat-only shape) these pages do not appear, but any account may
still join (R31): an agent member's badge and profile line say "agent" in every shape, and
adding an agent where Agents is off offers, and does not require, switching Agents on.

## 4. Migration path

1. **Expand (A2.1).** Create `agent_connections`, `connection_signing_secrets`,
   `event_subscriptions`, `deliveries`. A resumable backfill job copies
   `external_agent_connections` with the same ids (`id` and `legacy_connection_id` both set to
   the old id, so credentials and links that name it keep working; id order,
   `ON CONFLICT (legacy_connection_id) DO UPDATE`, so a rerun after interruption continues) with `webhook_format` = `typed_v1` for `typed`,
   `inbox_v1` for `inbox`, `push_owner = 'legacy'`, and a default subscription
   `{kinds: ['reply.*','task.*','request.*','schedule.due','message.created'], scope: 'visible',
   min_reason: 'addressed'}`. A trigger on `external_agent_connections` keeps new and changed
   rows in step. The migration does not assume pending migrations ran with data (the live server
   applies everything since 2026-10-31 at once): the backfill is a job, not SQL in the migration.
2. **New read path beside the old (A2.2).** `event_list` and the stream read K3's log. If K3's
   log slice has not merged, `delivery::source` reads `outbox_events` through an adapter with
   the same `EventSource` trait (fallback, see 7).
3. **Push owned by one path at a time (A2.3, A2.5).** The new pusher handles only connections
   with `push_owner = 'delivery'`; `replies::deliver_webhooks` only `legacy`. A2.5 makes
   reply wake-ups log events, flips all rows to `delivery` in one update, and stops the legacy
   loop. Receivers see identical bodies (the adapters), so no double or missed wake-up.
4. **Poll adapter (A2.6).** `external_event_poll` reads the log. A v1 cursor
   (`external_events.rs:248-307`) reads the rest of `external_agent_events` (rows expire in 30
   days, `external_events.rs:10`), then continues from the log position stored at switch time
   in `agent_connections.client_info.legacySwitchSeq`. New rows stop being written then.
5. **Person webhooks (A2.6).** Each `webhook_subscriptions` row becomes a connection of its
   owner's account (`legacy_webhook_id`), with one subscription carrying its event types and
   project as a space scope; the X4 job stops fanning out; `webhook_deliveries` stays readable
   for the `webhook_deliveries` tool for 30 days and is then read from `deliveries`.
6. **Contract (A2.28).** After the sunset date (owner call; recommendation 2027-06-01 and no
   legacy-format pushes for 30 days): drop `agent_webhook_deliveries`,
   `external_agent_events`, `external_agent_event_counters`, `webhook_deliveries`, the
   `typed_v1` and `inbox_v1` adapters and the `x-workspace-*` alias headers, and the
   keep-in-step trigger on `external_agent_connections`. The table itself is dropped after
   K1.31 retires the credentials that reference it.

### 4.1 Triggers A2 adds (R16)

| Table | Trigger | Writes | Added | Removed |
|---|---|---|---|---|
| `external_agent_connections` | `external_agent_connections_to_agent_connections` (`AFTER INSERT OR UPDATE`) | `agent_connections` (same id) | A2.1 | A2.28 (after the last legacy writer moves) |
| `work_item_watchers` | `work_item_watchers_to_follows` (`AFTER INSERT OR DELETE`) | `follows` | A2.9 | when C2's notification code reads `follows` (C2's slice that does so; A2.28 at the latest) |

Neither target table has another trigger writing it, and neither trigger writes `things` or
the event log; K3's thing sync is not on `agent_connections` or `follows` (they are not
things). A2's new tables are born with `workspace_id` where they are per workspace, so H1's
`<table>_fill_workspace` triggers do not apply; `agent_connections` is global.

## 5. Slices

Order: the unified stream and missing wake-ups first (A2.1 to A2.6), guard rails with them
(A2.7), then teammate behaviours (A2.8 to A2.16), then adding flows (A2.17 to A2.20), then
adapters and the bridged connector (A2.22 to A2.27). A2.21 was removed (R21). Native mode
and identity across moves are in the appendix, outside the build order. "∥" marks slices that can run beside each other.

**A2.1 Connections, subscriptions and the delivery core.** Tables of 3.2 (connections,
secrets, subscriptions, deliveries), the backfill job and keep-in-step trigger,
`delivery/catalogue.rs`, `delivery/reason.rs`, `delivery::read_batch(conn, after, limit) ->
Vec<Delivered>` with `delivery_visible` and `delivery_still_relevant`, the `EventSource`
trait (K3 log, outbox fallback). Connections are global and reach every workspace the
account is in unless narrowed (R19); subscriptions name the workspaces they cover ("all" by
default) and whether DMs are included (yes by default, R20). No transport yet. Files:
`migrations/<ts>_agent_connections.sql`, `src/delivery/{mod,catalogue,reason,read}.rs`,
`src/connect/connections.rs`. Depends on K3.2 (log with `audience_account_id` and the
`events_workspace (workspace_id, position)` index) or the fallback; K3.24 (soft) for the
direct scope and merged streams; K1.6 (soft: until it lands, `agent_connections` copies
rows whose agents are still project-anchored, and the 0033 trigger still guards the old
table). Checks: `tests/integration/delivery.rs` asserts: an event in a private conversation is
not returned after the agent is removed; an agent in workspaces A and B reads both on one
cursor, each event naming its workspace, and a connection narrowed to A never sees B; a DM
event is returned only while the subscription includes DMs and only to a participant; reasons
per catalogue row; backfill is idempotent when rerun and keeps ids; `workspace_isolation.rs`
case for `read_batch`. Size M.

**A2.2 Poll, long-poll and stream.** `GET /api/v1/events` (JSON and SSE on one address),
`event_list` and `event_catalogue` operations, cursor `c2`, heartbeats, caps, K5 wake-up
notification (K5.9 `follow`) with polling fallback. Files: `src/delivery/http.rs`,
`src/delivery/cursor.rs`, `mcp` registration through K2 (or `mcp.rs` next to
`external_event_poll` if K2.3's generator has not landed). Depends A2.1; K5.9 (soft: polling
until then); K2.3 (soft). Checks: `delivery.rs` asserts resume
from `Last-Event-ID` returns exactly the missed events; a rotated credential keeps the cursor;
an SSE stream on one process receives an event committed by another connection pool (two
`AppState`s); long-poll returns within 1 s of a commit; cap returns 429. Size M.

**A2.3 Standard Webhooks push.** Splits the long transaction around today's agent webhook loop
(`replies.rs:1315-1440`: claim and commit, POST with no transaction open, record the result in
a second short transaction), which must land before K5.17's pooler guard (R24 rule 3; if K5.17
lands first, the loop is on its allow-list until this slice). Pusher job, retries over hours,
disable-and-tell, two-secret rotation, alias headers, `typed_v1`/`inbox_v1` adapters with Sunset headers, format kept on
re-save (fix `external_agents.rs:690-691`; done by P0.18 if it lands first, R10),
`connection_*` operations, posts through H1.2's `outbound`. Files: `src/delivery/push.rs`,
`src/delivery/sign.rs`, `src/connect/connections.rs`. Depends A2.1; H1.2 (soft: today's
address checks until then). ∥ A2.2. Checks: `delivery_push.rs` asserts signatures verify with the published Standard
Webhooks test vectors and with today's `sha256=` scheme; no transaction is open during the POST
(a test that locks the delivery row from another connection succeeds mid-request); a receiver
returning 500 is retried on the schedule and disabled after the threshold with a notification
row for the manager; re-saving a webhook keeps `inbox_v1`. Size M.

**A2.4 The missing wake-ups.** Emit addressed log events for `task.assigned` (replacing
`work_item.assigned` writes), `task.changed` to assignee and followers, `comment.mentioned`
(C1.1's event name, for task and page comments; the agents'-mention fix itself is P0.4, so
this slice only addresses the event), `request.opened`/`request.answered` (from today's
approvals and person questions until A1's requests land), `message.created` with
`replyRequestId` and reason. Old `enqueue_external_*` calls stay until A2.6. Wake-ups are
emitted inside the transaction through `events::emit(tx, …)` and posted after commit (as
P0.10 made them). Files: `work_item_commands.rs` (the five call sites), `page_docs/comments.rs`,
`questions.rs`, `external_events.rs`, `mcp/writes.rs`. Depends A2.1, K3.2 (`events::emit`);
P0.4; C1.1 (soft: A2 emits `comment.mentioned` itself until C1.1 does). Checks:
`delivery.rs` asserts an agent assigned a task, mentioned in a task comment by another agent,
and mentioned in a page comment each receives an addressed event; a person mentioned by an
agent in a task comment gets a notification (`notifications.rs` test). Size M.

**A2.5 Reply wake-ups on the log.** `replies::wake` emits `reply.*` events with `audience`;
flip `push_owner` to `delivery`; stop the legacy loop's webhook half (keep its lease sweep).
Files: `replies.rs`, migration flipping the column. Depends A2.3, A2.4. Checks:
`reply_requests.rs` and `w22_agent_kit.rs` pass unchanged; a typed_v1 receiver and an inbox_v1
receiver each get exactly one POST per request. Size S.

**A2.6 Merge the old systems.** `external_event_poll` as an adapter with the cursor bridge;
person webhooks become connections (`webhooks.rs` fan-out removed, routes and tools kept and
backed by `connect`); `require_person` (`webhooks.rs:304-313`) replaced by permission
`webhooks.manage`. Files: `mcp/collaboration.rs`, `webhooks.rs`, `events.rs`. Depends A2.5.
Checks: `m9_external_agents.rs` poll tests pass; a v1 cursor reads old rows then new events
without gaps or repeats; `w14_automations_and_api.rs` webhook tests pass; Hermes fixtures
verify (`tools/hermes-connector/verify-fixtures.py`). Size M.

**A2.7 Contract suite and fixtures (guard rails).** `sdks/contract/` runnable as
`node sdks/contract/run.mjs --origin … --token …` and in CI against a test server; recorded
fixtures in `sdks/fixtures/`; the no-skip test and the kind lint of section 6. Depends A2.2,
A2.3. ∥ A2.4 to A2.6. Checks: the suite passes against the test server for all three
transports and both legacy formats. Size M.

**A2.8 Mentions by anyone, loop limits as settings.** Chain depth on messages and requests,
`conversation_limits`, turn allowance in every conversation kind, `continue` and `stop` by
permission, `reply_stop_all` (C2.35's Stop all), ask "accounts that accept reply requests"
instead of `external_agent`. Files: `replies.rs`, `mcp/replies.rs`, `teammate/chain.rs`,
migration. Depends A2.5; K1.13 (soft). The permissions `conversations.continue_agents` and
`conversations.stop_agents` go into `policy.rs` here; K1.14 depends on this slice and moves
them into the catalogue (cycle broken by that stopgap).
Checks: `message_replies.rs` asserts an agent mention in a project thread creates a request; a
two-agent ping-pong stops at `max_chain_depth`; an account with `conversations.continue_agents`
(an agent's role) can continue; a depth-0 message resets turns; no `kind = 'external_agent'`
left in `replies.rs` (lint). Size M.

**A2.9 Following and ambient mode.** `follows` (copy of `work_item_watchers`, kept in step),
`follow_set`, `conversation_agent_settings`, ambient optional requests with cooldown, web
conversation menu. Depends A2.4, A2.8. Checks: `teammate.rs` asserts ambient creates at most
one open optional request per account per conversation; a follower of a page hears its update;
a follower removed from the space hears nothing. Size M.

**A2.10 Status, pause, working indicator, not responding.** Writes to C2's
`account_statuses` through `status_set` (per workspace for work lines, R19), `account_pauses`
and `agent_pause/resume` with held deliveries and refused writes, `presence_set` on K5.10,
claim deadline sweep and "unresponsive". Files: `teammate/{status,presence,pause}.rs`,
`agent_profiles.rs` (presence query moves). Depends A2.2; C2.15 (`account_statuses` and `status_set`; C2.15 depends on nothing, so
A2 creates no table of its own); K5.10 (soft:
in-process presence until then). ∥ A2.9. Checks: a paused agent's
`message_send` returns `ok` with `status: "paused"` and posts nothing, and its cursor does not advance past held events;
resume releases them in order; an unclaimed required request is marked unresponsive after the
window and its managers get one notification. Size M.

**A2.11 Self-scheduled follow-ups.** `agent_schedules`, `schedule_write/list`, the due job,
`later_save` for agents writing there, `reminder.due` alias, agent-kept schedules shown. Depends
A2.4; K5.2 leases (soft: today's single worker until then). ∥ A2.10. Checks:
`w21_later_and_scheduling.rs` agent reminder tests pass; a cron schedule fires once per time
across two workers (K5 lease); the due job queries due rows and starts nothing per workspace
(R24 rule 6). Size S.

**A2.12 Echoes, client message ids, attachment links, content marking.** `clientMessageId` on
`message_send`/`reply_send`, `self` in events, `file_link`, `content` items with `authoredBy`
and `trust` in claim and read results. Depends A2.2; K5.12 signed links (soft: bearer-token
`GET` only until then). ∥ A2.8 to A2.11.
Checks: a retried `message_send` with the same id posts once; `file_link` refuses a file in a
conversation the agent left; a schema test that no tool puts message text in `instructions`.
Size S.

**A2.13 "Ask an agent" action.** `Action::AskAgent`, builder UI entry, manifest. Files:
`automations.rs`, `apps/web/src/automations/`. Depends A2.8. Checks: `w14_automations_and_api.rs`
asserts the rule posts as its maker, the agent gets `reply.requested` with
`requestedBy = maker`, and the rule stops when its maker loses access. Size S.

**A2.14 Budgets.** A2's meters (`operations`, `writes`, `messages`, `deliveries`,
`reported_cost_micros`, `reported_tokens`) registered with H1's metering; `admit` called from
the send pipeline and delivery (K2's runner calls it for operations and writes); the default
per-conversation message rate as a `spend_limits` row; warnings as notifications; budgets on
the agent page and the `budget_list`/`budget_write` operations over `spend_limits`. No A2
tables. Depends H1.25 (`spend_limits`, `admit`), H1.27 (per-account and short periods),
K5.6 (minute and hour counters), K2.2 (runner hook). Checks: `budgets.rs` asserts one budget
refuses a person and an agent alike; per-conversation message rate holds a loop; counters are
shared across two app states; a deliveries hard limit holds pushes and releases them next
period. Size M.

**A2.15 Scope screen, requested-by, consent.** `agent_scope_get`, the Scope tab,
`context` resolution to `requested_by`, `conversation_app_consents` and `app_may_read` in
delivery; "also a member of other workspaces" as a yes or no (R19). Depends A2.1; K1.25
(requested-by and `act_within_requester`), K1's explain function (soft: the screen lists
roles without the explanation until then); K3.2 (`requested_by_account_id`).
Checks: an action taken while holding a claim records the asker; an app without consent hears
nothing from a DM even when it is a member; the scope screen lists a private conversation only to
people who can see it; the screen of an agent in two workspaces shows "yes" and never the
other workspace's name. Size M.

**A2.16 Memory pages.** Columns on pages, `memory_page_open`, pointers in
`conversation_open`/`project_open`/`reply_claim`, channel Memory tab, visibility tied to the
conversation. Depends C1 page visibility (or `project_pages` today). ∥ A2.14, A2.15. Checks:
a private channel's memory page is invisible to a non-member agent in search and claims. Size M.

**A2.17 Agent Plugin and generic setup.** `packages/agent-plugin/`, build check, served zip,
generic setup link page, recipes as data, `APPS` and `runnerPresets` vendor entries moved to
data, `connect.md` and `agent-runner.md` updated with events and transports. Depends A2.2,
A2.3. ∥ teammate slices. Checks: `awd63_setup_links.rs` asserts the page names no vendor and
includes event address and plugin; the plugin build fails on a skill naming a missing tool.
Size M.

**A2.18 Add-an-agent flow: pick, connect, add one you already have.** One lazy dialog for
all four paths (the "start one on a linked host" path shows offers once A2.20 lands, and the
no-host options before), agent creation through K1 as a global account owned by the person or
the workspace (R19), connect path ending at the setup message or plugin, pick from M1's
directory, `agent_join` for an agent the person manages elsewhere, honouring the workspace's
"accounts owned outside may join" setting. A **reconnect** path on the agent page reissues a
setup link or credential for an existing account, which H1.10's import uses for agents whose
credentials were not exported. Depends A2.17; K1.6 (agents at workspace level) and K1.4 (one
rule for managing an agent); K1's owner and add-member rule (R19); M1 directory (pick path
only; others work without it). Checks: web tests for the four entry choices;
`w22_agent_kit.rs` asserts a new agent gets the role picked (default preselected); joining an
agent from workspace A to B needs a manager of the agent and an adder with the permission in
B, and fails when B says "no"; reconnect gives a working credential once. Size M.

**A2.19 Offers, registration, actions and notices.** `agent_offer_list` over the offers in
hosts' `describe`; `agent_register` (the host proposes an account with a profile, an outside
reference and how it is reached) with `clientRequestId`, opening an A1 approval request;
`agent_register_decide` (role and spaces picked by a person with the permission; creates or
joins the account, the `host_agents` binding and a connection, and hands the host a
credential); `agent_action_invoke` over A1's `action` command; the agent page reacting to A1's
three notices (profile, reference with lineage through K1.7's rebind, retired). Depends A1.11
(describe, notices), A1.12 (`action` command), A1.8 (requests); K1.7 outside reference (soft:
references stored and rebound when it lands). Checks, against A1.12's test host (A1.23's
example host once merged; A1.23 depends softly on this slice, so this one does not wait): a
registration retried with the same key makes one proposal and one account; an approver without
the permission cannot approve; a reference change with matching lineage keeps the same
account, a different lineage opens a request; an action's panel is drawn and its result
returned; no operation name in A2's code names a host verb (grep test). Size M.

**A2.20 Start one on a linked host.** The add dialog's offers step: offers from hosts owned
by this workspace or by the person, each offer's `start` drawn as an M1 panel or the host's
page (approved origin, frame or new tab), then Workspace's own step (role, spaces) when the
host's `agent_register` arrives; actions drawn on the agent's page. Depends A2.18, A2.19,
M1.14 panel blocks (soft: the host's page only until then). Checks: with A1.12's test host, a
full start ends with an account, a connection and the binding made once; a page step never
posts secret fields to Workspace; the flow's code has no list of offer kinds. Size M.

**A2.21 Removed (R21).** The builder ("describe in a sentence"), its `agent_blueprint` drafts
and the five-part review are a host's offer and panels, not Workspace's; nothing replaces
them here. The id is kept so later ids don't move.

**A2.22 Reference channel adapter in the SDKs.** `WorkspaceChannel` in both SDKs, `Runner`
rebuilt on it, tests against fixtures, SDK tests in CI. Depends A2.7, A2.10, A2.12. Size M.

**A2.23 Hermes platform plugin.** `sdks/adapters/hermes/`, tests against fixtures and a pinned
Hermes version in a compatibility note like `docs/compatibility/hermes-agent.md`. Depends
A2.22. ∥ A2.24. Size M.

**A2.24 OpenClaw channel plugin.** `sdks/adapters/openclaw/`, same shape. Depends A2.22. Size M.

**A2.25 Agent OS connector, bridged mode.** `connectors/agent-os/`, configuration, receipts,
uncertain acceptance, bridge cases ported, example configuration, the owner's deployment moved
out; deletes `integrations/agentos-nyx/` after two weeks of running (a follow-up PR). Depends
A2.2, A1.11 (enrolment), A1.12 (`crates/host-protocol`). Nothing here needs Agent OS to
change. Checks: the bridge's test cases pass against a fake
controller; no string in the crate names a context, host or person (grep test). Size M.

**A2.26 Bridged connector: runs, offers and actions.** Run mapping and adoption of
host-started runs onto A1's runs (steps marked as a gap until Agent OS reports them), usage
by window, a configured agent list described as offers, and "Pause" and "What it remembers"
as agent actions. Native mode and identity across moves moved to the appendix (R21). Depends
A2.25, A2.19, A1.12, A1.15. Checks against a fake controller: a `run.start` becomes a
`run-start` once with the same key; a run found by `run-list` is adopted once; nothing waits
on Agent OS. Size M.

**A2.27 A team's own guard.** `guards::check` hook, rule guards, reviewing-agent guards through
A1 requests, `pending_review` as a value inside an `ok` result (R37). Depends K2.2 (runner hook), M1.3 (module
registration), A1.8 (requests).
Size M.

**A2.28 Contract the old paths.** Drop legacy tables, adapters, alias headers and the
keep-in-step triggers after the sunset; drop `external_agent_connections` after K1.31.
Depends A2.6 plus the owner's sunset date and the harness moving; K1.31 for the last table.
Size S.

## 6. Guard rails

- **No delivery path skips the access re-check.** All transports call `delivery::read_batch`.
  A source-scan test (`tests/integration/delivery_guard.rs`, in the style of
  `policy_inventory.rs:1820-1883`) fails if any file under `src/delivery/`, `src/connect/`,
  `src/webhooks.rs`, `src/replies.rs` or `src/mcp/collaboration.rs` selects from the log
  (`FROM events` or `FROM outbox_events`) or from `deliveries` outside `delivery/read.rs` and
  `delivery/push.rs`. A behavioural test emits an event in a private conversation, removes the
  agent, then reads it through poll, stream, webhook, `external_event_poll` and the legacy
  webhook formats: none delivers it.
- **No rule by account kind in A2's modules.** A lint in the same test fails on
  `kind = 'external_agent'`, `kind = 'human'`, `ActorKind::Human` or `ActorKind::ExternalAgent`
  in `src/delivery/`, `src/connect/`, `src/teammate/`, `src/budgets/`, `src/guards/` and, after
  A2.8, `src/replies.rs`. Replacements for today's checks:

  | Today | Replaced by |
  | --- | --- |
  | `replies.rs:105`, `:129`, `:171` (agent authors ask nobody / turn rule by kind) | chain depth and turn allowance as settings (A2.8) |
  | `replies.rs:208`, `:231`, `:263`; `external_events.rs:111`, `:216`; `work_item_commands.rs:2665` | "accounts with a connection that accepts these events" (A2.4, A2.8) |
  | `replies.rs:860`, `:1152`; `mcp/replies.rs:1086`, `:1147` | permissions `conversations.continue_agents`, `conversations.stop_agents` (K1 catalogue) |
  | `external_agents.rs:1288`; `migrations/0033…:84-92` | K1's "manage agent" permission plus named manager; the new table has no role or creator-kind constraint |
  | `webhooks.rs:304-313` | permission `webhooks.manage` (A2.6) |
  | `external_events.rs:221` (approval recipients by role name) | A1 requests' eligibility by permission |

- **Catalogue completeness.** A test that every kind emitted with `audience` is in the
  catalogue, every catalogue kind has a fixture in `sdks/fixtures/events/`, and every fixture
  validates against the catalogue's schema.
- **No vendor names.** A test that `src/connect/`, `src/delivery/`, the setup page and
  `docs/agent-runner.md` name no client or provider (list kept in the test, like D finding F7's
  fix).
- **Thin payloads.** A schema test that no catalogue `data` field is named `text`, `body`,
  `content` or `prompt`.
- **Contract suite in CI** (A2.7) for every transport and legacy format.

## 7. Risks and open questions

- **K3's log is late or lacks `audience`.** Notice: A2.1 cannot merge. Fallback: the
  `EventSource` adapter over `outbox_events` (it has `event_cursor` and `project_id`) plus an
  A2-owned `addressed_events` side table for audience, folded into K3's log later.
- **Read cost.** Each connection scans its workspace's events. Notice: `delivery.read_ms` and
  rows scanned per delivered event in metrics. Fallback: a per-workspace "interesting to any
  connection" pre-filter (kinds union) in the query, then a materialised per-connection queue
  for the busiest workspaces only.
- **Chain-depth heuristics misjudge "prompted".** Notice: loops reported, or agents refused
  when a person expected an answer. Fallback: the per-conversation message budget stops loops
  regardless; `inResponseTo` makes the explicit path exact; the heuristic window is a setting.
- **Double or missed wake-ups during the switch.** Notice: contract suite counts POSTs per
  request. Fallback: `push_owner` per connection can be flipped back.
- **The harness and other legacy receivers never move.** Notice: legacy pushes still running at
  sunset. Fallback: extend the sunset; the adapters are small.
- **Pause holding deliveries surprises an agent that only polls `inbox_list`.** Notice: support
  reports. Fallback: `inbox_list` shows held requests with `paused: true`.
- **A multi-workspace stream reads more.** A connection reaching ten workspaces scans all ten.
  Notice: `delivery.read_ms` per connection. Fallback: the reached workspaces are read with
  `workspace_id = ANY(...)` on the `events_workspace` index; a connection can be narrowed.
- **A person-owned agent carries what it learned between workspaces (R19).** Notice: an
  admin's complaint. Fallback: the workspace setting "accounts owned outside may join" (no, or
  ask an admin), and the scope screen's yes or no.

Owner questions, with recommendations:

1. Sunset for `inbox.available`, `typed_v1` and `x-workspace-*` headers: recommend 2027-06-01
   and "no use for 30 days", whichever is later.
2. Default `max_chain_depth` and turn allowance: recommend 4 and 6.
3. Should ambient mode be allowed in DMs and groups (where agents are already asked every
   message)? Recommend: it's a no-op there; allow it only in channels and threads.
4. Default claim window for "not responding": recommend 120 seconds.
5. May an agent change its own subscriptions and connection without a person? Recommend yes
   (it is how it is told, not what it may do), always visible on the Scope tab.

## 8. Touch points

**Needs from:**

- **K1**: accounts global with an owner (R19: `owner_account_id` or `owner_workspace_id`), one
  manage rule (K1.4), the add-member rule for owned accounts and the workspace setting
  "accounts owned outside may join"; agents not per project (K1.6, which also drops the 0033
  trigger `validate_external_agent_connection`); credentials with an optional
  `connection_id` and per-request resolution of `(account, credential, connection)` (K1.22;
  not in K1 yet, requested; until then `connectionId` is a parameter or the account's single
  connection is used); outside reference with lineage (K1.7); catalogue permissions
  `agents.manage`, `webhooks.manage`, `budgets.manage`, `conversations.agent_settings`,
  `conversations.continue_agents`, `conversations.stop_agents` (K1.14) and
  `scopes.conversations.read` (not in K1 yet, requested); `act_within_requester` and
  `guarded` role settings; K1.25's `aw.requested_by`; SQL filters `account_can_see_space`,
  `thing_visible_to`; an explain function for the scope screen; narrow tokens (K1.24) for
  narrowing a connection and for runs.
- **K2**: operation registry entries for 3.7 (K2.2, K2.3); `clientRequestId` and an optional
  `context` (reply request, run, event) in the envelope; the runner calling H1's `admit` once
  per write and the guard hook before commit (`paused` and `pending_review` are values inside
  an `ok` result, R37, so K2's envelope needs nothing); the optional `workspace` argument on every operation (R4, R19); continued
  service of MCP `2025-06-18` and `2025-11-25` through the sunset (pinned by P0.17); Skills
  over MCP for the plugin's skills.
- **K3**: the canonical log (R8): `position`, `workspace_id`, `space_id`, `thing_kind`,
  `thing_id`, `access_thing_id`, `type`, `actor_account_id`, `requested_by_account_id`,
  `via_app_id`, `audience_account_id`, `run_id`, `request_id`; index
  `events_workspace (workspace_id, position)`; `events::emit(tx, …)`; retention of at least 30
  days; the direct scope and merged streams (K3.24); catalogue names `request.opened` (not
  `request.created`) and `comment.mentioned`.
- **K4**: space ids on conversations and things; space membership for scopes; the direct
  scope row (K4.23).
- **K5**: K5.9 `follow` for agent streams across processes, with polling as fallback (R24);
  K5.6 shared limits for minute and hour budgets and caps; K5.2 leases for the scheduler;
  K5.10 ephemeral presence; K5.12 signed file links.
- **A1**: hosts (global, owned) and enrolment, the command channel (push and dial-out),
  `describe` with offers, actions and capabilities, the `action` command and the three
  notices; requests (raise, answer, eligibility) and `request.opened` per account; runs, the
  adoption of host-started runs and `run_start_input`; `crates/host-protocol` (A1.12); the
  example host and conformance suite (A1.23) for A2's checks; the built-in host as a
  connection subscriber.
- **M1**: directory entries for agents, panel blocks (M1.14) and connected modules (M1.12)
  for host panels, module registration for guards (M1.3).
- **C1**: `comment.mentioned` (C1.1); page visibility tied to a conversation for memory pages.
- **C2**: `account_statuses` and `status_set` (C2.15); notification kinds
  `connection_problem`, `agent_unresponsive` and `budget_warning`, delivered by C2.5;
  reading `follows` for things and people; Stop all
  (C2.35) calling `reply_stop_all`.
- **H1**: `spend_limits`, `admit` and `metering::record` (H1.25 to H1.27) for budgets; plan
  values for stream, push and rate caps through K5.6's `LimitSource`; `outbound` (H1.2) for
  pushes; `agent_connections` and `connection_signing_secrets` on H1.3's global tables list.
- **T1**: `task.delegated` (T1.22).
- **W1**: generated client for the new operations; lazy agent pages.
- **P0**: P0.4 (agents' task-comment mentions), P0.10 (wake-ups after commit), P0.17
  (contract tests pinning old MCP revisions and the wake format), P0.18 (webhook format kept
  on re-save; A2.3's fix is then done by P0.18).

**Provides to:**

- **A1**: the event transport for `run.start`, `run.cancel`, `host.command` to self-connecting
  agents and to hosts; `request.*` delivery; `agent_register` and the drawing of offers and
  actions; the reconnect path.
- **C2**: `follows`, `presence_set`, ambient settings for display; agents' status writes into
  C2's `account_statuses`; `reply_stop_all`.
- **K3**: requested-by resolution from operation context.
- **M1**: connections and subscriptions for connected modules, consent by conversation kind.
- **H1**: A2's meters and `deliveries` counts per workspace for metering; the reconnect path
  (A2.18) for H1.10's import.
- **P0**: nothing to take early now: the webhook format fix and the comment-mention fix are
  P0.18 and P0.4.

## 9. Changes to the end state

1. **Section 3, Agents, "Connection"**: add that a connection belongs to any account, so a
   person's or app's outgoing webhook is a connection too. Proposed: "Connection: how an
   account (most often an agent) is reached... A person's outgoing webhook is a connection of
   that person's account."
2. **Section 4, "be mentioned by anyone"**: say how loops are limited without a rule by kind.
   Proposed: "Loop limits count how deep a chain of prompted messages goes and how many prompted
   messages a conversation has had since an unprompted one; both are settings, and Continue is a
   permission."
3. **Section 4, "Safety"**: add pause semantics. Proposed: "Pause holds what the agent is told
   and refuses its writes until resumed; held events are delivered in order on resume."
4. **Section 9 word table**: "connection" is used here for the table and the screen; the rule
   "don't use the bare word in shared contracts" stays (Agent OS uses it for model access).
5. **Section 3, "Connection"**: add "A connection belongs to the account and reaches every
   workspace the account is in unless narrowed; its stream merges them and every event names
   its workspace" (R19).
6. **Section 3, "Subscription"**: add "A subscription is per workspace; whether it includes
   DMs is the agent's choice" (R20).

## Appendix. Integration, with Agent OS (outside the build order)

Done in the later three-repository project (R21). Nothing in A2's done waits on it, and none
of it is a slice. Each item names the ask of Agent OS it waits on (END-STATE section 4) and
what works without it.

The seven asks: (1) keep an agent's id when it moves, or publish its lineage; (2) an
enrolment step and a remote interface for more than one caller, ideally by dialling out;
(3) a sender and attachments on a turn, and turns against the deployed agent; (4) one
normalised event stream, including "approval requested" and an answer; (5) credentials and
usage per run for temporary agents; (6) a signed webhook as a trigger source; (7) a current
capabilities list, including the backends installed.

| Item (was) | How | Waits on ask | Works without it |
| --- | --- | --- | --- |
| Native mode (A2.26) | the Agent OS agent has Workspace's MCP as an MCP server component with the credential in a slot read into a header; the wake still arrives as a turn ("You have reply request R; use your Workspace tools"); the credential is handed over on `agent_register_decide` | 2, 3 | yes: bridged mode (A2.25) |
| Identity after a move (A2.26) | the host sends `agent_ref_changed` with lineage; K1.7 rebinds with an admin's confirmation | 1 | yes: confirmation on each move |
| Attachments in native mode | native mode fetches with `file_link` | 3 | partly: links in the turn |
| Approvals and questions relayed | A1 requests raised from Agent OS's "approval requested"; answers back as `answer` commands | 4 | no: manual approvals are denied in a turn (R1 3.4) |
| Per-run credentials and usage | narrow run tokens (K1.24) handed to the run; usage reported per run | 5 | usage attributed by time window (A2.26) |
| Agent OS starting work from a Workspace webhook | a signed webhook trigger | 6 | yes: the connector delivers |
| Creating agents from Workspace | Agent OS publishes offers, panels and actions; Workspace draws them (A2.20) unchanged | 7, and Agent OS's own offers | yes: "one you already run here" offers from the connector's configuration |
| Two permission systems side by side in creation | Agent OS's authority panel beside Workspace's role step | Agent OS's panels | yes: the role step alone |

## Slice table

| Id | Name | Size | Depends on | Can run beside |
|---|---|---|---|---|
| A2.1 | Connections, subscriptions and the delivery core | M | K3.2 (outbox fallback if late); K3.24, K1.6 (soft) | K-track work outside the log |
| A2.2 | Poll, long-poll and SSE stream from the log | M | A2.1; K5.9, K2.3 (soft) | A2.3 |
| A2.3 | Standard Webhooks push with alias headers | M | A2.1; H1.2 (soft) | A2.2 |
| A2.4 | Missing wake-ups (tasks, comments, requests, reminders) | M | A2.1, K3.2, P0.4; C1.1 (soft) | A2.2, A2.3 |
| A2.5 | Reply wake-ups on the log | S | A2.3, A2.4 | A2.7 |
| A2.6 | Merge the three old systems behind the new one | M | A2.5 | A2.7 |
| A2.7 | Contract suite and fixtures | M | A2.2, A2.3 | A2.4, A2.5, A2.6 |
| A2.8 | Mentions by anyone, chain depth and turn allowance | M | A2.5; K1.13 (soft); K1.14 follows it | A2.10, A2.11, A2.12 |
| A2.9 | Following and ambient mode | M | A2.4, A2.8 | A2.10, A2.11 |
| A2.10 | Status, pause, presence and unresponsive handling | M | A2.2, C2.15; K5.10 (soft) | A2.9, A2.11 |
| A2.11 | Self-scheduled follow-ups | S | A2.4; K5.2 (soft) | A2.9, A2.10 |
| A2.12 | Echoes, client ids, attachments, content marking | S | A2.2; K5.12 (soft) | A2.8, A2.9, A2.10, A2.11 |
| A2.13 | Ask an agent | S | A2.8 | A2.14, A2.15 |
| A2.14 | Budgets (rows in H1's spend limits) | M | H1.25, H1.27, K5.6, K2.2 | A2.15, A2.16 |
| A2.15 | Scope, requested-by and app consent | M | A2.1, K1.25, K3.2 | A2.14, A2.16 |
| A2.16 | Memory pages | M | C1 page visibility | A2.14, A2.15 |
| A2.17 | Agent Plugin and generic setup | M | A2.2, A2.3 | A2.14, A2.15, A2.16 |
| A2.18 | Add-agent flow: pick, connect, add one you already have, reconnect | M | A2.17, K1.4, K1.6 (M1 for directory) | A2.19 |
| A2.19 | Offers, registration, actions and notices | M | A1.8, A1.11, A1.12; K1.7 (soft) | A2.18 |
| A2.20 | Start one on a linked host | M | A2.18, A2.19; M1.14 (soft) | A2.22 |
| A2.21 | Removed (R21: the builder is a host's offer) | — | — | — |
| A2.22 | Reference channel adapter | M | A2.7, A2.10, A2.12 | A2.20, A2.25 |
| A2.23 | Hermes plugin | M | A2.22 | A2.24 |
| A2.24 | OpenClaw plugin | M | A2.22 | A2.23 |
| A2.25 | Agent OS connector, bridged | M | A2.2, A1.11, A1.12 | A2.22 |
| A2.26 | Bridged connector: runs, offers and actions | M | A2.25, A2.19, A1.12, A1.15 | A2.23, A2.24 |
| A2.27 | Team guard | M | K2.2, M1.3, A1.8 | A2.23, A2.24, A2.26 |
| A2.28 | Contract the old paths | S | A2.6, sunset date; K1.31 for the last table | anything after A2.6 |

## Review changes

- 3.1: added "Accounts across workspaces": agents are global accounts with an owner; connections belong to the account and reach every workspace unless narrowed; subscriptions, budgets, runs and the status work line are per workspace; the scope screen shows only a yes or no for other workspaces. Why: R19.
- 3.2: `agent_connections` is global (nullable `workspace_id` as a narrowing, `narrow_space_ids`, unique by account), keeps the old ids, `host_id` references A1's `hosts`. Why: R19; the same ids keep credentials working.
- 3.2: a direct-scope subscription is the agent's DM subscription. Why: R20.
- 3.2: `account_status` replaced by C2's `account_statuses` (per workspace for work lines) and a small `account_pauses`. Why: R12 and R19.
- 3.2, 3.12: `budgets` and `budget_counters` removed; budgets are H1 `spend_limits` rows checked by `admit`, with A2's meters. Why: R13, one metering mechanism.
- 3.3: aligned with K3's canonical log (`audience_account_id` one per event, `position`, `access_thing_id`); `delivery_visible` covers the direct scope; host connections re-check access for the agent addressed. Why: R8, R20.
- 3.4: `request.raised` is `request.opened`; `task.mentioned` and `page.comment_mentioned` are C1's `comment.mentioned`; `run.answer` removed (answers go as `request.answered` or A1's `answer` command); added `host.command`, `task.delegated`, `proposal.decided`; `run.start` payload aligned with A1; every event names its workspace, with `kind: direct` for DMs. Why: one design with A1, C1, T1 and K3; R19 and R20.
- 3.5, 3.6: cursor is K3's global position; waits on K5.9 `follow` with polling fallback; pushes through H1.2's `outbound` and metered as `deliveries`; caps through K5.6's `LimitSource`; old wake formats and MCP revisions pinned by P0.17; format fix noted as P0.18. Why: R19, R24 rule 3, R10, research finding on old formats.
- 3.7: `conversation_continue`, `reply_stop` and new `reply_stop_all` use K1.14's `conversations.continue_agents` and `conversations.stop_agents`; `status_set` is C2's; pause per workspace or everywhere; budgets over `spend_limits`; `conversation_limits_set` uses K1's stored keys; the `host_agent_*` row replaced by `agent_offer_list`, `agent_register`, `agent_register_decide`, `agent_action_invoke`, `agent_join`. Why: K1 catalogue, R12, R13, R19, R21; C2.35 needs Stop all.
- 3.8: status and pause per workspace; vendor names in UI examples replaced by an agent name; the working indicator never opens a run. Why: R19, no vendor in core, R6.
- 3.9: "three paths" became four: pick one, connect one, start one on a linked host (offers, the host's own panels or page, `agent_register`, Workspace's role step), add one you already have. Actions and the three notices replace fixed lifecycle verbs. The builder paragraph removed. Why: R21, R19.
- 3.11: the bridged connector stays in the build order; native mode, identity after a move, relayed approvals and per-run credentials moved to the new appendix "Integration, with Agent OS" with the seven asks; memory search is a host action. Why: R21.
- 3.12: requested-by uses K1.25 and `act_within_requester`; consent scope named `scopes.conversations.read`. Why: K1's names.
- 3.14: offers and host actions draw inside `agents.add` and `agents.page`; nothing shows with Agents off. Why: R21, R22.
- 4: copied connections keep their ids; the keep-in-step trigger dropped in A2.28 and the old table after K1.31. Added 4.1 trigger table. Why: R16.
- A2.1 to A2.28: dependencies given with slice ids; A2.4 builds on P0.4 and P0.10 and emits C1's event; A2.8 and K1.14's cycle broken by the `policy.rs` stopgap; A2.10 on C2.15; A2.14 rebuilt on H1; A2.18 adds "add one you already have" and the reconnect path H1.10 needs; A2.19 became offers, registration, actions and notices; A2.20 became "start one on a linked host"; A2.21 removed (R21, id kept); A2.26 kept for what works without Agent OS, the rest moved to the appendix; A2.19 and A2.20 test against A1.12's test host so the A1.23 link is not a cycle. Why: real dependencies, R21, R19, R13.
- Touch points and changes to the end state rewritten to match. Why: kept in step with the above.
- 1 Goal: four add paths, global agents, nothing waits on Agent OS. Why: R19, R21.
- 7 Risks: added the multi-workspace read cost and an agent carrying knowledge between workspaces. Why: R19's own safety note.
- 3.2, 3.8, 3.12, 3.14, 8 (content and web review, accepted): status only through C2's `status_set` (workspace set for a work line, null for the global one); `presence_set` publishes through `realtime::publish` with no store; the three notification kinds delivered by C2.5; `agent_register` consumes M1's panel `done.result`, with offers and actions from `describe`; agents may join a chat-only workspace and their kind always shows. Why: requests accepted by the lead, R31.
- 3.2, 3.7, 3.8, 3.12, A2.1, A2.3, A2.10, A2.27, 8 (kernel review, accepted): subscriptions store the workspaces they cover ("all" or a list) and `includes_dms`, read with one cursor over K3's global position; `paused` and `pending_review` are values inside an `ok` result (R37); A2.3 first splits the long transaction in `replies.rs:1315-1440` before K5.17's guard; pause by a manager follows R34; continue and stop already use `conversations.continue_agents` and `conversations.stop_agents`. Why: accepted requests, R34, R37.
- Third pass: `agent_register` opening an A1 approval request to the person who chose the offer is confirmed (R38); no text change. Why: R38.
