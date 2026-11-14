# The build order

How the master plan gets built: 31 large builds in six stages. This replaces the wave order
of 2026-11-14 (sixteen waves, one pull request per slice), kept for reference in
`archive/BUILD-ORDER.waves.md`. No feature was removed in the change: the same 471 slices
are here, grouped into bigger pieces of work.

Rulings in `LEAD-RULINGS.md` beat plan text. R44 to R50 are the rulings behind this file.
The list of slices, builds, stages and milestones is `slices.json`; the parts of this file
between `gen:` markers are written from it by `tools/plan.py --write` and are never edited
by hand.

## Contents

1. The short version
2. Why builds, not slices
3. How to run a build
4. Stages and builds, every slice in order
5. The journey: the proof at the end of stage 2
6. Milestones a person notices
7. What can start now
8. Rules between builds
9. Keeping the plan honest
10. What moved from the wave order
11. Hot files
12. Decisions that hold builds up
13. Integration with Agent OS: outside this order

## 1. The short version

<!-- gen:counts -->
- **471 slices** in the fourteen plans (P0 39, K1 34, K2 23, K3 25, K4 26, K5 21, T1 45, A1 23, A2 27, M1 20, C1 37, C2 41, W1 47, H1 63). 468 are in builds; 3 are parked with a reason (K5.16, C2.33, C2.38). 6 more ids are retired and kept only as pointers (A2.21, W1.16, W1.23, W1.27a, W1.27b, W1.27d).
- **31 builds in 6 stages.** One build is one long session, one branch and one pull request.

| Stage | Name | Builds | Slices |
| --- | --- | --- | --- |
| 0 | Steady ground | 1 | 39 |
| 1 | Foundations | 7 | 102 |
| 2 | Agents are accounts everywhere | 7 | 99 |
| 3 | The whole product on the foundations | 9 | 128 |
| 4 | Complete, hosted and connected | 6 | 83 |
| 5 | Clean-up and release | 1 | 17 |
| | **Total** | **31** | **468** |
<!-- /gen:counts -->

- **A slice is a line in a build's work list**, not a pull request. A build's session does
  its slices in order, commits as it goes, and opens one pull request at the end, so CI and
  review run on the whole build, a few times, instead of on every slice.
- **A build starts when the builds it needs have merged.** Stages are for reading and for
  the gates: each stage ends with something that must work before the lead moves on.
- **Stages 0 to 2 are fixed. Stages 3 to 5 are the current cut** and are cut again at the
  stage 2 gate from what the first fifteen builds showed (section 9).
- **No dates and no day counts.** Sizes (S, M, L) only compare slices with each other. How
  long a build takes is measured on the first builds, not guessed.
- **Your own agents at any role, editing directly: after stage 1.** One agent in all your
  workspaces: after stage 2. A chat-only workspace whole, and adding an agent three ways:
  after stage 3. The hosted service can open: after stage 4.

## 2. Why builds, not slices

The wave order gave every slice its own session, pull request, CI run and review round. For
471 slices that is at least 471 CI runs if everything passes first time, and in practice
well over a thousand, with a rebase between most of them. The sixteen waves were not asked
for by the dependencies either: counted by hard dependencies alone, the work is about four
layers deep. The waves came from assuming eight to ten small sessions at once.

A session can carry much more than one slice. So:

| | Wave order | This order |
| --- | --- | --- |
| Unit of work | a slice (one change) | a build (7 to 22 slices on one theme; build 0 has 39 small fixes) |
| Sessions and pull requests | 471 | 31 |
| Sync points | 16 waves | 6 stages, each ending in a gate that must work |
| CI and review | per slice | per build, aiming for three CI runs or fewer |
| Rebases | most slices | when a sibling build merges: a handful per build |
| Estimates | session-days, two-week waves | none; measured on the first builds |

What it costs: a build's pull request is large, a stopgap is needed a little more often
(section 8), and a mistake found late in a build is found later than it would have been.
The guards from stage 0, a commit per slice, and the gates are what keep that in hand.

## 3. How to run a build

1. **Start.** The lead starts a build when everything under its "Starts after" has merged.
   The session claims the build's task in the AWD project and reads, in this order:
   `END-STATE.md`, `LEAD-RULINGS.md`, this file, the plan sections for its slices, `AGENTS.md`
   and the skills that fit (`add-operation`, `add-migration`, `add-mcp-tool`,
   `check-in-browser`, `steward`).
2. **One branch**, `build/<id>`. Push it as the work goes so nothing is lost. Don't open the
   pull request yet: CI runs on pull requests and on `main` only, so pushes to the branch
   cost nothing.
3. **Slices in the listed order.** One or more commits per slice, each message starting
   with the slice id. After each slice: `cargo check` and the tests for the code touched
   (web: typecheck and the touched tests). Tick the slice off in the task.
4. **Test first where a bug would be silent**: permissions, private conversations, claims
   and leases, reply rules, credentials. Add each slice's isolation cases.
5. **A seam ships with a real user** (R48). A new module, trait or table that nothing in
   the build uses yet is not done.
6. **Soft needs.** If a soft need has landed, use it. If not, build the stopgap the slice
   names, list it in the pull request under "Stopgaps" and name the build that removes it.
   Never wait on a soft need; never build past a hard one.
7. **When a sibling build merges**, merge `main` into the branch at the next slice boundary.
8. **Finish.** Merge `main`, run the full local check once (`tools/ci/run.sh`, or the same
   commands in a cloud session), and walk P0's definition of done once for the whole build:
   guards green and allowances only going down, interface changes written in
   `interfaces.md`, new tables named by R1 with an export declaration and their triggers
   listed (R16), layout parts registered with the shapes that show them (R22), nothing per
   workspace and pooler-safe (R24).
9. **One pull request**, titled `Build <id>: <name>`. The description lists every slice with
   its commits, the stopgaps, interface changes, and anything found and not done (each as a
   new task). Review goes slice by slice along the commits.
10. **One review round.** CI and the review run on the whole pull request; fix everything
    they find in one pass and push once. Aim for three CI runs or fewer; if a build needs
    more, the pull request says why, and the lead writes it down (section 9).
11. **If the session can't finish**, it stops at a slice boundary, opens the pull request
    with the slices that are done, and hands the rest back. The lead moves the unfinished
    slices to a follow-on build in `slices.json` with a written reason. Half a slice never
    ships.
12. **Record what it took** in the build's task: slices done, sessions used, CI runs,
    review rounds, stopgaps left behind.

## 4. Stages and builds, every slice in order

A build id is its stage and a letter: A accounts and spaces, E events and servers, H
hosting, R row security, O operations and modules, W web, T tasks and records, C chat (and
documents until stage 4), D documents, G agents and hosts, X the clean-up. Within a build
the slices are listed in the order to do them.

<!-- gen:stages -->
### Stage 0: Steady ground

**Done when:** Private things stay private: no DM thread names in anyone else's sidebar, no comment text in notifications, no reset mail through a workspace someone merely joined, no strangers in people search. CI fails on flaky tests. The plan documents are in the repository.

#### Build 0: Steady ground

Silent bugs, privacy leaks, shared limits, safe retries, guards, honest CI, the plan in the repository.

