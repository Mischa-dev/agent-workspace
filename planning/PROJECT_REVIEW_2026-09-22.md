# Workspace project review and future plan

September 22, 2026. This is a point-in-time review of the current checkout, the two UI worktrees, the hosted app, and read-only Red Hat and Nyx service status. It is a plan, not an instruction to deploy, start agent work, publish code, or change access.

## The actual future roadmap lives in Workspace

[Workspace: build the future](https://workspace.mischanelson.dev/projects/d00f7995-1b57-4417-b6de-a61b603b135f) is now a real project in the hosted app with ten draft initiatives. They are intentionally big bets: an intelligent task system, Workspace building its own next versions, agent crew mission control, living knowledge, shared creative rooms, an artifact studio, cross-app action, an everywhere experience, cheap reversible experiments, and an extension platform. Each task has a product outcome. None has been assigned or started.

The task system is good enough to hold and develop this future right now. It has Projects, Outline/Board/Dependencies views, parent tasks, Resources, agent assignments, and bounded work sessions. It is not yet the self-building engine in those draft tasks. The engineering items below describe what exists and what will help ship the larger bets. They are not a demand to spend the next phase writing test tickets before exploring ideas.

## The current state

Workspace is already a usable private collaboration app. Messages, versioned Resources, Spaces, Projects, Tasks, scoped agent accounts, MCP/HTTP, approvals, bounded work, and recovery all have working implementations. The hosted app responds at `workspace.mischanelson.dev`; `/health` reported the worker online and dispatch enabled. Red Hat's app and backup timer were active, the last backup unit result was successful, and the five Nyx crew timers were active with successful service results. The hosted Agents view showed recent check-ins from Muse and the five crew members, and their conversation previews showed replies. A check-in or reply preview alone does not prove every agent's model, tool, handoff, and failure path.

The hosted interface already includes the UI overhaul's compact rail, URL navigation, search, themes, read state, message history paging, drafts, and detail panels. The older screenshots in `planning/UI_OVERHAUL_AUDIT.md` describe the pre-overhaul UI. They remain useful history, but they are not a picture of today's hosted interface. The later implementation record is `planning/UI_OVERHAUL_STATUS.md` on `codex/workspace-ui-overhaul`.

Source state needs attention before the next release. This checkout is at `2128a0f` on `codex/private-v0`, with uncommitted persistent-credential changes and an untracked Nyx bridge and UI plan. `codex/workspace-ui-overhaul` is clean at `4994c42`, four commits ahead. `codex/ui-fixes` is another worktree at `4994c42` with uncommitted reliability, Markdown, and visual fixes that are not deployed. Local `main` is still at `bd2c901`, and this repository has no Git remote configured. The deployed source folder has no Git metadata. Selected hosted source hashes match the UI worktree and selected credential files match this checkout, but the hosted asset hash matches neither checkout's clean build. The exact deployed source is therefore not represented by one current commit.

## Keep and maintain

| Area | Evidence and judgment | Near-term action |
| --- | --- | --- |
| Core record model | Conversations, requests, Resources, tasks, revisions, and results remain distinct. Existing tests cover permissions, idempotency, claims, Stop, dependencies, and recovery. | Preserve these contracts. Add focused tests only when changing them. |
| Agent access and transport | Public OAuth/MCP and HTTP have prior live protocol qualification. Current credentials are per agent and revocable. | Keep the common domain rules and configurable client connections. Qualify actual client behavior separately. |
| Hosted deployment | App, worker, PostgreSQL, Cloudflare route, daily backup timer, and isolated restore have evidence. Current public health passed. | Maintain and observe. Do not redesign hosting to solve UI or agent workflow gaps. |
| Local managed work | Real Codex conversations, isolated worktrees, two overlapping code runs, bounded tools, and recovery were previously qualified on the Mac. | Keep the bounded local profile. A remote server does not inherit its runtime or repository access. |
| UI foundation | The released shell, search, routing, themes, favorites, read state, drafts, and paging are substantial progress. The current hosted screen is more compact and readable than the pre-overhaul capture. | Finish reliability and accessibility on this foundation. A new frontend framework or visual restart is unnecessary. |
| AgentOS boundary | AgentOS owns agent declarations and execution. Workspace owns collaboration records and permissions. The Nyx bridge follows that split. | Keep this division when expanding the bridge. |

## Work that needs attention

| Priority | Problem | Why it matters | Finish condition |
| --- | --- | --- | --- |
| P0 | Reconcile release source and documentation | Three worktrees, uncommitted production changes, an old `main`, and no source commit in the deployed folder make future fixes and rollback harder to audit. `README.md` still says no public deployment exists; `deploy/AGENT_CONNECTIONS.md` still describes a 30-day refresh limit. The W backlog marks all 28 packages planned even where L packages and UI work now cover part of them. | One reviewed release commit contains the actual intended code and migrations; its image records commit/digest provenance; current docs and backlog distinguish delivered, partial, and future work. No remote push is implied. |
| P1 | Prove ordinary agent work end to end | Muse and crew check-ins are live, but the repo does not contain a repeatable per-client record of one real request, model execution, permitted tool or Resource use, answer, correction/Stop behavior, and recovery. Atlas and Ember still show old last-seen times. | A small matrix records each actual client separately, with request and runtime receipts, original conversation return, failures, and clear status for dormant identities. No test message is sent without a scoped test plan. |
| P1 | Finish the UI fix branch and release it safely | `codex/ui-fixes` addresses session hangs, stale snapshots, dialog races, raw Markdown, dense text, and confusing Inbox rows. It is not on the hosted app. | Review and integrate the fix branch with the credential changes; run full domain, transport, and UI checks against an isolated database; check a real hosted candidate and rollback; then record the exact promoted image. |
| P1 | Make tests reproducible from a checkout | The current checkout passed 81 tests after its dedicated PostgreSQL cluster was started. The clean UI worktree passed typecheck/build, but its local config points at a database port whose `.local/postgres` directory is absent, so its full suite could not be rerun in this review. `ui-fixes` passed 10 new focused tests and typecheck. | One documented, isolated test command prepares and tears down its own database without starting owner dispatch or relying on a stale private config. Every release candidate runs that full command. |
| P2 | Complete daily-use UI behavior | The UI release status still lists thread-first-reply drafts, continuous paging around linked old messages, structured mentions, mark-unread, notification choices, review dismissal, settings organization, and interaction component consolidation. | The journeys in `UI_OVERHAUL_PLAN.md` pass on populated desktop and phone states, including failed sends, reconnects, stale attachments, and long history. |
| P2 | Qualify accessibility and performance | The release has responsive viewport checks and a one-machine large-fixture measurement, but no complete keyboard, VoiceOver, contrast, real phone keyboard, or p95/INP evidence. The old and new stylesheets overlap. | Record keyboard and screen-reader journeys, contrast/zoom/reflow, real phone behavior, and measured reference-device latency. Consolidate styles and shared controls where tests show risk. |
| P2 | Complete coding delivery if it is still a product priority | Hosted managed coding is not configured; project tests, arbitrary shell, draft PR creation, checks/review sync, and combined branch verification are not available in the qualified profile. | Select an execution host and exact repository scope; run a bounded real project check; publish a draft PR only through a separately authorized scoped connection; tie checks and prerequisite evidence to exact commits. |
| P2 | Expand the Nyx bridge only for real needs | The bridge currently handles response requests and text replies. Assigned chat tasks and binary Resource downloads are documented limits; there is no dedicated bridge test suite in this checkout. | Add receipt and failure tests, then support assigned tasks and files if actual crew workflows need them. Preserve idempotency, lease renewal, and AgentOS authority. |
| P3 | Scale the data path from measured load | Commands still load all `records` into a transaction. The UI release bounds message snapshots and pages history, but one 10,000-message fixture is not a long-term load result. | Record realistic p95 query, snapshot, search, and render costs. Add targeted indexes and bounded domain queries where measurements show a problem. Preserve revision and permission semantics. |
| P3 | Strengthen routine operations | Backups and restore work, but off-server copying is manual and no retention rule exists. At review time there were 11 small backup directories, about 992 KB total, and 41 GB free on the Red Hat filesystem. | Add a retention and off-server copy policy that preserves recoverability, verifies checksums, reports failures, and periodically exercises an isolated restore. Document image and schema rollback together. |

## Recommended sequence

Start shaping the ten initiatives in the live project now. The release work in step 0 matters when a candidate is ready to promote; it does not block new concepts or disposable previews. The first two product bets to explore are the task system as Workspace's brain and a loop where Workspace helps build its own next version.

### 0. Establish one release baseline

1. Compare the deployed image and source with `4994c42`, the credential changes in `codex/private-v0`, and the pending `codex/ui-fixes` diff. Keep the UI release as the starting code line because it is already hosted. Resolve overlapping files deliberately. Do not discard either worktree's changes.
2. Run the full suite through an isolated database harness, typecheck, build, and a focused hosted candidate check. Record the exact commit, migration set, built asset hashes, image digest, and rollback image. Keep credentials and private runtime data out of Git and images.
3. Update `README.md`, `ACCEPTANCE.md`, connection instructions, release status, and the L/W backlogs from the verified result. Mark partial coverage instead of treating every W item as new work. Leave Git remote/publication as a separate owner choice.

### 1. Finish the connected daily-use loop

1. For Muse and each Nyx crew member, use one harmless, bounded request in a disposable conversation or other clearly marked test area. Capture check-in, claim, actual model task, any needed Workspace tool or Resource operation, answer in the source thread, and final status. Run one correction or Stop and one lost-response/restart case per integration type rather than duplicating every fault test for every person.
2. Make the Agents screen distinguish configured, recently checked in, completed a real round trip, stale, and failed. An expected five-minute poll is not a response-time guarantee. Decide whether Atlas and Ember get real hosted clients or remain clearly inactive examples.
3. Promote the reviewed UI fixes with a production-safe backup and rollback path. Verify sign-in, message send/retry, Inbox, Resource rendering, permissions, and narrow layout against the hosted candidate.

### 2. Complete the interface as one product

1. Finish the messaging edges first: thread first-reply composer, old-message newer-page continuity, reliable attachments and drafts, IME/double-submit/reconnect behavior, and precise agent progress.
2. Add structured mentions, mark-unread, notification preferences, durable review/update dismissal, and a dedicated settings structure. Keep unread, review, and running-work counts separate.
3. Extract shared menus, dialogs, fields, panels, and status controls from the large TSX files. Merge legacy and overhaul CSS as screens migrate. Preserve the current palettes and layout.
4. Run the complete keyboard, screen-reader, contrast, phone, and performance gate in `UI_OVERHAUL_PLAN.md`. Use populated projects, long names, attachments, and failure states, not only empty screens.

### 3. Choose and complete the next major capability

The two largest remaining product tracks are hosted coding delivery and deeper AgentOS participation. They can be designed independently after the release baseline, but each has an external authority boundary.

- Coding delivery: bounded execution host and project check profile, exact repository authorization, local result review, draft PR publication, commit-specific CI/review synchronization, and prerequisite branch combination checks. Keep merge and deployment under a separate owner decision.
- AgentOS participation: versioned identity mapping, task and file coverage in the bridge, controller contract tests, real tool-backed task evidence, and a small connection/status view. AgentOS remains the source of truth for runtime configuration and supervision.

### 4. Improve scale and operations when use justifies it

Measure normal and larger workloads before replacing the record store. Add query/index changes in small migrations, then recheck access and revision rules. Set backup retention, copy verification, alerting, and restore cadence. Track dependency and security reviews before expanding repository execution or adding more human users.

## Possible additions, not current prerequisites

These are valid future choices once the daily loop is reliable: decision and constraint records; generated conversation briefs with source links; provenance-aware shared memory; cross-device drafts; push notifications; more human collaborators; a replaceable model-assisted speaker selector for genuinely ambiguous messages; richer project scheduling and alternatives; and a plugin/automation designer. Each should answer a demonstrated use case and have its own privacy, permission, and failure contract. None should delay fixing release provenance or the current agent and UI journeys.

## Review evidence and limits

- Current checkout: `./workspace typecheck`, `./workspace build`, and 81/81 tests passed. Its database was started only for the test and returned to the stopped state.
- Clean UI worktree: typecheck and build passed in this review. `planning/UI_OVERHAUL_STATUS.md` records 83 earlier passing tests and the UI fixture work. Its full suite could not be rerun here because its dedicated PostgreSQL data directory is absent.
- Pending UI fixes: typecheck, `git diff --check`, and 10/10 new focused tests passed in this review. `planning/UI_FIX_VERIFICATION.md` records a broader fixture/browser pass; the fixes are not deployed.
- Hosted app: public health passed; the signed-in desktop Messages and Agents screens were inspected read-only. Red Hat service/timer status and Nyx timers were checked without sending messages or starting tasks. No new model run, physical phone test, restore, or credential change was performed for this review.
