# Every slice, by plan

Generated from `slices.json` by `tools/plan.py --write`. Do not edit by hand.

471 slices; 468 in builds, 3 parked; 6 retired ids.

## P0 (39)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| P0.0 | The sidebar shows only conversations the caller can see | S | 0 | none |  |
| P0.1 | Events keep their side effects | S | 0 | none |  |
| P0.2 | A database blip doesn't stop the server | S | 0 | none |  |
| P0.3 | CI tells the truth (flaky fails; web files run Rust tests) | S | 0 | none |  |
| P0.4 | Agents' task comments mention and reference | S | 0 | none |  |
| P0.5 | Reading through an app clears notifications | S | 0 | none |  |
| P0.6 | One rule for managing and acting as an agent | S | 0 | none |  |
| P0.7 | Module gate covers pages and boards named by id | S | 0 | none |  |
| P0.8 | SQL permission helpers respect the base role | S | 0 | none |  |
| P0.9 | Refusals match tool schemas; public API maps every outcome | S | 0 | none |  |
| P0.10 | Wake-ups posted outside the transaction | S | 0 | none |  |
| P0.11 | GitHub-driven task moves through the task command | S | 0 | none |  |
| P0.12 | Board drag, mark-blocked, ready-for-review through the task command | M | 0 | P0.4, P0.11 |  |
| P0.13 | Valid callers never share a global budget; address header setting | S | 0 | none |  |
| P0.14 | OAuth registration counts only unclaimed clients | S | 0 | P0.13 |  |
| P0.15 | Safe retries on creates, part 1 | S | 0 | none |  |
| P0.16 | Safe retries on creates, part 2; honest instructions; create guard | S | 0 | none |  |
| P0.17 | Contract tests for connected clients | S | 0 | none |  |
| P0.18 | Saving a webhook keeps its wake format | S | 0 | none |  |
| P0.19 | Race tests run on today's schema; naming guard file | S | 0 | none |  |
| P0.20 | Shared integration fixtures | M | 0 | none |  |
| P0.21 | Template database per run; fast test databases | M | 0 | P0.20 |  |
| P0.22 | Privacy net inside a workspace | M | 0 | P0.20, P0.24 |  |
| P0.23 | Read-only tools change nothing | S | 0 | P0.20 |  |
| P0.24 | One source scanner for every guard | S | 0 | none |  |
| P0.25 | Retire upgrade replays for data that no longer exists | S | 0 | P0.19 |  |
| P0.26 | Rules in AGENTS.md and server structure guards | M | 0 | P0.24 |  |
| P0.27 | Web structure guards | S | 0 | none |  |
| P0.28 | Conversation access: the AGENTS.md wording, the thin `conversation_readable_by` wrapper (R32) and the pairing test | S | 0 | P0.26 |  |
| P0.29 | Trigger refusal texts pinned | S | 0 | none |  |
| P0.30 | The master plan lives in the repository | S | 0 | none |  |
| P0.31 | The front door describes today; history moved | S | 0 | P0.30 |  |
| P0.32 | No vendor in tool descriptions and core messages | S | 0 | P0.26 |  |
| P0.33 | Personal machines and repositories out of config, seed and docs | S | 0 | P0.26 |  |
| P0.34 | Dead code out of the build; SDK tests in CI | S | 0 | none |  |
| P0.35 | Schema snapshot on demand | S | 0 | none |  |
| P0.36 | Page comment notifications hold ids, not text (the 140-character preview, `page_docs/comments.rs:451`; C1 and C2's "P0's new preview slice after P0.35") | S | 0 | none |  |
| P0.37 | Account mail never goes through a workspace someone merely joined | S | 0 | none |  |
| P0.38 | No account reveals its other workspaces | S | 0 | P0.20 |  |

## K1 (34)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| K1.1 | Guard rails with a ratchet | S | 1A | P0.24, P0.26 |  |
| K1.2 | The access module and compare mode | M | 1A | none |  |
| K1.3 | A complete SQL role check (fix done by P0.8) | S | 1A | P0.8 |  |
| K1.4 | One rule for managing an agent | M | 1A | P0.6, K1.2 |  |
| K1.5 | Account kinds and one parser | M | 1A | none |  |
| K1.6 | Agents belong to the workspace, not a project | L | 1A | K1.4, K1.5 |  |
| K1.7 | Outside reference and rebind | M | 2A | K1.6 | A1.7, A1.11 |
| K1.8 | Profile and worker agents become agent accounts | M | 3A | K1.4, K1.5, A1.1 |  |
| K1.9 | Catalogue shape: kinds, levels, capabilities | M | 1A | K1.3 |  |
| K1.10 | Every membership names a role row | M | 1A | K1.9 |  |
| K1.11 | Modules off is a permission (gate fix done by P0.7) | M | 3A | P0.7, K1.2, K1.10, M1.2 |  |
| K1.12 | Suggest instead of edit is a permission | M | 1A | K1.10 |  |
| K1.13 | Kind refusals become permissions, part 1 | M | 1A | K1.2 |  |
| K1.14a | Agents may hold any role (role caps and `people_only` removed) | S | 1A | K1.12, K1.13 |  |
| K1.14b | Kind refusals, part 2 (new capabilities, sensitive requests) | M | 3A | K1.14a, A1.8, A2.8 |  |
| K1.15 | Chat onto access | M | 3A | K1.2, K1.10 |  |
| K1.16 | Tasks and task comments onto access | M | 3A | K1.2, K1.10 |  |
| K1.17 | Documents and records onto access | M | 3A | K1.2, K1.10 |  |
| K1.18 | Runs, approvals and code onto access | M | 3A | K1.2, K1.10 |  |
| K1.19 | Members, roles and workspace admin onto access | M | 3A | K1.2, K1.10 |  |
| K1.20 | Lists, search and notifications in SQL | L | 2A | K1.2, K1.10 | K3.10, K4.2 |
| K1.21 | The MCP edge onto access | M | 3A | K1.2, K1.11, K2.3 |  |
| K1.22 | One credentials table and one resolver | M | 2A | K1.6 |  |
| K1.23 | Exchange, short-lived tokens, rotation, revocation; acting for bound agents; reach | M | 2A | K1.22 | A1.11 |
| K1.24 | Narrow tokens for runs and tasks | M | 2A | K1.22 |  |
| K1.25 | Acting context on every action | M | 2A | K1.22, K3.2 |  |
| K1.26 | Permissions in the web | M | 3A | K1.10 | K2.4, W1.4 |
| K1.27 | Groups, guests and role grants | M | 2A | K1.10, K1.20 |  |
| K1.28 | One name per account per workspace | M | 2A | K1.5 |  |
| K1.29 | One way to create a person; one invitation engine; unclaimed accounts | M | 1A | K1.5 |  |
| K1.30 | Server roles and the identity provider seam | M | 2A | K1.22 |  |
| K1.31 | Contract: delete the old paths | M | 5X | none |  |
| K1.32 | Per-thing grants and link tokens | M | 3A | K1.20, K3.10 |  |
| K1.33 | Accounts across workspaces | M | 2A | K1.6, K1.23, K4.23 |  |

## K2 (23)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| K2.1 | Protocol matrix and vendor-free descriptions (rewrite done by P0.32) | S | 1O | none | P0.32 |
| K2.2 | The ops kernel | M | 1O | none |  |
| K2.3 | Generated edges: web, MCP, the v2 route, automations | M | 1O | K2.2 | H1.20 |
| K2.4 | Code generation and drift checks | M | 1O | K2.2, K2.3 |  |
| K2.5 | Template slice: decisions, add-operation skill | S | 1O | K2.3, K2.4 |  |
| K2.6 | Conventions guard and aliases | S | 1O | K2.3 |  |
| K2.7 | Messages (mark-seen fix done by P0.5) | M | 2O | P0.5, K2.5, K2.6 |  |
| K2.8 | Threads and conversations (C1.2's create_thread) | M | 2O | K2.7 | K4.24, C1.2 |
| K2.9 | Reply requests as typed functions and ops | M | 2O | K2.7 |  |
| K2.10 | Pages, skills, page discussion (proposable) | M | 3O | K2.6, C1.7 | K1.12 |
| K2.11 | Sheets and collections | M | 3O | K2.6 |  |
| K2.12 | Whiteboards | S | 2O | K2.6 |  |
| K2.13 | Tasks core writes (mentions fix done by P0.4) | M | 2O | P0.4, K2.6 | T1.3 |
| K2.14 | Tasks: claims, links, planning | M | 2O | K2.13 | K3.15 |
| K2.15 | Task and space reads (one list engine) | M | 2O | K2.13 | T1.9 |
| K2.16 | Inbox, notifications, Later, questions, files | M | 3O | K2.6 |  |
| K2.17 | Workspace, members, roles, settings (space ops are K4.7, K4.8) | M | 3O | none | K1.2, K1.10, K1.12, K4.7, K4.8 |
| K2.18 | Automations, webhooks, code, environments | M | 2O | K2.3, A1.18 |  |
| K2.19 | Runs, approvals, launch, usage: aliases | S | 3O | A1.3, A1.8 |  |
| K2.20 | MCP resources, skills, server card | M | 3O | K2.1, K2.10 |  |
| K2.21 | MCP Tasks extension for long operations | M | 2O | K2.3 |  |
| K2.22 | Contract: delete hand-written edges (per batch) | M | 5X | none |  |
| K2.23 | Alias sunset and public v1 retirement | S | 5X | none |  |

## K3 (25)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| K3.1 | Outbox side effects retry instead of being lost (done by P0.1) | S | 1E | P0.1 |  |
| K3.2 | The event log and the catalogue | M | 1E | none |  |
| K3.3 | Sequencer, cursors and the consumer runtime | M | 1E | K3.2 |  |
| K3.4 | Webhooks and automations read the log | M | 1E | K3.3 |  |
| K3.5 | Pages, sheets and whiteboards in events, automations and webhooks | M | 3E | K3.4 |  |
| K3.6 | Boards and sheets in search | S | 1E | none |  |
| K3.7 | Write-time domain events for tasks, records, messages, runs, PRs | M | 3E | K3.4 |  |
| K3.8 | Events for the writes that have none | M | 1E | K3.2 |  |
| K3.9 | Notifications read the log | S | 3E | K3.3 |  |
| K3.10 | The thing registry | L | 1E | none | T1.3 |
| K3.11 | More kinds in the registry | M | 1E | K3.10 |  |
| K3.12 | The link store, copied and mirrored | M | 1E | K3.10, K3.11 |  |
| K3.13 | Links from text, without route regexes | M | 3E | K3.12 |  |
| K3.14 | One link reader | M | 3E | K3.12, K3.13 | T1.18 |
| K3.15 | Links as operations | M | 3E | K3.14 | K2.3 |
| K3.16 | Dependency and relation readers move to links | M | 3E | K3.15 |  |
| K3.17 | The search index | M | 3E | K3.3, K3.11 |  |
| K3.18 | Search reads the index | M | 3E | K3.17 | K1.20 |
| K3.19 | History as a seam | M | 1E | K3.10 |  |
| K3.20 | Activity feeds and the audit trail from the log | M | 3E | K3.7 | K1.25 |
| K3.21 | Guard rails | S | 2E | K3.2, K3.10 |  |
| K3.22 | The outbox retires | S | 5X | K3.4, K3.9, K5.5 |  |
| K3.23 | Contract the old link stores | S | 5X | K3.16, A2.28 |  |
| K3.24 | The direct scope and merged streams in the log | S | 2A | K3.3, K4.23 |  |
| K3.25 | Cold log and history | M | 3E | K3.20, K5.11 |  |

## K4 (26)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| K4.1 | Space words in the database (views, columns, kinds) | M | 1A | none |  |
| K4.2 | `spaces` module and access helpers | M | 1A | K4.1 |  |
| K4.3 | Guard rails, first set | S | 1A | P0.0, P0.26, K4.1, K4.2 |  |
| K4.4 | Conversations carry their workspace; one full visibility rule | M | 1A | P0.0, K4.2 |  |
| K4.5 | Workspace slugs | S | 1A | none | K2.3 |
| K4.6 | Explicit workspace on workspace-level requests | M | 3A | K4.5 |  |
| K4.7 | Space operations: list, open, create, update, archive | M | 2A | K2.3, K2.5, K4.2 |  |
| K4.8 | Space membership operations and events | M | 2A | K4.7 | K1.10 |
| K4.9 | Personal spaces | M | 2A | K1.4, K4.7 |  |
| K4.10 | The workspace space as a plain kind | M | 1A | K4.2 | M1.2 |
| K4.11 | Private channels: rule and operations | M | 2A | K4.4 |  |
| K4.12 | DM permissions from a shared workspace's role | M | 2A | K4.4 | K1.2, K1.9 |
| K4.13 | Workspace-level carriers off the Messages space | M | 2A | K3.2, K4.4 | K5.5 |
| K4.14 | DM storage, expand | S | 2A | K4.4 |  |
| K4.15 | DMs written and read without a project | M | 2A | K4.12, K4.13, K4.14 | K1.28 |
| K4.16 | Move existing DM rows | S | 3A | K4.15 | A1.1 |
| K4.17 | Retire the hidden Messages projects | S | 3A | K1.6, K1.22, K4.16, A1.1 |  |
| K4.18 | The address module and resolver | M | 2A | K4.5 |  |
| K4.19 | Links switch to workspace addresses | S | 3A | K4.18, W1.14a |  |
| K4.20 | Space starting shapes (M1's package format) | M | 3A | K4.7 | M1.2, M1.17 |
| K4.21 | The rename at the interface | M | 3A | K2.6, K4.7 |  |
| K4.22 | The flip (base tables renamed, old names as views) | M | 5X | none |  |
| K4.23 | The direct scope | S | 2A | K4.14 |  |
| K4.24 | Personal DMs: one per pair, one list everywhere | M | 2A | K4.12, K4.15, K4.23 | K2.3 |
| K4.25 | A chat-only workspace is whole | S | 3A | K4.10, K4.24 | M1.17 |
| K4.26 | Invitation link tokens | S | 1A | none | K1.29, K2.3 |

## K5 (21)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| K5.1 | Cluster test harness and app assembly | S | 1E | none |  |
| K5.2 | Instances and leases | M | 1E | P0.2, K5.1 |  |
| K5.3 | The built-in host behind one lease | M | 1E | K5.2 |  |
| K5.4 | Event positions and the wake (shadow) | M | 1E | K3.2, K5.1 |  |
| K5.5 | Fan-out across instances | M | 1E | K5.4 |  |
| K5.6 | Shared limits | M | 1E | P0.13, K5.1 |  |
| K5.7 | Process roles, drain and readiness | M | 2E | K5.2, K5.5 |  |
| K5.8 | Subscriptions at subscribe time | M | 2E | K5.5 | K1.20, K4.2 |
| K5.9 | Protocol 2, resume, and `follow` | M | 2E | K5.8 |  |
| K5.10 | Presence, typing, and document watching | M | 2E | K5.9 |  |
| K5.11 | Storage seam and streaming uploads | M | 2E | K5.1 |  |
| K5.12 | S3-compatible store and signed links | M | 2E | K5.11 |  |
| K5.13 | Moving files between stores | S | 3E | K5.12 |  |
| K5.14 | Provider sign-ins through the database | S | 2E | K5.3 |  |
| K5.15 | Remove the old paths | S | 5X | K5.9, W1.6 |  |
| K5.16 | One owner per document (deferred) | L | parked: One owner per document: deferred by K5 until measurements ask for it. | K5.2, K5.10, C1.30 |  |
| K5.17 | Pooler-safe database use | S | 1E | K5.4 |  |
| K5.18 | The tick endpoint and a cold `api` | S | 2E | K5.7 |  |
| K5.19 | The live contract and its conformance suite | M | 2E | K5.7, K5.9 |  |
| K5.20 | Direct upload and download, and a workspace's own bucket | M | 3E | K5.12 | K2.3 |
| K5.21 | Live document rooms under the contract | S | 3E | K5.10, K5.19 |  |

## T1 (45)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| T1.1 | Freeze guard for task-side views | S | 1T | none |  |
| T1.2 | One field and view vocabulary | S | 1T | none |  |
| T1.3 | Step 3a: record owns title, values, archive | M | 1T | P0.11, P0.12 |  |
| T1.4a | Step 3b: task readers move (task routes) | M | 1T | T1.3 |  |
| T1.4b | Step 3b: task readers move (MCP, briefing, My work) | M | 1T | T1.3 |  |
| T1.4c | Step 3b: task readers move (the rest) | M | 1T | T1.3 |  |
| T1.4d | Step 3c: drop the cached columns | S | 1T | T1.4a |  |
| T1.5 | `views` module and `RecordSource` | M | 1T | none |  |
| T1.6 | Personal, shared, pinned, default views | M | 3T | K1.9, T1.5 |  |
| T1.7 | Tasks source, accessors, indexes, 10k guard | M | 1T | T1.5 |  |
| T1.8 | Task saved views into the one store | M | 3T | T1.6, T1.7 |  |
| T1.9 | Task lists, `task_list`, automations on the compiler | L | 3T | T1.8 |  |
| T1.10 | Easy field types and system fields | M | 1T | T1.5 |  |
| T1.11 | Step 4: edit Tasks like any collection | M | 3T | P0.11, P0.12, T1.4d, T1.10 |  |
| T1.12 | Step 5: `fields` and `collection` on task tools (done early by #268) | M | 4T | K2.13, T1.11 |  |
| T1.13 | Web views engine on collections, adapters, `FilterEditor` | M | 3T | T1.6, W1.17 | W1.18 |
| T1.13a | The field registry (from W1.16) | M | 4T | T1.2, W1.17 |  |
| T1.14 | Timeline and list kinds for every collection | M | 3T | T1.13 |  |
| T1.15 | Tasks page on views: list (with W1.27a, W1.27d) | M | 3T | T1.9, T1.13, T1.14, W1.18 |  |
| T1.16 | Tasks page on views: board and timeline (with W1.27b) | M | 3T | T1.15, W1.19 |  |
| T1.17 | Formula language, same-record recalculation | M | 3T | T1.10 |  |
| T1.18 | Two-way relations, rollups, recalculation jobs | M | 4T | T1.17 | K3.12 |
| T1.19 | Validation, required, defaults, builder-only | S | 4T | K1.9, T1.10 |  |
| T1.20 | File field | M | 4T | K5.11, T1.10, C1.33 |  |
| T1.21 | Button field | S | 4T | K2.2, T1.10 |  |
| T1.22 | Delegate | S | 4T | T1.7, A1.13 |  |
| T1.23 | Triage queue | M | 4T | T1.7, T1.15 |  |
| T1.24 | Cycles | M | 4T | T1.7, T1.31 |  |
| T1.25 | Step 6: acts like a task (done early by #268; left: forms for task-like collections, switching off once it has records, per-collection workflows, the switch in workspace templates) | L | 4T | T1.12, T1.16 |  |
| T1.26 | Initiatives (containment by `parent_id`) | M | 4T | K3.10, K4.10, T1.14 |  |
| T1.27 | Keyboard-first pass | M | 4T | T1.16, T1.41 |  |
| T1.28 | Forms: definitions, internal, conditions | M | 4T | T1.11 |  |
| T1.29 | Public form limits | S | 3T | none | K5.6 |
| T1.30 | Record templates and recurring records | M | 4T | T1.11 |  |
| T1.31 | Aggregates and chart view kind | M | 3T | T1.13 |  |
| T1.32 | Dashboards module | M | 3T | T1.31, M1.1 | M1.4 |
| T1.33 | Sheets as a dashboard source | S | 4T | T1.32, C1.20 |  |
| T1.34 | Outside data: connections, saved queries | M | 3T | K1.9, K5.6, T1.32 |  |
| T1.35 | Reviewed queries, shared results | M | 4T | T1.34 |  |
| T1.36 | Fields on pages; page lists through views | M | 4T | T1.13 | C1.27 |
| T1.37 | Record comments slot | S | 3T | C1.3, W1.21 |  |
| T1.38 | Automations on records | M | 4T | K2.3, T1.9 |  |
| T1.39 | Contract: remove the second task engine (deletes W1.27a/b/d's old screens) | M | 5X | T1.9, T1.15, T1.16 |  |
| T1.40 | Contract: drop `saved_views` | S | 5X | T1.39 |  |
| T1.41 | Records and tasks proposable, with K3 history | M | 4T | K3.19, T1.11, C1.7, C1.9 |  |

## A1 (23)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| A1.1 | Run record tables and mirror | M | 1G | none |  |
| A1.2 | Run service reads, part 1, and the guard | M | 1G | A1.1 |  |
| A1.3 | Run operations for any agent | L | 1G | A1.1 | K2.2, K2.3, K3.3, K5.6 |
| A1.4 | Connected runners get runs | M | 1G | A1.3 |  |
| A1.5 | Runs page and run view for every agent | M | 2G | A1.2, A1.3 | W1.18 |
| A1.6 | Run service reads, part 2, and neutral wording | M | 2G | A1.2 |  |
| A1.7 | Requests record | M | 1G | A1.1 |  |
| A1.8 | Request operations and answering by role | L | 2G | A1.7 | K1.13, K1.27, C1.7 |
| A1.9 | Run leases and recovery | M | 2G | none | K5.3 |
| A1.10 | One run creation path | M | 2G | A1.2, A1.3 |  |
| A1.11 | Hosts and enrolment | L | 1G | A1.1 | K1.7, K1.22, K1.23, M1.12 |
| A1.12 | Host command channel | L | 2G | A1.3, A1.11 | K1.24, A2.2 |
| A1.13 | One launch path | M | 3G | A1.4, A1.10, A1.12 | T1.22, A2.2, H1.25 |
| A1.14 | Built-in host, step (a) | L | 2G | A1.8, A1.9, A1.10, A1.12 |  |
| A1.15 | Usage by window with confidence | M | 3G | A1.3 |  |
| A1.16 | Built-in host, step (b) | L | 2G | A1.14 |  |
| A1.17 | Built-in host, step (c) | L | 3G | A1.16, H1.13 | M1.14 |
| A1.18 | Code-host seam, part 1 | M | 1G | none |  |
| A1.19 | Code-host seam, part 2 | L | 3G | P0.11, T1.3, A1.3, A1.18 |  |
| A1.20 | A2A binding | L | 3G | A1.8, A1.12 |  |
| A1.21 | ACP in the built-in host | L | 3G | A1.16 |  |
| A1.22 | Contract | M | 5X | A1.6, A1.8, A1.14, A1.17 |  |
| A1.23 | Example host and conformance suite | M | 2G | A1.11, A1.12 | A2.19 |

## A2 (27)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| A2.1 | Connections, subscriptions and the delivery core | M | 2G | K3.2 | K1.6, K3.24 |
| A2.2 | Poll, long-poll and SSE stream from the log | M | 2G | A2.1 | K2.3, K5.9 |
| A2.3 | Standard Webhooks push with alias headers | M | 2G | A2.1 | H1.2 |
| A2.4 | Missing wake-ups (tasks, comments, requests, reminders) | M | 2G | P0.4, K3.2, A2.1 | C1.1 |
| A2.5 | Reply wake-ups on the log | S | 2G | A2.3, A2.4 |  |
| A2.6 | Merge the three old systems behind the new one | M | 3G | A2.5 |  |
| A2.7 | Contract suite and fixtures | M | 2G | A2.2, A2.3 |  |
| A2.8 | Mentions by anyone, chain depth and turn allowance | M | 2G | A2.5 | K1.13 |
| A2.9 | Following and ambient mode | M | 4G | A2.4, A2.8 |  |
| A2.10 | Status, pause, presence and unresponsive handling | M | 3G | A2.2, C2.15 | K5.10 |
| A2.11 | Self-scheduled follow-ups | S | 2G | A2.4 | K5.2 |
| A2.12 | Echoes, client ids, attachments, content marking | S | 3G | A2.2 | K5.12 |
| A2.13 | Ask an agent | S | 3G | A2.8 |  |
| A2.14 | Budgets (rows in H1's spend limits) | M | 4H | K2.2, K5.6, H1.25, H1.27 |  |
| A2.15 | Scope, requested-by and app consent | M | 4G | K1.25, K3.2, A2.1 |  |
| A2.16 | Memory pages | M | 4G | none | C1.27 |
| A2.17 | Agent Plugin and generic setup | M | 3G | A2.2, A2.3 |  |
| A2.18 | Add-agent flow: pick, connect, add one you already have, reconnect | M | 3G | K1.4, K1.6, A2.17 | M1.11 |
| A2.19 | Offers, registration, actions and notices | M | 3G | A1.8, A1.11, A1.12 | K1.7 |
| A2.20 | Start one on a linked host | M | 3G | A2.18, A2.19 | M1.14 |
| A2.22 | Reference channel adapter | M | 3G | A2.7, A2.10, A2.12 |  |
| A2.23 | Hermes plugin | M | 4G | A2.22 |  |
| A2.24 | OpenClaw plugin | M | 4G | A2.22 |  |
| A2.25 | Agent OS connector, bridged | M | 3G | A1.11, A1.12, A2.2 |  |
| A2.26 | Bridged connector: runs, offers and actions | M | 4G | A1.12, A1.15, A2.19, A2.25 |  |
| A2.27 | Team guard | M | 4G | K2.2, A1.8, M1.3 |  |
| A2.28 | Contract the old paths | S | 5X | K1.31, A2.6 |  |

## M1 (20)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| M1.1 | The registry | M | 1O | none |  |
| M1.2 | Module state in a table; gate follows roles and id routes | M | 1O | P0.7, M1.1 |  |
| M1.3 | Server registrations without neighbours | M | 1O | M1.1 |  |
| M1.3k | Kinds, events, search from registrations | M | 4W | K3.2, K3.10, M1.1 |  |
| M1.3o | Operations and permissions from registrations | S | 5X | K1.9, K2.2, M1.1 |  |
| M1.4 | Web registry (the one registration contract, with W1's shell fields) | M | 1O | M1.1 |  |
| M1.5 | Web registry: embeds, live, tabs, composer; web scaffold | M | 3O | M1.4 |  |
| M1.6 | Built modules: tables and operations | M | 1O | M1.2 |  |
| M1.7 | Built modules in the app | M | 4W | M1.4, M1.6 |  |
| M1.8 | Package format and export | M | 1O | M1.1 |  |
| M1.9 | Package preview and install | L | 2O | M1.6, M1.8 |  |
| M1.10 | Package upgrades | M | 4W | M1.9 |  |
| M1.11 | Templates become packages; the directory; switch contract step | M | 2O | M1.2, M1.9 |  |
| M1.12 | Connected modules: install, approve, revoke | L | 3O | K1.5, K1.22, M1.2, M1.6 |  |
| M1.13 | Remote operations and events in | M | 3O | K2.3, K3.3, M1.12 |  |
| M1.14 | Panel blocks (`PanelHost` through W1.25's `PanelBlocks`) | M | 3O | M1.4, M1.12 | W1.25 |
| M1.15 | Framed panels | M | 4W | M1.14 |  |
| M1.16 | Agent-creation offers, panels and actions end to end | S | 4W | A2.20, M1.15 |  |
| M1.17 | Shapes | M | 2O | M1.9, M1.11 |  |
| M1.18 | File sources: the contract | M | 3O | M1.12, M1.13 |  |

## C1 (37)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| C1.1 | Task comment parity now (beyond P0.4) | S | 1C | P0.4 |  |
| C1.2 | One thread-creation function | M | 1C | none |  |
| C1.3 | Discussion foundation (R3 split first if not landed) | L | 1C | P0.0, C1.2 | K3.10 |
| C1.4 | Task comments become conversations | L | 2C | C1.1, C1.3 |  |
| C1.5 | Page comments become conversations | M | 2C | C1.3 |  |
| C1.6 | Retire the old comment tables | S | 5X | C1.4, C1.5, C1.7 |  |
| C1.7 | Proposals record, pages moved onto it | M | 2C | C1.3 | A1.7 |
| C1.8 | Suggest is a role setting (routing only) | M | 3C | K1.12, K2.2, C1.7 |  |
| C1.9 | Change sets and one-tap undo | L | 2C | C1.12 |  |
| C1.10 | Recent-edit outlines in the editors | M | 4D | C1.9 |  |
| C1.11 | Proposals for sheets, boards, records | L | 3C | C1.8, C1.9 | T1.41 |
| C1.12 | Sheet writes stop holding the project lock | S | 1C | none |  |
| C1.13 | Sheet history: snapshots and restore | M | 3C | C1.9, C1.12 |  |
| C1.14 | Number formats and cell styles | M | 3C | C1.12 |  |
| C1.15 | Validation and dropdowns | M | 4D | C1.12 |  |
| C1.16 | Find and replace, sort, filter views | L | 3C | C1.12 |  |
| C1.17 | Conditional formatting | S | 3C | C1.14 |  |
| C1.18 | Charts | M | 4D | T1.31, C1.14 |  |
| C1.19 | Cell comments in the grid | S | 1C | C1.3 |  |
| C1.20 | A sheet as a thing and a source (`sheets::read_range`) | M | 3C | K3.11 |  |
| C1.21 | Raise sheet limits after measuring | S | 1C | C1.12 |  |
| C1.22 | Ask in a page comment | S | 2C | C1.5, C1.9 |  |
| C1.23 | PDF export | M | 1C | none |  |
| C1.24 | Templates for pages and boards | M | 1C | none |  |
| C1.25 | A page from a message, thread or selection | M | 3C | none | K3.12, C2.20 |
| C1.26 | A page tab on a channel (`conversation_tabs`) | S | 4C | C2.8 | K3.12 |
| C1.27 | Page sharing levels and public links | M | 4D | K1.32, C1.3 |  |
| C1.28 | Documents in events and search | M | 4D | K3.5, K3.17 |  |
| C1.29 | Board images through the storage seam | M | 4D | K5.11, C1.33 |  |
| C1.30 | Live cursors, selections and follow | M | 4D | K5.9 |  |
| C1.31 | Element comments and ask about a selection | M | 4D | C1.3 |  |
| C1.32 | Stickies into tasks, a board into a page | M | 4D | none | K3.12 |
| C1.33 | One attachment model and previews | M | 3C | C1.3 | K5.11 |
| C1.34 | History on every document via K3's seam; compaction | M | 4D | K3.19, C1.13, W1.21 |  |
| C1.35 | Document-kind contract test | S | 4D | C1.11, C1.28, C1.34 |  |
| C1.36 | Files from an outside source, by reference | M | 4D | M1.18, C1.33 | K5.14 |
| C1.37 | Move a page between spaces | S/M | 4D | K4.9, C1.3, C1.33 |  |

## C2 (41)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| C2.1 | Close the listing gaps (beyond P0.0, P0.28) | S | 1C | P0.0, P0.28 |  |
| C2.2 | One read path (from P0.5's `mark_seen`) | S | 1C | P0.5 |  |
| C2.3 | Unread rules and counts | M | 1C | C2.2 |  |
| C2.4 | Badge and needs-you on the server | S | 2C | C2.3 |  |
| C2.5 | DMs and threads reach you; A2 kinds | M | 1C | none | P0.4 |
| C2.6 | Private channels | M | 2C | C2.1 |  |
| C2.7 | Join, leave, browse, topic | M | 2C | C2.6 |  |
| C2.7b | Remove membership fallback | S | 5X | C2.7 |  |
| C2.8 | Channel details and tabs (part `channel.tabs`) | M | 4C | C2.7 | K3.12 |
| C2.9 | Sidebar sections | S/M | 2C | C2.3, W1.11, W1.13 |  |
| C2.10 | Triage, faster | S | 2C | C2.4 |  |
| C2.11 | Saved inbox views | M | 4C | C2.4 |  |
| C2.12 | Request cards | S | 4C | A1.8, W1.24 |  |
| C2.13 | Keyword alerts | S | 1C | C2.5 |  |
| C2.14 | Schedules and away | S | 2C | C2.15 |  |
| C2.15 | Status (account and nullable workspace; A2's columns) | M | 1C | none |  |
| C2.16 | Presence, typing, @here (K5's presence) | M | 4C | K5.10 |  |
| C2.17 | Profiles and avatars | M | 3C | K1.5, K1.28 |  |
| C2.18 | User group mentions | S | 3C | K1.27 |  |
| C2.19 | Any emoji and custom emoji | M | 1C | none |  |
| C2.20 | Make from messages: registry and chip | M | 4C | none | K3.12 |
| C2.21 | Make a page or a record | S | 4C | C1.25, C2.20 | T1.11 |
| C2.22 | Make a sheet row or a sticky | S | 4C | C2.20 |  |
| C2.23 | Internal link cards | S/M | 3C | K3.10, W1.21 |  |
| C2.24 | Slash commands from manifests | S | 2C | none | M1.4, M1.5 |
| C2.25 | Search filters and grammar | M | 3C | none | K3.17, K3.18 |
| C2.26 | Saved searches and jump-to | S | 3C | C2.25 |  |
| C2.27 | Answers with sources | M | 4C | K3.17, K3.18, C2.25 |  |
| C2.28 | Drafts across devices | S/M | 1C | none |  |
| C2.29 | Speed | M | 4C | none |  |
| C2.30 | Keyboard and the chat gate | S | 1C | none |  |
| C2.31 | Phone flows | M | 4C | W1.29 |  |
| C2.32 | Share and capture | S | 2C | none | K4.9 |
| C2.33 | Offline reading for chat (W1.31's store) | S | parked: Offline reading in C2: W1.31 does it; waits for the owner's agreement. | W1.31 |  |
| C2.34 | Push that acts | S | 2C | C2.3, C2.4 |  |
| C2.35 | Agents in the room | M | 4C | A1.3, A1.5, A2.8 |  |
| C2.36 | Threads view and outcome | S/M | 1C | C2.5 |  |
| C2.37 | Guests | S | 3C | K1.27, C2.7 |  |
| C2.38 | Shared channels (sketch) | L | parked: Shared channels: a sketch only, later. | K4.24, C2.37 |  |
| C2.39 | DMs belong to their participants | L | 3C | P0.0, K1.33, K4.12, K4.23, K4.24, C2.3 |  |
| C2.40 | Chat standing alone | M | 3C | K1.29, K4.10, K4.26, M1.17, C2.9, W1.13 | K4.25 |

## W1 (47)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| W1.1 | Realtime context split | S | 1O | none |  |
| W1.2 | Every key from a factory | M | 2W | none |  |
| W1.3 | Per-event handlers, no project-wide refetch | M | 3W | W1.2 |  |
| W1.4 | Op client and error sentences | M | 1O | K2.4 |  |
| W1.5 | The thing store | M | 2W | K2.2, K3.10, W1.4 |  |
| W1.6 | Live client on protocol 2 | L | 3W | K3.3, K5.9, K5.10, W1.3, W1.5 |  |
| W1.6b | Remove the old dispatcher | S | 5X | K5.9, W1.6 |  |
| W1.7 | Recorded event streams | S | 3W | W1.3 |  |
| W1.9a | Tasks onto the op client | M | 3W | K2.13, T1.4a, W1.5 |  |
| W1.9b | Conversations, inbox, later onto the op client | M | 4W | K2.7, K2.16, W1.5 |  |
| W1.9c | Pages, sheets, whiteboards onto the op client | M | 4W | K2.10, K2.11, K2.12, W1.5 |  |
| W1.9d | Collections, fields, views, forms onto the op client | M | 4W | K2.11, W1.5 |  |
| W1.9e | Runs, launch, agents onto the op client | M | 4W | K2.19, A1.3, W1.5 |  |
| W1.9f | Settings, team, spaces, onboarding onto the op client | M | 4W | K2.17, W1.5 |  |
| W1.10 | Data router, same URLs | M | 1O | none |  |
| W1.11 | Persistent shell | M | 1O | W1.10 |  |
| W1.12a | Pages off AppShell: chat and inbox folders | M | 2W | W1.11 |  |
| W1.12b | Pages off AppShell: tasks, views, runs, pages folders | M | 3W | W1.11 |  |
| W1.12c | Pages off AppShell: the rest, delete workarounds | M | 3W | W1.12a, W1.12b |  |
| W1.13 | Shell drawn from module registrations | L | 1O | M1.4, W1.11 |  |
| W1.14a | Workspace in the address, switcher with unread marks, DM routes, redirects | M | 2W | K4.5, W1.10, W1.11 |  |
| W1.14b | Space routes and thing addresses | M | 3W | K3.10, K4.7, K4.18, W1.13, W1.14a |  |
| W1.14c | Internal links through href helpers | S | 4W | W1.14b |  |
| W1.14d | Delete legacy redirects | S | 5X | K4.19, W1.14c |  |
| W1.15 | Initial-load diet and budget policy | S | 1O | none |  |
| W1.17 | Pieces foundation (kit moves, SidePanel, Sheet, grid) | M | 1O | none |  |
| W1.18 | Table and list pieces; collections onto them | L | 2W | W1.5, W1.17 |  |
| W1.19 | Board piece; collections onto it | M | 2W | W1.17 |  |
| W1.20 | Calendar and gallery pieces (timeline is T1.14) | M | 3W | W1.17 |  |
| W1.21 | Detail view, history panel, chips, discussion shell | M | 2W | K3.19, C1.3, W1.17 |  |
| W1.22 | Form piece (filter builder is T1.13) | M | 4W | W1.17 | T1.13a |
| W1.24 | Request card and run view | M | 3W | A1.5, A1.8 |  |
| W1.25 | Panel block renderer | M | 4W | M1.12 | T1.31 |
| W1.26 | Embeds through registrations and pieces | M | 4W | M1.5, W1.13, W1.18 | C1.5 |
| W1.27c | Task detail onto the detail view | M | 4W | T1.15, C1.3, W1.9a, W1.21 |  |
| W1.28 | Other lists onto the list piece | M | 4W | W1.18, W1.20 | C2.11 |
| W1.29 | Phone navigation | M | 3W | W1.13, W1.21 |  |
| W1.30 | Outbound queue for any queueable operation | M | 3W | K2.2, W1.4, W1.5 |  |
| W1.31 | Offline reading (opt-in per person, workspace can switch off) | M | 4W | W1.5, W1.30 |  |
| W1.32 | Install, push and capture | S | 2W | A1.7, W1.13, W1.14a |  |
| W1.33 | Speed in CI | M | 4W | W1.11, W1.18 |  |
| W1.34 | Test diet | M | 4W | W1.12c, W1.13 |  |
| W1.35 | The test-for-done harness | M | 1O | none |  |
| W1.36 | Accessibility gate | S | 1O | W1.13, W1.17 |  |
| W1.37 | Role choices from the server, not account kind | S | 2W | K1.10 |  |
| W1.38 | The web app as static files, config fetched at start | S | 2W | W1.10 | H1.41 |
| W1.39 | Shape walk test | M | 3W | M1.17, W1.13, W1.35 | K4.25, C2.40 |

## H1 (63)

| Id | Name | Size | Build | Needs | Soft needs (stopgap) |
| --- | --- | --- | --- | --- | --- |
| H1.1 | Limit scopes, no global bucket for good callers [owner now] | M | 2H | none | K5.6 |
| H1.2 | One outbound guard [owner now] | S | 1H | none |  |
| H1.3 | Tenant table registry and guard | S | 1H | none |  |
| H1.4 | Workspace column: chat | M | 3R | H1.3 |  |
| H1.5 | Workspace column: tasks and planning | M | 3R | H1.3 | T1.3 |
| H1.6 | Workspace column: documents and records | M | 3R | H1.3 |  |
| H1.7 | Workspace column: agents, runs, code, MCP | M | 3R | H1.3 | K1.6, A1.1, A1.22, A2.1 |
| H1.8 | Export declarations | M | 1H | H1.3 | M1.3 |
| H1.9 | Workspace export [owner now] | L | 2H | K1.9, H1.8 | K2.3 |
| H1.10 | Workspace import | L | 3H | K1.29, H1.9, H1.24 | A2.18 |
| H1.11 | Lossless round-trip guard | M | 4H | H1.10 |  |
| H1.12 | Container-ready configuration [owner now] | M | 1H | none |  |
| H1.13 | Image and compose | M | 2H | H1.12 | A1.17 |
| H1.14 | First run in the browser | M | 2H | K1.29, H1.24 | M1.17 |
| H1.15 | Startup under a lease, upgrades, releases | S | 4H | K5.2 |  |
| H1.16 | Portable backup and restore [owner now] | M | 1H | H1.12 |  |
| H1.17 | Server mail transport | S | 2H | none |  |
| H1.18 | Plain OpenID Connect sign-in | M | 3H | K1.30, K5.14 |  |
| H1.19 | Self-host documentation and offline check | M | 3H | H1.13, H1.16 |  |
| H1.20 | Tenancy scope | M | 1H | none | K2.3 |
| H1.21 | Row security: framework and chat | M | 3R | H1.4, H1.20 |  |
| H1.22 | Row security: every other table's policies, observe mode | M | 3R | H1.5, H1.6, H1.7, H1.21 |  |
| H1.23 | Noisy-neighbour controls | M | 4H | K5.6, H1.1, H1.2 |  |
| H1.24 | Workspace lifecycle | M | 2H | K4.5, H1.8 |  |
| H1.25 | Metering core | M | 2H | K2.2, H1.24 | K5.6 |
| H1.26 | Meters wired in | M | 3H | H1.25 |  |
| H1.27 | Spend limits per account, space, conversation | S | 4H | H1.25 | K5.6 |
| H1.28 | Open sign-up with verified email | M | 3H | K1.29, K4.5, H1.17, H1.24 | M1.17 |
| H1.29 | Abuse controls and review queue | M | 3H | H1.1, H1.28 |  |
| H1.30 | Sign-in with common providers | S | 4H | H1.18 |  |
| H1.31 | Company single sign-on | M | 3H | H1.18, H1.25 |  |
| H1.32 | Directory sync | M | 4H | K1.27, H1.31 |  |
| H1.33 | Billing seam and plan state | M | 3H | H1.25, H1.26 |  |
| H1.34 | Checkout, trials, downgrades, dunning | M | 4H | H1.33 |  |
| H1.35 | Staff console: core | M | 3H | H1.20, H1.24, H1.25 |  |
| H1.36 | Staff console: review queue, metrics, support access | M | 4H | H1.29, H1.35 |  |
| H1.37 | Hosted operations profile | M | 4H | K5.7, K5.12, H1.12, H1.17, H1.26 |  |
| H1.38 | Hosted agent service seam | S | 4H | A1.3, A1.11, A1.13, H1.25 |  |
| H1.39 | Release readiness (parts [owner now]) | M | 3H | H1.19 |  |
| H1.40 | Trust documents and publication (licence chosen at release, R25) | M | 5X | H1.39 |  |
| H1.41 | Static web app and client config | S | 1H | H1.12 |  |
| H1.42 | Idle-cost guard and hosting shapes | M | 2H | H1.24 | K5.7, K5.18 |
| H1.43 | A workspace's own bucket: screen and plan rule | S | 4H | K5.20, H1.26 |  |
| H1.44 | Unscoped call-site ratchet | S | 1H | H1.20 |  |
| H1.45 | Scope call sites, one module directory (1 of 18) | S | 1H | H1.44 |  |
| H1.46 | Scope call sites, one module directory (2 of 18) | S | 1H | H1.44 |  |
| H1.47 | Scope call sites, one module directory (3 of 18) | S | 2H | H1.44 |  |
| H1.48 | Scope call sites, one module directory (4 of 18) | S | 2H | H1.44 |  |
| H1.49 | Scope call sites, one module directory (5 of 18) | S | 2H | H1.44 |  |
| H1.50 | Scope call sites, one module directory (6 of 18) | S | 2H | H1.44 |  |
| H1.51 | Scope call sites, one module directory (7 of 18) | S | 2H | H1.44 |  |
| H1.52 | Scope call sites, one module directory (8 of 18) | S | 2H | H1.44 |  |
| H1.53 | Scope call sites, one module directory (9 of 18) | S | 3R | H1.44 |  |
| H1.54 | Scope call sites, one module directory (10 of 18) | S | 3R | H1.44 |  |
| H1.55 | Scope call sites, one module directory (11 of 18) | S | 3R | H1.44 |  |
| H1.56 | Scope call sites, one module directory (12 of 18) | S | 3R | H1.44 |  |
| H1.57 | Scope call sites, one module directory (13 of 18) | S | 3R | H1.44 |  |
| H1.58 | Scope call sites, one module directory (14 of 18) | S | 3R | H1.44 |  |
| H1.59 | Scope call sites, one module directory (15 of 18) | S | 3R | H1.44 |  |
| H1.60 | Scope call sites, one module directory (16 of 18) | S | 3R | H1.44 |  |
| H1.61 | Scope call sites, one module directory (17 of 18) | S | 3R | H1.44 |  |
| H1.62 | Scope call sites, one module directory (18 of 18) | S | 3R | H1.44 |  |
| H1.63 | Strict row rules | S | 4H | H1.22, H1.44, H1.45, H1.46, H1.47, H1.48, H1.49, H1.50, H1.51, H1.52, H1.53, H1.54, H1.55, H1.56, H1.57, H1.58, H1.59, H1.60, H1.61, H1.62 |  |

## Retired ids

- A2.21: Removed (R21: the builder is a host's offer)
- W1.16: (moved to T1.13a: field registry)
- W1.23: (moved to T1.31: chart renderer)
- W1.27a: (merged into T1.15)
- W1.27b: (merged into T1.16)
- W1.27d: (merged into T1.15 and T1.39)