- **Slices:** 39 (34 S, 5 M), from P0.
- **Starts after:** nothing; it can start now.
- **In this order:** P0.0 The sidebar shows only conversations the caller can see; P0.1 Events keep their side effects; P0.2 A database blip doesn't stop the server; P0.3 CI tells the truth (flaky fails; web files run Rust tests); P0.4 Agents' task comments mention and reference; P0.5 Reading through an app clears notifications; P0.6 One rule for managing and acting as an agent; P0.7 Module gate covers pages and boards named by id; P0.8 SQL permission helpers respect the base role; P0.9 Refusals match tool schemas; public API maps every outcome; P0.10 Wake-ups posted outside the transaction; P0.11 GitHub-driven task moves through the task command; P0.12 Board drag, mark-blocked, ready-for-review through the task command; P0.13 Valid callers never share a global budget; address header setting; P0.14 OAuth registration counts only unclaimed clients; P0.15 Safe retries on creates, part 1; P0.16 Safe retries on creates, part 2; honest instructions; create guard; P0.17 Contract tests for connected clients; P0.18 Saving a webhook keeps its wake format; P0.19 Race tests run on today's schema; naming guard file; P0.20 Shared integration fixtures; P0.21 Template database per run; fast test databases; P0.23 Read-only tools change nothing; P0.24 One source scanner for every guard; P0.22 Privacy net inside a workspace; P0.25 Retire upgrade replays for data that no longer exists; P0.26 Rules in AGENTS.md and server structure guards; P0.27 Web structure guards; P0.28 Conversation access: the AGENTS.md wording, the thin `conversation_readable_by` wrapper (R32) and the pairing test; P0.29 Trigger refusal texts pinned; P0.30 The master plan lives in the repository; P0.31 The front door describes today; history moved; P0.32 No vendor in tool descriptions and core messages; P0.33 Personal machines and repositories out of config, seed and docs; P0.34 Dead code out of the build; SDK tests in CI; P0.35 Schema snapshot on demand; P0.36 Page comment notifications hold ids, not text (the 140-character preview, `page_docs/comments.rs:451`; C1 and C2's "P0's new preview slice after P0.35"); P0.37 Account mail never goes through a workspace someone merely joined; P0.38 No account reveals its other workspaces.

### Stage 1: Foundations

**Done when:** The app works as before on the new foundations, and one connected (outside) agent takes a run and finishes it through operations. The event log receives every write; one operation is defined once and served to the web and to MCP; the access module answers every check in compare mode with no differences; your own agents hold any role and edit directly; a host can enrol.

#### Build 1A: Access and roles

The access module, roles as rows, agents at any role editing directly, workspace slugs, the workspace's own space, invitation links, conversations that carry their workspace.

- **Slices:** 19 (6 S, 12 M, 1 L), from K1, K4.
- **Starts after:** build 0.
- **Stopgap unless it has landed:** K4.5 on K2.3 (build 1O); K4.10 on M1.2 (build 1O); K4.26 on K2.3 (build 1O).
- **In this order:** K1.1 Guard rails with a ratchet; K1.2 The access module and compare mode; K1.3 A complete SQL role check (fix done by P0.8); K1.5 Account kinds and one parser; K4.1 Space words in the database (views, columns, kinds); K4.5 Workspace slugs; K1.4 One rule for managing an agent; K1.9 Catalogue shape: kinds, levels, capabilities; K1.29 One way to create a person; one invitation engine; unclaimed accounts; K4.2 `spaces` module and access helpers; K1.6 Agents belong to the workspace, not a project; K1.10 Every membership names a role row; K1.13 Kind refusals become permissions, part 1; K4.3 Guard rails, first set; K4.4 Conversations carry their workspace; one full visibility rule; K4.10 The workspace space as a plain kind; K4.26 Invitation link tokens; K1.12 Suggest instead of edit is a permission; K1.14a Agents may hold any role (role caps and `people_only` removed).

#### Build 1C: Chat and comments basics

Right unread counts, DMs and thread replies that reach you, status, any emoji, drafts across devices, the discussion foundation, sheets that don't lock the project, PDF export, templates.

- **Slices:** 18 (8 S, 2 S/M, 7 M, 1 L), from C1, C2.
- **Starts after:** build 0.
- **Stopgap unless it has landed:** C1.3 on K3.10 (build 1E).
- **In this order:** C1.2 One thread-creation function; C1.12 Sheet writes stop holding the project lock; C1.23 PDF export; C1.24 Templates for pages and boards; C2.1 Close the listing gaps (beyond P0.0, P0.28); C2.2 One read path (from P0.5's `mark_seen`); C2.15 Status (account and nullable workspace; A2's columns); C2.19 Any emoji and custom emoji; C2.28 Drafts across devices; C2.30 Keyboard and the chat gate; C1.3 Discussion foundation (R3 split first if not landed); C2.3 Unread rules and counts; C2.5 DMs and threads reach you; A2 kinds; C1.1 Task comment parity now (beyond P0.4); C1.19 Cell comments in the grid; C1.21 Raise sheet limits after measuring; C2.13 Keyword alerts; C2.36 Threads view and outcome.

#### Build 1E: Event log and many servers

The event log and sequencer, the thing registry, the link store, leases, fan-out across instances, shared limits, pooler-safe database use.

- **Slices:** 17 (4 S, 12 M, 1 L), from K3, K5.
- **Starts after:** build 0.
- **Stopgap unless it has landed:** K3.10 on T1.3 (build 1T).
- **In this order:** K3.1 Outbox side effects retry instead of being lost (done by P0.1); K3.2 The event log and the catalogue; K3.6 Boards and sheets in search; K3.10 The thing registry; K5.1 Cluster test harness and app assembly; K3.3 Sequencer, cursors and the consumer runtime; K3.8 Events for the writes that have none; K3.11 More kinds in the registry; K5.2 Instances and leases; K5.4 Event positions and the wake (shadow); K3.4 Webhooks and automations read the log; K3.12 The link store, copied and mirrored; K3.19 History as a seam; K5.3 The built-in host behind one lease; K5.5 Fan-out across instances; K5.6 Shared limits; K5.17 Pooler-safe database use.

#### Build 1G: Runs, requests and hosts

Run records, run operations for any agent, connected runners that get runs, the requests record, hosts and enrolment, the code-host seam.

- **Slices:** 7 (5 M, 2 L), from A1.
- **Starts after:** nothing; it can start now.
- **Stopgap unless it has landed:** A1.3 on K2.2 (build 1O); A1.3 on K2.3 (build 1O); A1.3 on K3.3 (build 1E); A1.3 on K5.6 (build 1E); A1.11 on K1.7 (build 2A); A1.11 on K1.22 (build 2A); A1.11 on K1.23 (build 2A); A1.11 on M1.12 (build 3O).
- **In this order:** A1.1 Run record tables and mirror; A1.3 Run operations for any agent; A1.7 Requests record; A1.18 Code-host seam, part 1; A1.2 Run service reads, part 1, and the guard; A1.11 Hosts and enrolment; A1.4 Connected runners get runs.

#### Build 1H: Hosting groundwork

One outbound guard, the tenant table registry, container-ready configuration, tenancy scope, portable backup and restore, the static web app, export declarations, the call-site ratchet.

- **Slices:** 10 (6 S, 4 M), from H1.
- **Starts after:** nothing; it can start now.
- **Stopgap unless it has landed:** H1.20 on K2.3 (build 1O); H1.8 on M1.3 (build 1O).
- **In this order:** H1.2 One outbound guard [owner now]; H1.3 Tenant table registry and guard; H1.12 Container-ready configuration [owner now]; H1.20 Tenancy scope; H1.16 Portable backup and restore [owner now]; H1.41 Static web app and client config; H1.44 Unscoped call-site ratchet; H1.8 Export declarations; H1.45 Scope call sites, one module directory (1 of 18); H1.46 Scope call sites, one module directory (2 of 18).

#### Build 1O: Operation layer, modules and the web shell

The ops kernel with generated web, MCP and v2 edges; the module registry, module state and packages; the persistent shell drawn from module registrations; the op client; the pieces foundation; the test-for-done harness.

- **Slices:** 21 (6 S, 14 M, 1 L), from K2, M1, W1.
- **Starts after:** build 0.
- **Stopgap unless it has landed:** K2.3 on H1.20 (build 1H).
- **In this order:** K2.1 Protocol matrix and vendor-free descriptions (rewrite done by P0.32); K2.2 The ops kernel; M1.1 The registry; W1.1 Realtime context split; W1.10 Data router, same URLs; W1.15 Initial-load diet and budget policy; W1.17 Pieces foundation (kit moves, SidePanel, Sheet, grid); K2.3 Generated edges: web, MCP, the v2 route, automations; K2.4 Code generation and drift checks; M1.2 Module state in a table; gate follows roles and id routes; M1.3 Server registrations without neighbours; M1.4 Web registry (the one registration contract, with W1's shell fields); W1.11 Persistent shell; W1.35 The test-for-done harness; K2.5 Template slice: decisions, add-operation skill; K2.6 Conventions guard and aliases; M1.6 Built modules: tables and operations; M1.8 Package format and export; W1.4 Op client and error sentences; W1.13 Shell drawn from module registrations; W1.36 Accessibility gate.

#### Build 1T: Tasks become records

The record owns title, values and archive; every task reader moves; the cached columns go; the views module; easy field types.

- **Slices:** 10 (3 S, 7 M), from T1.
- **Starts after:** build 0.
- **In this order:** T1.1 Freeze guard for task-side views; T1.2 One field and view vocabulary; T1.5 `views` module and `RecordSource`; T1.3 Step 3a: record owns title, values, archive; T1.7 Tasks source, accessors, indexes, 10k guard; T1.4a Step 3b: task readers move (task routes); T1.4b Step 3b: task readers move (MCP, briefing, My work); T1.10 Easy field types and system fields; T1.4c Step 3b: task readers move (the rest); T1.4d Step 3c: drop the cached columns.

### Stage 2: Agents are accounts everywhere

**Done when:** The journey passes as an automated test (section 5): one outside agent, one identity, two workspaces; it receives an event, edits through an operation, raises a request an authorised account answers, retries without duplicating, and loses access the moment its membership is revoked. Every hosted-cheap guard of R24 is on.

#### Build 2A: One account everywhere: credentials, DMs, spaces

One credentials table, short-lived tokens, accounts across workspaces, the direct scope, one DM per pair, space operations, private channels, personal spaces, groups and guests, addresses.

- **Slices:** 22 (3 S, 18 M, 1 L), from K1, K3, K4.
- **Starts after:** build 1A, build 1E, build 1O.
- **In this order:** K1.20 Lists, search and notifications in SQL; K1.22 One credentials table and one resolver; K1.28 One name per account per workspace; K4.7 Space operations: list, open, create, update, archive; K4.11 Private channels: rule and operations; K4.12 DM permissions from a shared workspace's role; K4.13 Workspace-level carriers off the Messages space; K4.14 DM storage, expand; K1.7 Outside reference and rebind; K1.23 Exchange, short-lived tokens, rotation, revocation; acting for bound agents; reach; K4.8 Space membership operations and events; K4.15 DMs written and read without a project; K4.23 The direct scope; K1.24 Narrow tokens for runs and tasks; K1.25 Acting context on every action; K1.27 Groups, guests and role grants; K1.33 Accounts across workspaces; K3.24 The direct scope and merged streams in the log; K4.9 Personal spaces; K4.24 Personal DMs: one per pair, one list everywhere; K1.30 Server roles and the identity provider seam; K4.18 The address module and resolver.

#### Build 2C: Conversations everywhere

Page and task comments become conversations, private channels, sidebar sections, badge and needs-you counts from the server, one-tap undo, the proposals record, triage, schedules, slash commands.

- **Slices:** 14 (7 S, 1 S/M, 4 M, 2 L), from C1, C2.
- **Starts after:** build 1C, build 1O.
- **Stopgap unless it has landed:** C2.24 on M1.5 (build 3O); C2.32 on K4.9 (build 2A).
- **In this order:** C1.5 Page comments become conversations; C2.9 Sidebar sections; C1.4 Task comments become conversations; C2.4 Badge and needs-you on the server; C2.6 Private channels; C2.10 Triage, faster; C2.14 Schedules and away; C2.24 Slash commands from manifests; C2.32 Share and capture; C2.34 Push that acts; C1.9 Change sets and one-tap undo; C1.22 Ask in a page comment; C2.7 Join, leave, browse, topic; C1.7 Proposals record, pages moved onto it.

#### Build 2E: Live protocol, storage and cold start

Protocol 2 with resume, presence and typing, the storage seam and S3-compatible store, process roles, the tick endpoint and a cold api, the live contract and its conformance suite.

- **Slices:** 10 (3 S, 7 M), from K3, K5.
- **Starts after:** build 1E.
- **Stopgap unless it has landed:** K5.8 on K1.20 (build 2A).
- **In this order:** K5.7 Process roles, drain and readiness; K5.8 Subscriptions at subscribe time; K3.21 Guard rails; K5.9 Protocol 2, resume, and `follow`; K5.11 Storage seam and streaming uploads; K5.18 The tick endpoint and a cold `api`; K5.12 S3-compatible store and signed links; K5.19 The live contract and its conformance suite; K5.10 Presence, typing, and document watching; K5.14 Provider sign-ins through the database.

#### Build 2G: Delivery to agents and the host protocol

Requests answered by role, run leases, the host command channel, the built-in host on the protocol, connections and subscriptions, webhooks and the stream, the contract suite, the example host with its conformance tests, the runs page.

- **Slices:** 17 (2 S, 11 M, 4 L), from A1, A2.
- **Starts after:** build 1E, build 1G.
- **Stopgap unless it has landed:** A1.8 on K1.27 (build 2A); A1.8 on C1.7 (build 2C); A1.12 on K1.24 (build 2A); A2.1 on K3.24 (build 2A); A2.2 on K5.9 (build 2E); A1.5 on W1.18 (build 2W); A1.23 on A2.19 (build 3G).
- **In this order:** A1.8 Request operations and answering by role; A1.9 Run leases and recovery; A1.10 One run creation path; A1.12 Host command channel; A2.1 Connections, subscriptions and the delivery core; A1.14 Built-in host, step (a); A2.3 Standard Webhooks push with alias headers; A2.4 Missing wake-ups (tasks, comments, requests, reminders); A2.11 Self-scheduled follow-ups; A1.16 Built-in host, step (b); A2.2 Poll, long-poll and SSE stream from the log; A2.5 Reply wake-ups on the log; A2.7 Contract suite and fixtures; A2.8 Mentions by anyone, chain depth and turn allowance; A1.5 Runs page and run view for every agent; A1.6 Run service reads, part 2, and neutral wording; A1.23 Example host and conformance suite.

#### Build 2H: Workspace lifecycle, metering, export, first run

Workspace lifecycle and export, limit scopes, server mail, the metering core, the idle-cost guard, image and compose, first run in the browser.

- **Slices:** 14 (7 S, 6 M, 1 L), from H1.
- **Starts after:** build 1A, build 1H, build 1O.
- **Stopgap unless it has landed:** H1.42 on K5.7 (build 2E); H1.42 on K5.18 (build 2E); H1.13 on A1.17 (build 3G); H1.14 on M1.17 (build 2O).
- **In this order:** H1.24 Workspace lifecycle; H1.47 Scope call sites, one module directory (3 of 18); H1.48 Scope call sites, one module directory (4 of 18); H1.1 Limit scopes, no global bucket for good callers [owner now]; H1.9 Workspace export [owner now]; H1.17 Server mail transport; H1.25 Metering core; H1.42 Idle-cost guard and hosting shapes; H1.49 Scope call sites, one module directory (5 of 18); H1.50 Scope call sites, one module directory (6 of 18); H1.13 Image and compose; H1.14 First run in the browser; H1.51 Scope call sites, one module directory (7 of 18); H1.52 Scope call sites, one module directory (8 of 18).

#### Build 2O: Operations for chat, tasks and automations; packages and shapes

Messages, threads, tasks, whiteboards, automations and long operations on the operation layer; package preview and install; templates as packages; workspace shapes.

- **Slices:** 12 (1 S, 10 M, 1 L), from K2, M1.
- **Starts after:** build 1G, build 1O.
- **Stopgap unless it has landed:** K2.8 on K4.24 (build 2A); K2.14 on K3.15 (build 3E); K2.15 on T1.9 (build 3T).
- **In this order:** K2.7 Messages (mark-seen fix done by P0.5); M1.9 Package preview and install; K2.13 Tasks core writes (mentions fix done by P0.4); M1.11 Templates become packages; the directory; switch contract step; M1.17 Shapes; K2.12 Whiteboards; K2.8 Threads and conversations (C1.2's create_thread); K2.9 Reply requests as typed functions and ops; K2.14 Tasks: claims, links, planning; K2.15 Task and space reads (one list engine); K2.18 Automations, webhooks, code, environments; K2.21 MCP Tasks extension for long operations.

#### Build 2W: Web: addresses, the thing store and the main pieces

Workspace in the address with a switcher, the thing store, table, list, board and detail pieces, pages off the old shell, install and push.

- **Slices:** 10 (3 S, 6 M, 1 L), from W1.
- **Starts after:** build 1A, build 1C, build 1E, build 1G, build 1O.
- **In this order:** W1.37 Role choices from the server, not account kind; W1.5 The thing store; W1.14a Workspace in the address, switcher with unread marks, DM routes, redirects; W1.38 The web app as static files, config fetched at start; W1.12a Pages off AppShell: chat and inbox folders; W1.18 Table and list pieces; collections onto them; W1.32 Install, push and capture; W1.2 Every key from a factory; W1.19 Board piece; collections onto it; W1.21 Detail view, history panel, chips, discussion shell.

### Stage 3: The whole product on the foundations

**Done when:** A workspace with only Chat on is whole (the shape walk passes for every shape). An agent can be added three ways. Open sign-up works. The web app is live on protocol 2 with resume. Every area of the server answers through the access module. Row security runs in observe mode with no unscoped call sites left.

#### Build 3A: Everything onto the access module; chat-only; the DM move

Every area of the server onto access, permissions in the web, per-thing grants and link tokens, sensitive requests by permission, a chat-only workspace made whole, existing DMs moved, the rename at the interface.

- **Slices:** 18 (4 S, 14 M), from K1, K4.
- **Starts after:** build 2A, build 2G, build 2W.
- **In this order:** K1.8 Profile and worker agents become agent accounts; K1.11 Modules off is a permission (gate fix done by P0.7); K4.25 A chat-only workspace is whole; K1.14b Kind refusals, part 2 (new capabilities, sensitive requests); K1.15 Chat onto access; K1.16 Tasks and task comments onto access; K1.17 Documents and records onto access; K1.18 Runs, approvals and code onto access; K1.19 Members, roles and workspace admin onto access; K1.21 The MCP edge onto access; K1.26 Permissions in the web; K1.32 Per-thing grants and link tokens; K4.6 Explicit workspace on workspace-level requests; K4.20 Space starting shapes (M1's package format); K4.21 The rename at the interface; K4.16 Move existing DM rows; K4.17 Retire the hidden Messages projects; K4.19 Links switch to workspace addresses.

#### Build 3C: DMs owned by their participants; chat alone; sheets and proposals

DMs that belong to the people in them, chat standing alone, profiles, guests, group mentions, search grammar, sheet history, formats, proposals for sheets, boards and records, one attachment model.

- **Slices:** 17 (4 S, 1 S/M, 9 M, 3 L), from C1, C2.
- **Starts after:** build 2A, build 2C, build 2O, build 2W.
- **Stopgap unless it has landed:** C2.40 on K4.25 (build 3A); C1.11 on T1.41 (build 4T); C2.25 on K3.17 (build 3E); C2.25 on K3.18 (build 3E); C1.25 on C2.20 (build 4C).
- **In this order:** C2.17 Profiles and avatars; C2.18 User group mentions; C2.23 Internal link cards; C2.37 Guests; C2.39 DMs belong to their participants; C2.40 Chat standing alone; C1.8 Suggest is a role setting (routing only); C1.13 Sheet history: snapshots and restore; C1.11 Proposals for sheets, boards, records; C1.14 Number formats and cell styles; C1.17 Conditional formatting; C1.33 One attachment model and previews; C2.25 Search filters and grammar; C2.26 Saved searches and jump-to; C1.16 Find and replace, sort, filter views; C1.20 A sheet as a thing and a source (`sheets::read_range`); C1.25 A page from a message, thread or selection.

#### Build 3E: Search, links, activity and cold storage

Write-time domain events, links from text and as operations, the search index, activity feeds and the audit trail from the log, cold log and history, direct upload and a workspace's own bucket.

- **Slices:** 14 (3 S, 11 M), from K3, K5.
- **Starts after:** build 2E.
- **Stopgap unless it has landed:** K3.14 on T1.18 (build 4T).
- **In this order:** K3.7 Write-time domain events for tasks, records, messages, runs, PRs; K3.13 Links from text, without route regexes; K3.17 The search index; K5.13 Moving files between stores; K5.21 Live document rooms under the contract; K3.5 Pages, sheets and whiteboards in events, automations and webhooks; K3.14 One link reader; K3.15 Links as operations; K3.18 Search reads the index; K3.20 Activity feeds and the audit trail from the log; K3.16 Dependency and relation readers move to links; K5.20 Direct upload and download, and a workspace's own bucket; K3.9 Notifications read the log; K3.25 Cold log and history.

#### Build 3G: Add an agent three ways; one launch path; open protocols

The add-agent flow, offers and registration, starting one on a linked host, the built-in host packaged, one launch path, A2A and ACP, the reference channel adapter, the bridged Agent OS connector.

- **Slices:** 16 (2 S, 10 M, 4 L), from A1, A2.
- **Starts after:** build 1C, build 1T, build 2G, build 2H.
- **Stopgap unless it has landed:** A1.17 on M1.14 (build 3O); A1.13 on T1.22 (build 4T); A2.20 on M1.14 (build 3O).
- **In this order:** A1.17 Built-in host, step (c); A2.10 Status, pause, presence and unresponsive handling; A2.12 Echoes, client ids, attachments, content marking; A2.13 Ask an agent; A2.17 Agent Plugin and generic setup; A1.15 Usage by window with confidence; A2.18 Add-agent flow: pick, connect, add one you already have, reconnect; A2.19 Offers, registration, actions and notices; A2.22 Reference channel adapter; A2.25 Agent OS connector, bridged; A1.13 One launch path; A1.19 Code-host seam, part 2; A1.20 A2A binding; A1.21 ACP in the built-in host; A2.6 Merge the three old systems behind the new one; A2.20 Start one on a linked host.

#### Build 3H: Sign-up, sign-in, billing seam, staff console

Meters wired in, workspace import, OpenID Connect and company single sign-on, open sign-up, abuse controls, the billing seam, the staff console, self-host documentation.

- **Slices:** 10 (9 M, 1 L), from H1.
- **Starts after:** build 2A, build 2E, build 2H.
- **Stopgap unless it has landed:** H1.10 on A2.18 (build 3G).
- **In this order:** H1.26 Meters wired in; H1.10 Workspace import; H1.18 Plain OpenID Connect sign-in; H1.19 Self-host documentation and offline check; H1.28 Open sign-up with verified email; H1.29 Abuse controls and review queue; H1.31 Company single sign-on; H1.33 Billing seam and plan state; H1.35 Staff console: core; H1.39 Release readiness (parts [owner now]).

#### Build 3O: Operations for documents and settings; connected modules and panels

Pages, sheets, collections, inbox, settings and runs on the operation layer; MCP resources; connected modules; remote operations; panel blocks; the file source contract.

- **Slices:** 11 (1 S, 9 M, 1 L), from K2, M1.
- **Starts after:** build 2A, build 2C, build 2G.
- **Stopgap unless it has landed:** M1.14 on W1.25 (build 4W).
- **In this order:** M1.12 Connected modules: install, approve, revoke; M1.13 Remote operations and events in; M1.14 Panel blocks (`PanelHost` through W1.25's `PanelBlocks`); K2.10 Pages, skills, page discussion (proposable); K2.11 Sheets and collections; K2.16 Inbox, notifications, Later, questions, files; K2.17 Workspace, members, roles, settings (space ops are K4.7, K4.8); M1.5 Web registry: embeds, live, tabs, composer; web scaffold; M1.18 File sources: the contract; K2.19 Runs, approvals, launch, usage: aliases; K2.20 MCP resources, skills, server card.

#### Build 3R: Workspace columns and row security

Every table gets its workspace column, the remaining call sites are scoped, row security policies on every table in observe mode.

- **Slices:** 16 (10 S, 6 M), from H1.
- **Starts after:** build 1H.
- **Stopgap unless it has landed:** H1.7 on A1.22 (build 5X).
- **In this order:** H1.4 Workspace column: chat; H1.5 Workspace column: tasks and planning; H1.6 Workspace column: documents and records; H1.7 Workspace column: agents, runs, code, MCP; H1.53 Scope call sites, one module directory (9 of 18); H1.54 Scope call sites, one module directory (10 of 18); H1.55 Scope call sites, one module directory (11 of 18); H1.21 Row security: framework and chat; H1.56 Scope call sites, one module directory (12 of 18); H1.57 Scope call sites, one module directory (13 of 18); H1.58 Scope call sites, one module directory (14 of 18); H1.59 Scope call sites, one module directory (15 of 18); H1.60 Scope call sites, one module directory (16 of 18); H1.22 Row security: every other table's policies, observe mode; H1.61 Scope call sites, one module directory (17 of 18); H1.62 Scope call sites, one module directory (18 of 18).

#### Build 3T: Tasks page on views; formulas, dashboards, outside data

Views shared and pinned, the views engine on collections, the Tasks page on views, aggregates and charts, dashboards, the formula language, outside data.

- **Slices:** 14 (2 S, 11 M, 1 L), from T1.
- **Starts after:** build 1T, build 2W.
- **In this order:** T1.6 Personal, shared, pinned, default views; T1.13 Web views engine on collections, adapters, `FilterEditor`; T1.8 Task saved views into the one store; T1.14 Timeline and list kinds for every collection; T1.9 Task lists, `task_list`, automations on the compiler; T1.11 Step 4: edit Tasks like any collection; T1.15 Tasks page on views: list (with W1.27a, W1.27d); T1.29 Public form limits; T1.31 Aggregates and chart view kind; T1.37 Record comments slot; T1.16 Tasks page on views: board and timeline (with W1.27b); T1.32 Dashboards module; T1.17 Formula language, same-record recalculation; T1.34 Outside data: connections, saved queries.

#### Build 3W: Web: live on protocol 2, phone, offline queue

Per-event handlers, the live client on protocol 2, space routes and thing addresses, phone navigation, the outbound queue, the request card and run view, the shape walk test.

- **Slices:** 12 (1 S, 10 M, 1 L), from W1.
- **Starts after:** build 1T, build 2A, build 2E, build 2G, build 2O, build 2W.
- **Stopgap unless it has landed:** W1.39 on K4.25 (build 3A); W1.39 on C2.40 (build 3C).
- **In this order:** W1.3 Per-event handlers, no project-wide refetch; W1.7 Recorded event streams; W1.12b Pages off AppShell: tasks, views, runs, pages folders; W1.39 Shape walk test; W1.24 Request card and run view; W1.6 Live client on protocol 2; W1.9a Tasks onto the op client; W1.12c Pages off AppShell: the rest, delete workarounds; W1.14b Space routes and thing addresses; W1.20 Calendar and gallery pieces (timeline is T1.14); W1.29 Phone navigation; W1.30 Outbound queue for any queueable operation.

### Stage 4: Complete, hosted and connected

**Done when:** The eleven-point test for done (END-STATE) is walked end to end by the harness. The hosted profile starts only with every cost metered; plans, checkout and budgets work; strict row rules are on.

#### Build 4C: Chat complete

Channel details and tabs, make a page, record, row or sticky from messages, presence and @here, answers with sources, agents in the room, request cards, saved inbox views, phone flows, speed.

- **Slices:** 12 (4 S, 8 M), from C1, C2.
- **Starts after:** build 3C, build 3E, build 3W.
- **In this order:** C2.8 Channel details and tabs (part `channel.tabs`); C1.26 A page tab on a channel (`conversation_tabs`); C2.20 Make from messages: registry and chip; C2.21 Make a page or a record; C2.22 Make a sheet row or a sticky; C2.11 Saved inbox views; C2.16 Presence, typing, @here (K5's presence); C2.27 Answers with sources; C2.29 Speed; C2.31 Phone flows; C2.35 Agents in the room; C2.12 Request cards.

#### Build 4D: Documents complete

Validation and charts in sheets, page sharing levels and public links, live cursors, element comments, history on every document, files from an outside source, moving a page between spaces.

- **Slices:** 13 (1 S, 1 S/M, 11 M), from C1.
- **Starts after:** build 3A, build 3C, build 3E, build 3O, build 3T.
- **In this order:** C1.28 Documents in events and search; C1.10 Recent-edit outlines in the editors; C1.15 Validation and dropdowns; C1.18 Charts; C1.27 Page sharing levels and public links; C1.29 Board images through the storage seam; C1.30 Live cursors, selections and follow; C1.31 Element comments and ask about a selection; C1.32 Stickies into tasks, a board into a page; C1.34 History on every document via K3's seam; compaction; C1.36 Files from an outside source, by reference; C1.37 Move a page between spaces; C1.35 Document-kind contract test.

#### Build 4G: Agent plugins, memory, consent and ambient mode

Following and ambient mode, scope and consent screens, memory pages, the Hermes and OpenClaw plugins, the bridged connector's runs and offers, the team guard.

- **Slices:** 7 (7 M), from A2.
- **Starts after:** build 2A, build 3G.
- **Stopgap unless it has landed:** A2.16 on C1.27 (build 4D).
- **In this order:** A2.9 Following and ambient mode; A2.15 Scope, requested-by and app consent; A2.16 Memory pages; A2.23 Hermes plugin; A2.24 OpenClaw plugin; A2.26 Bridged connector: runs, offers and actions; A2.27 Team guard.

#### Build 4H: The hosted service

Spend limits and budgets, checkout and plans, strict row rules, noisy-neighbour controls, directory sync, the staff review queue, the hosted operations profile, upgrades and releases.

- **Slices:** 13 (6 S, 7 M), from A2, H1.
- **Starts after:** build 3E, build 3G, build 3H, build 3R.
- **In this order:** H1.27 Spend limits per account, space, conversation; H1.63 Strict row rules; A2.14 Budgets (rows in H1's spend limits); H1.11 Lossless round-trip guard; H1.23 Noisy-neighbour controls; H1.32 Directory sync; H1.34 Checkout, trials, downgrades, dunning; H1.36 Staff console: review queue, metrics, support access; H1.37 Hosted operations profile; H1.43 A workspace's own bucket: screen and plan rule; H1.15 Startup under a lease, upgrades, releases; H1.30 Sign-in with common providers; H1.38 Hosted agent service seam.

#### Build 4T: Records act like tasks

Relations and rollups, validation, file and button fields, triage, cycles, initiatives, forms, templates and recurring records, automations on records, delegation, the keyboard-first pass.

- **Slices:** 19 (4 S, 14 M, 1 L), from T1.
- **Starts after:** build 2E, build 3C, build 3G, build 3T.
- **Stopgap unless it has landed:** T1.36 on C1.27 (build 4D).
- **In this order:** T1.22 Delegate; T1.33 Sheets as a dashboard source; T1.12 Step 5: `fields` and `collection` on task tools; T1.13a The field registry (from W1.16); T1.18 Two-way relations, rollups, recalculation jobs; T1.19 Validation, required, defaults, builder-only; T1.20 File field; T1.21 Button field; T1.23 Triage queue; T1.24 Cycles; T1.26 Initiatives (containment by `parent_id`); T1.28 Forms: definitions, internal, conditions; T1.30 Record templates and recurring records; T1.35 Reviewed queries, shared results; T1.36 Fields on pages; page lists through views; T1.38 Automations on records; T1.41 Records and tasks proposable, with K3 history; T1.25 Step 6: acts like a task; T1.27 Keyboard-first pass.

#### Build 4W: Web on operations and pieces everywhere

Every page onto the op client and the shared pieces, the panel renderer and framed panels, built modules in the app, package upgrades, offline reading, speed in CI.

- **Slices:** 19 (2 S, 17 M), from M1, W1.
- **Starts after:** build 3G, build 3O, build 3T, build 3W.
- **Stopgap unless it has landed:** W1.22 on T1.13a (build 4T); W1.28 on C2.11 (build 4C).
- **In this order:** M1.15 Framed panels; W1.14c Internal links through href helpers; M1.3k Kinds, events, search from registrations; M1.7 Built modules in the app; M1.10 Package upgrades; W1.9b Conversations, inbox, later onto the op client; W1.9c Pages, sheets, whiteboards onto the op client; W1.9d Collections, fields, views, forms onto the op client; W1.9e Runs, launch, agents onto the op client; W1.9f Settings, team, spaces, onboarding onto the op client; W1.22 Form piece (filter builder is T1.13); W1.25 Panel block renderer; W1.26 Embeds through registrations and pieces; W1.27c Task detail onto the detail view; W1.28 Other lists onto the list piece; W1.31 Offline reading (opt-in per person, workspace can switch off); W1.33 Speed in CI; W1.34 Test diet; M1.16 Agent-creation offers, panels and actions end to end.

### Stage 5: Clean-up and release

**Done when:** The old paths are deleted (no release in between is needed, R51). The whole build is rehearsed on a copy of the live database and then deployed once. Trust documents are published and the licence is chosen (R25).

#### Build 5X: Remove the old paths and release

Every plan's contract slice: old tables, edges, dispatchers and redirects deleted. Then the deploy rehearsal on a copy of the live database, the one deploy, and the trust documents and publication.

- **Slices:** 17 (11 S, 6 M), from K1, K2, K3, K4, K5, T1, A1, A2, M1, C1, C2, W1, H1.
- **Starts after:** every other build has merged (R51: no release in between; old names that outside clients use keep their aliases until their sunset, counted from the deploy).
- **In this order:** K1.31 Contract: delete the old paths; K2.22 Contract: delete hand-written edges (per batch); K2.23 Alias sunset and public v1 retirement; K3.22 The outbox retires; K4.22 The flip (base tables renamed, old names as views); K5.15 Remove the old paths; T1.39 Contract: remove the second task engine (deletes W1.27a/b/d's old screens); A1.22 Contract; M1.3o Operations and permissions from registrations; C1.6 Retire the old comment tables; C2.7b Remove membership fallback; W1.6b Remove the old dispatcher; W1.14d Delete legacy redirects; T1.40 Contract: drop `saved_views`; A2.28 Contract the old paths; K3.23 Contract the old link stores; H1.40 Trust documents and publication (licence chosen at release, R25).
<!-- /gen:stages -->

### Parked, with a reason

Not in any build, as in the wave order: **C2.38** shared channels (a sketch only, later),
**C2.33** offline reading in C2 (W1.31 does it; waits for the owner's agreement), **K5.16**
one owner per document (deferred by K5 until measurements ask for it).

## 5. The journey: the proof at the end of stage 2

The foundations are only worth building on if they work together. So stage 2 is done when
one automated test, `tests/integration/journey.rs`, passes from start to finish (R47).
Build 1G creates the file with its first step; each later build turns on the steps its
slices make possible; build 2G owns making all of it pass.

| Step | What happens | Made by |
| --- | --- | --- |
| 1 | An outside agent connects with one credential and takes a run through operations | A1.1, A1.3, A1.4 (stage 1) |
| 2 | The same agent, one account, is a member of two workspaces | K1.5, K1.6, K1.22, K1.23, K1.33 |
| 3 | It receives an event from each workspace on one stream, and by webhook | A2.1, A2.2, A2.3, K3.24 |
| 4 | It edits a task through an operation. The same operation from the browser and from MCP gives the same result and the same line in the audit trail | K2.2, K2.3, K2.13, K1.25 |
| 5 | It raises a request. An account that holds the permission answers it: once a person, once another agent | A1.7, A1.8 |
| 6 | It sends the same create twice and one thing exists. A run start whose answer was lost is retried and runs once | P0.15, P0.16, A1.3, A1.12 |
| 7 | Its membership in one workspace is revoked while a stream is open, a delivery is queued and an operation is waiting: the stream closes for that workspace, the delivery is dropped, the operation is refused. The other workspace carries on | K1.20, K5.8, A2.1, A2.2 |
| 8 | A request answered after it changed, or after the requester lost access, is refused | A1.8 |

The example host and its conformance tests (A1.23) and the contract suite for connected
agents (A2.7) are built in the same build as the protocol they test, not later.

If the journey shows that an abstraction is wrong, fixing it comes before any stage 3 build
merges, and the lead rewrites the affected plan sections and builds with the reason
(section 9).

## 6. Milestones a person notices

<!-- gen:milestones -->
| After stage | What a person notices | The slices that make it |
| --- | --- | --- |
| 0 | Private things stay private | P0.0, P0.36, P0.37, P0.38 |
| 1 | The app feels settled: right unread counts, DMs and thread replies always reach you, no reload between pages | C2.3, C2.5, W1.11 |
| 1 | Your own agents edit directly, at any role, Admin and Owner included | K1.9, K1.10, K1.12, K1.13, K1.14a |
| 1 | A connected outside agent takes a run and finishes it through operations | A1.1, A1.3, A1.4 |
| 2 | Workspace addresses /w/<slug> and a switcher with unread marks | K4.5, W1.14a |
| 2 | One agent in all your workspaces, with one DM with you everywhere | K1.5, K1.6, K1.22, K1.23, K1.33, K4.23, K4.24 |
| 2 | Agents are woken for tasks, comments, requests and reminders, by webhook or stream, with a contract suite and an example host | A2.1, A2.2, A2.3, A2.4, A2.5, A2.7, A1.12, A1.23 |
| 2 | Every hosted-cheap guard of R24 is on | K5.17, K5.18, K5.19, H1.41, H1.42 |
| 3 | A workspace with only Chat on is whole | K4.10, K4.26, M1.17, C2.40, K4.25, W1.39, C2.39 |
| 3 | Sensitive requests answered by whoever holds the permission, person or agent | K1.14b |
| 3 | Add an agent three ways: from the directory, by pasting a link into one you run, or on a host you have linked, with the host's steps drawn in the app | A2.18, A2.19, A2.20, A1.11, A1.12, M1.14 |
| 3 | Open sign-up with a verified email, asking what the workspace is for | H1.28, H1.14 |
| 3 | Live everywhere on one protocol with resume; phone navigation; offline writes queue | W1.6, W1.29, W1.30 |
| 4 | A host's own framed panels, checked end to end | M1.15, M1.16 |
| 4 | The hosted service can open: every cost metered, plans and checkout, budgets, files from outside sources | H1.26, H1.33, H1.34, H1.37, A2.14, C1.36 |
| 4 | Records act like tasks; dashboards and outside data | T1.17, T1.18, T1.25, T1.32, T1.34, T1.41 |
| 5 | Old paths gone; release with trust documents | K1.31, K2.22, T1.39, A1.22, H1.40 |
<!-- /gen:milestones -->

The stage shown is worked out from the slices named, so it can't disagree with section 4.

## 7. What can start now

Build 0, and beside it every stage 1 build: 1A, 1E, 1H, 1O, 1T, 1C and 1G. A stage 1 build
that starts before build 0 has merged does its slices that name a P0 slice last, after
merging `main`. Build 0 is 39 small fixes and should be the first to land. Inside build 0,
P0.0 (DM thread names in the project sidebar) is the first commit.

How many builds run at once is a choice about cost and machines, not about the plan: fewer
at once changes the calendar and nothing else.

## 8. Rules between builds

- **Merge order inside a stage** is whatever finishes first. The build that merges second
  merges `main` and fixes what it finds. Shared registries (the tool list in `mcp.rs`,
  routes in `api.rs` and `App.tsx`, `policy_inventory.rs`, `ui/index.ts`,
  `registry.json`) take new lines next to related ones and are never reordered.
- **Migrations** use UTC timestamps; each build takes its timestamps from the hour it
  starts, so two builds never collide.
- **Stopgaps.** Each build's "Stopgap unless it has landed" line names the soft needs that
  may not be there yet. A stopgap is removed by the next build in the same lane, which says
  so in its pull request.
- **Row security call sites** (H1.45 to H1.62, one server module directory each): each
  hosting build picks, when it starts, the directories that no sibling build of its stage
  is rewriting. H1.44's count of unscoped calls may only go down, so new code is scoped
  from the start.
- **Splitting and merging builds.** The lead may split a build that proved too large or
  merge two that proved small. That is an edit to `slices.json`, a run of `tools/plan.py`,
  and a line in the plan changes log (section 9).
- **The cycles the wave order broke stay broken the same way** (its section 9, R43): A1.11
  goes before K1.7 and K1.23 with an interim hashed host token and stored notices; T1.22
  follows A1.13; H1.13 goes before A1.17; K1.30 goes before H1.18 and does not need it;
  H1.20 goes before K2.3 with a plain path scope; W1.39 lands its Friends walk marked to
  wait for C2.40. K4.24 owns DM storage and the one-per-pair rule, C2.39 the policy and
  screens; K4.11 owns the private-channel rule, C2.6 the screens.

## 9. Keeping the plan honest

- **One list.** `slices.json` holds every slice's build, size and needs. `tools/plan.py`
  refuses a slice with no build or reason, a need that doesn't exist, a hard need that
  lands later or in a sibling build of the same stage, a cycle, a milestone that names an
  unscheduled slice, and any difference between the list and the slice tables in `plans/`.
  It then writes `SLICES.md` and the generated parts of this file. Counts are computed, not
  typed. P0.30 puts `slices.json` and `tools/plan.py` in the repository with the plans
  (`docs/plans/master/`) and adds the check to CI.
- **Measured, not estimated.** Each build's task records slices done, sessions used, CI
  runs and review rounds. After stage 1 the lead writes what a build really costs and sizes
  the rest from that.
- **Gates need a working demonstration.** A stage is done when its "Done when" line is
  shown working: the test passing, or the flow walked in a browser. A gate that fails is
  fixed before anything later merges.
- **Later stages are a forecast.** Stages 3 to 5 are cut again at the stage 2 gate. Any
  change to a build after it was first written (a slice moved, a build split, a need
  changed) is recorded in `PLAN-CHANGES.md` with the date and the reason.
- **Rehearsed at every gate** (R51). Nothing is deployed until build 5X. At each stage
  gate from stage 1 on, the latest live backup is restored into a scratch database, every
  migration and the new server run against it, and counts, read checks and the main pages
  are checked. A failed rehearsal is fixed before the next stage merges.
- **Hosting numbers are targets until measured** (R49). The free plan's shape and the idle
  cost are checked against the meters (H1.25, H1.26) and the idle-cost guard (H1.42) before
  anything is promised in public.

## 10. What moved from the wave order

Everything else kept its relative place. These moved, each because the dependencies allow
it or need it:

- **Earlier, to prove the design sooner:** A1.4 connected runners get runs (wave 8 to
  build 1G); A1.5 and A1.6 the runs page and reads, A1.23 the example host and conformance
  suite (wave 11), A2.7 the contract suite and A2.8 mentions by anyone (wave 7), all to
  build 2G; K1.12 and K1.14a, agents editing directly and holding any role (waves 4 and 5
  to build 1A).
- **Earlier, because nothing held them back but session count:** K2.8, K2.9, K2.12, K2.14,
  K2.15, K2.18, K2.21 (waves 11 and 12 to build 2O); K2.19, K2.20 (to 3O); T1.4b, T1.4c,
  T1.4d, T1.10 (to 1T); C1.7 (to 2C); K5.10, K5.14 (to 2E); K4.18, K1.30 (to 2A); K5.20,
  K3.9, K3.16, K3.25 (to 3E); K4.17, K4.19 (to 3A); H1.15, H1.30, H1.38 (to 4H); T1.25,
  T1.27 (to 4T); C1.35 (to 4D).
- **Later, because a hard need is in a sibling build:** T1.6, T1.8, T1.9, T1.13, T1.14 (to
  3T, after K1.9 and the web pieces); M1.12 (to 3O, after K1.22); C2.17 (to 3C, after
  K1.28); W1.37 (to 2W); the fillers T1.22, T1.33, C1.26, C1.28, C2.21, C2.22 (to stage 4).
- **Into another lane's build:** K3.24 the direct scope in the log (in 2A, right after
  K4.23); A2.14 budgets (in 4H, as rows in H1.27's spend limits); M1.3k, M1.7, M1.10,
  M1.15, M1.16 (in 4W, with the web work they need).
- **Needs changed:** K3.11 registers every kind whose table exists on its branch; the `run`
  kind's one registration line is added by whichever of 1E and 1G merges second. K1.30 no
  longer names H1.18.
- **"Add an agent three ways"** is a stage 3 milestone made of slices that are all in stage
  3 (A2.18, A2.19, A2.20, M1.14). A host's framed pages and the end-to-end panel check
  (M1.15, M1.16) are named as their own stage 4 milestone.
- Pull request #262 has merged, so T1.3 waits on nothing outside the plan.

## 11. Hot files

Files that more than one build of the same stage changes. The build that merges second
merges `main` and keeps its edit small.

<!-- gen:hot -->
| Stage | File | Builds that change it | Slices |
| --- | --- | --- | --- |
| 1 | `App.tsx` | 1A, 1O | K4.10, W1.10, W1.13 |
| 1 | `api.rs` | 1A, 1E, 1O | K2.3, K4.2, K5.2 |
| 1 | `auth/mod.rs` | 1A, 1O | K1.2, K1.5, K2.1 |
| 1 | `conversations.rs` | 1A, 1C, 1G | K4.4, A1.2, C1.2, C1.3, C2.5 |
| 1 | `external_agents.rs` | 1A, 1G | K1.6, K4.10, A1.2 |
| 1 | `lib.rs` | 1A, 1E, 1H, 1O | K1.2, K2.3, K4.2, K5.1, K5.2, H1.12 |
| 1 | `main.rs` | 1A, 1E, 1H | K1.2, K5.1, H1.12 |
| 1 | `mcp.rs` | 1A, 1E, 1G, 1O | K1.2, K1.5, K1.6, K1.14a, K2.1, K2.3, K4.2, K4.10, K5.2, A1.2 |
| 1 | `modules/gate.rs` | 1A, 1O | K4.10, W1.13 |
| 1 | `registry.json` | 1A, 1O | K4.10, W1.13 |
| 1 | `work_items.rs` | 1A, 1C, 1G, 1T | K4.4, T1.4a, A1.2, C1.1 |
| 1 | `workspace_isolation.rs` | 1A, 1G | K4.2, A1.3 |
| 2 | `conversations.rs` | 2A, 2G, 2O | K2.7, K4.11, K4.12, A1.8 |
| 2 | `direct.rs` | 2A, 2E, 2G, 2O | K1.33, K2.7, K4.11, K4.12, K4.15, K4.23, K4.24, K5.12, A1.8, A2.4 |
| 2 | `notifications.rs` | 2A, 2G | K1.20, K4.13, K4.15, K4.23, A1.8, A2.4 |
| 2 | `outbox.rs` | 2A, 2E | K4.7, K4.13, K5.8 |
| 2 | `realtime.rs` | 2A, 2E | K4.7, K4.13, K5.8 |
| 2 | `workspace_isolation.rs` | 2A, 2E | K1.33, K4.24, K5.12 |
| 3 | `conversations.rs` | 3C, 3E | K3.13, C2.39 |
| 3 | `direct.rs` | 3C, 3E | K3.13, C2.39 |
<!-- /gen:hot -->

`tests/integration/main.rs` and `workspace_isolation.rs` change in almost every build: add
test modules and cases next to related ones, never reorder.

## 12. Decisions that hold builds up

Owner questions from the plans, by the first build they touch. Each has the planner's
recommendation; unanswered ones proceed on it ("assumed") and are recorded in
`docs/plans/master/decisions.md`.

<!-- gen:decisions -->
| Build | Question | Slices | Recommendation |
| --- | --- | --- | --- |
| 0 | Is a role's `modules_off` a permission or only a view? (P0 Q1) | P0.7, K1.11 | A permission |
| 0 | Is any database still from before migration 0010? (P0 Q2) | P0.25 | If not, retire the five replays |
| 0 | May research files go into the repository as they are? (P0 Q3) | P0.30 | Only end state and plans now; research after stripping owner detail |
| 0 | Keep `tools/ci/run.sh` here? (P0 Q4) | P0.33 | Yes, hosts from an untracked file |
| 0 | Build new task view features only on the collections side until T1 merges the engines? (P0 Q5) | P0.26 rule 4, T1.1 | Yes |
| 1A | Does a retired workspace slug stay reserved? (K4 Q3) | K4.5 | Yes |
| 1A | A separate "comment" permission? (C1 Q2) | K1.9, C1.3 | Yes, given wherever edit was |
| 1A | May members read an open team space without joining? (K4 Q2) | K4.4, K4.7 | Yes, per space |
| 1A | `#general` in new workspaces? (K4 Q5) | K4.10, C2.40 | Yes |
| 1C | PDF through a library in the server? (C1 Q4) | C1.23 | Yes |
| 1C | DMs push by default; replying follows a thread? (C2 Q1, Q2) | C2.5 | Yes; yes with push off |
| 1E | How long is the event log kept? (K3 Q1) | K3.2, K3.25 | Facts 400 days, signals 7, history for ever |
| 1E | PostgreSQL notifications rather than a broker? (K5 Q2) | K5.4 | Yes |
| 1O | One envelope for old tool names too? (K2 Q2) | K2.3 | No; old names keep old shapes |
| 1O | A built module in one space and also workspace-wide? (M1 Q1) | M1.6 | Both, default the space |
| 1T | The task key stays on the task core? (T1 Q6) | T1.3 | Yes |
| 1T | Labels per project or per workspace? (T1 Q1) | T1.10 | Per project, opt-in per collection |
| 2A | May an agent's managers read its personal space? (K4 Q1) | K4.9 | Yes, read-only |
| 2E | Two instances on the live server after K5.7? (K5 Q1) | K5.7 | Yes, behind the existing proxy |
| 2G | Who answers built-in approvals by default? (A1 Q2) | A1.8 | Owners and admins (`runs.approve`) |
| 2G | The built-in host's store in the same PostgreSQL? (A1 Q4) | A1.14 | Yes, own schema |
| 2G | Default chain depth and turn allowance (A2 Q2) | A2.8 | 4 and 6 |
| 2H | Who may create workspaces on a self-hosted server? (H1 Q5) | H1.24 | A server setting, operators only |
| 2H | The free plan's numbers (H1 Q1) | H1.25, H1.27, H1.33 | H1 3.8's table |
| 2W | Words in addresses (`tasks`, `runs`, `chat`, `s/<key>`)? (W1 Q3) | W1.14a | Yes, with the workspace change |
| 3C | Bottom bar on the phone? (W1 Q4) | W1.29, C2.40 | Yes |
| 3E | Pasted URLs with a thing's id count as references? (K3 Q2) | K3.13 | Yes in the same workspace |
| 3E | Agents see others' activity in feeds? (K3 Q3) | K3.20 | Yes, same access rule |
| 3H | Git history at release (H1 Q3) | H1.39, H1.40 | Scan, then rewrite infrastructure strings once |
| 3O | Connected modules from any URL on hosted? (M1 Q2) | M1.12 | Any URL, with preview and outbound check |
| 3T | Formula syntax; per-field permissions (T1 Q2, Q3) | T1.17, T1.19 | Yes; builder-only, no hidden fields |
| 4C | `@here` means active now? (C2 Q5) | C2.16 | Yes |
| 4D | Public page links on hosted, on by default? (C1 Q3) | C1.27 | Off by default |
| 4H | Staff impersonation? (H1 Q4) | H1.36 | None; support through an invited guest |
| 4H | Hosted infrastructure separate from the owner's server? (H1 Q7) | H1.37 | Yes |
| 5X | Alias window; public v1 retirement (K2 Q1, Q4) | K2.22, K2.23 | 180 days and 30 days unused, counted from the one deploy (R51); public v1 retired 12 months after v2 is live |
| 5X | Sunset for old agent delivery formats (A2 Q1) | A2.28 | 2027-06-01 or 30 days unused, the later |
| 5X | Licence (R25, H1 Q2) | H1.40 | Chosen at release |
<!-- /gen:decisions -->

Already decided, so not blocking: the default agent role and "Member, suggests" (R5), who
manages an owned account (R34), who approves `agent_register` (R45, which replaces R38's
wording), `conversation_readable_by` as a one-call wrapper (R32), invitation link tokens are
K4's (R30, K4.26), client config is H1's (R30, H1.41). R31 says reaching someone you share
no workspace with does not exist and needs a ruling before anyone adds it; no slice adds it.

## 13. Integration with Agent OS: outside this order

Nothing in this build order waits on Agent OS (R21). The work that needs changes inside
Agent OS is in A2's appendix "Integration, with Agent OS" and is done later, in the separate
three-repository project:

- native mode (runs and identity owned by Agent OS rather than bridged);
- an agent's identity kept across moves through the host's lineage (until then, each move
  needs an account that manages the agent to confirm it, through K1.7's rebind);
- approvals relayed into Agent OS's own approval system;
- per-run credentials inside Agent OS;
- Agent OS creating agents from Workspace;
- the two permission systems shown side by side.

What stays in the order and needs no Agent OS change: the host protocol and client library
(A1.11, A1.12), the built-in host (A1.14 to A1.17), the example host and conformance suite
(A1.23), offers, `agent_register`, actions and notices (A2.19), and the generic bridged
connector, which runs beside an unchanged Agent OS through its public controller interface
(A2.25 in build 3G, A2.26 in build 4G). A2.21 (the agent builder) is removed, not deferred:
a builder is a host's offer (R21).
