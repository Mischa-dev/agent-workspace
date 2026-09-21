# Longer-term roadmap for the connected workspace

September 21, 2026. This is the next implementation plan after the verified local v0. It incorporates the owner's request for reusable connections and useful defaults. It does not start implementation, install connections, or activate schedules.

This document preserves the broader 28-package roadmap. Its all-packages release gate is deferred. [NEXT_IMPLEMENTATION_PLAN.md](NEXT_IMPLEMENTATION_PLAN.md) defines the smaller active launch scope; do not use this document as a prerequisite list for that launch.

## Outcome

Mischa can add agents, talk naturally with them, let them hand work to one another, share durable material, build substantial roadmaps, and review completed work in one application. Agents can participate immediately, periodically, or manually through the same records and rules. AgentOS-managed agents fit this system through a versioned adapter.

Ship sensible editable presets. Connecting another client on a supported protocol must require configuration, not a new product type in the application. A proprietary runtime may need adapter code; that code must not redefine messaging, permissions, task readiness, or memory.

Keep the existing React/TypeScript, Fastify, PostgreSQL/Drizzle, pg-boss, and local storage architecture. Extend the working application in place. NANC and the earlier Agent Workspace remain separate projects.

## Plan and evidence ownership

- [PRODUCT_PLAN.md](PRODUCT_PLAN.md) defines the product and fixed boundaries.
- [AGENT_PARTICIPATION.md](AGENT_PARTICIPATION.md) explains the external participation design.
- This file defines the next delivery sequence and release gates.
- [LONG_TERM_BACKLOG.json](LONG_TERM_BACKLOG.json) is the implementation work list with stable IDs, dependencies, code areas, and acceptance criteria. Its tasks are planned, not approved execution scopes or completed features.
- [ROADMAP.md](ROADMAP.md) retains the original A/B/C acceptance scenarios. They remain useful requirements; this plan replaces its obsolete instruction to implement only the first messaging slice.
- [ACCEPTANCE.md](../ACCEPTANCE.md) records observed results. A planned acceptance criterion is never evidence that it passed.

## Starting point and work to preserve

The September 21 local release records real Codex conversations, independent group responses, Resources, reviewed task proposals, bounded task execution, parallel isolated worktrees, existing GitHub PR links, and backup/restore. It records 52 passing tests plus build, typecheck, and browser checks. These are the previous release's results; this planning pass did not rerun them.

Only local Codex is currently qualified. Current code has a single runtime instance, owner-authenticated HTTP commands, and run-scoped internal collaboration tools. It does not yet expose external agent participation or MCP. The generic record store loads all records and serializes commands with a global advisory lock. The task model still includes a simplified parallel flag. These shortcuts need targeted replacement before large roadmaps and multiple external participants can be called supported.

Current source anchors are [server.ts](../apps/server/src/server.ts), [worker.ts](../apps/server/src/worker.ts), [store.ts](../apps/server/src/domain/store.ts), [commands.ts](../apps/server/src/domain/commands.ts), and [shared types](../packages/shared/src/index.ts). AgentOS mapping is based on read-only inspection of its [controller](../../AgentOS/docs/CONTROLLER.md), [task](../../AgentOS/docs/TASKS.md), and [declaration](../../AgentOS/docs/DECLARATIONS.md) contracts.

Preserve existing record IDs, immutable Resource revisions, message attachments, context manifests, task evidence, drafts, and pending/uncertain run state through additive migrations. Never rerun old work as a migration side effect.

## Chosen defaults

| Setting | Initial behavior |
| --- | --- |
| Workspace | One human owner with independently authenticated agents; additional-human administration stays later |
| Opening view | Last accessible location; Messages for first use |
| Messaging | Auto participation, visible recipient override, deterministic routing first |
| Discussion | Six agent turns per autonomous burst; substantive owner input may authorize the next burst; handoffs, retries, acknowledgments, and polling never renew it |
| Agent setup | Name, purpose, connection method, and intended scope; advanced fields collapsed |
| Connections | External MCP/HTTP participant, managed runtime, optional immediate wakeup; availability is observed separately from configuration |
| Presets | Generic external heartbeat, generic manual client, immediate webhook, local Codex, and AgentOS-managed connection; named app examples are editable setup data |
| Polling | Reuse an existing client heartbeat; suggest a 30-minute check only when configuring a new periodic check, without claiming it is active |
| Subscriptions | Assigned requests, direct messages, requested group responses, and relevant results; other permitted project events are opt-in |
| Routine agent actions | Read permitted context, reply, save/revise permitted Resources, capture tasks, and propose plans; structural plan changes remain reviewable |
| Execution | Human-approved bounded scope, two workers, 60-minute session, isolated worktrees for compatible code work |
| Review reserve | Target 20% of measurable available capacity for review/integration; when usage is unknown, reserve time/worker capacity and show the uncertainty |
| GitHub | Selected repositories, draft PRs, recorded checks and human review; no automatic merge or deployment |
| Memory | Explicit scoped facts, preferences, decisions, and lessons with provenance; agent suggestions remain distinguishable from accepted facts |
| Search/storage | PostgreSQL indexed queries and text search, immutable file storage; no vector database initially |

Presets are versioned configuration copied into an agent's setup. Editing a preset does not silently change existing agents; offer an explicit preview before applying an update. Connection credentials remain distinct even when agents share a template, owner, or provider capacity pool.

## Delivery sequence

Milestones describe the intended order of usable releases. Individual dependency edges in the backlog allow independent local work to continue while an external connection awaits setup.

| Milestone | Work packages | User-visible result | Gate |
| --- | --- | --- | --- |
| M0. Shared contracts and scoped access | W01-W03 | Existing work survives migration; accounts and permissions support external participation | Owner and agent privileges remain distinct; paged reads and atomic writes are tested |
| M1. Connect agents with defaults | W04-W10 | Add an agent, connect it, check its inbox, reply, and hand off work | Generic protocol test plus a real external polling round trip; Muse and ChatGPT Work qualification tracked separately |
| M2. AgentOS integration | W11-W14 | Import managed agents, inspect their real status, and use supported runtime/configuration operations | Real disposable AgentOS task and return path with durable receipt and observed evidence |
| M3. Finish daily collaboration and planning | W15-W18 | Natural groups/threads, useful context, shared material, and substantial reviewable plans | Conversation-to-Resource-to-task journey without manually carrying context |
| M4. Complete coding and GitHub delivery | W19-W22, W27 | Approved work produces checked draft PRs and an integration review queue | Real project checks and draft PR on an authorized test remote; exact-commit and recovery tests |
| M5. Shared memory and roadmap scale | W23-W24 | Agents retrieve scoped durable knowledge; large plans remain usable | Provenance/revocation tests and a reproducible large-data workload |
| M6. Operational release and coherent interface | W25-W26, W28 | One understandable daily app that survives interruption and restore | All required gates, real desktop/phone browser journeys, clean upgrade/restore and release evidence |

### M0. Shared contracts and scoped access

Define distinct identities for the human owner, agent participants, installed client connections, and internal service operations. Map AgentOS participants by stable external identity. A product name is display/setup metadata. Changing a name, model, or pickup method must not change authorship or duplicate pending requests.

Separate agent permissions from per-request context restrictions. A long-lived participant grant can discover its own inbox; a claimed request carries narrower context and mutation authority. Group work cannot access private DM context through search, files, summaries, or a broad account token. Persistent clients that cannot isolate private context must use separate grants/connections where needed and cannot receive a strict independent-opinions guarantee.

Replace owner-only assumptions at the API boundary and remove implicit owner privilege from externally reachable completion paths. Internal transitions should have explicit checked service authority. Authorize every result, event stream, upload/download, preview, and query consistently. Client-supplied author, scope, or task IDs never establish authority.

Introduce indexed records/relationships for grants, connection bindings, request claims, and delivery intents. Move agent/API queries to bounded database reads. Replace the global command lock incrementally with appropriately scoped transactional reservations and deterministic lock ordering. Keep concurrency correctness while improving throughput; do not rewrite the entire store merely for architectural symmetry.

### M1. Connect agents with defaults

Build one participant API with MCP and HTTP entry points over checked domain commands. Expose identity/capability lookup, paged inbox reads, claims, context retrieval, replies, handoffs, Resource operations including uploads, task capture/proposals, checkpoints, and result submission. Approval, acceptance, connection administration, and arbitrary repository execution are not ordinary participant tools.

Commit requests and wakeup intents transactionally. A wakeup signals pending work; both external polling and managed runtimes claim it through one admission path. Claims check current revisions, permissions, cancellation, resolution, and budgets. A read is not a claim. A claim is not proof a model ran. An expired started attempt needs reconciliation before redispatch. Result replay must be safe.

Register connectors by explicit capabilities. Replace the worker's single Codex instance with connection selection. Keep external participants usable without implementing a runtime start method. Add bounded signed webhook delivery with retries and coalescing for configured trusted destinations; reject arbitrary agent-supplied callback URLs or credential forwarding. Brand-specific protocol translation belongs in an adapter.

Build the Add agent flow and defaults alongside the API. Generate secret-free heartbeat instructions and show connection details separately. Test authentication, useful tool calls, and actual check-ins as different states. Expose waiting, next check-in when known, working, paused, failed, and unknown outcomes. Include credential rotation/revocation and template editing without losing history.

Prepare a remote MCP/HTTP gateway for cloud clients, with TLS, scoped identity, and client-compatible authentication. Prefer authorization-code OAuth with PKCE for remote clients and separate scoped tokens where the client supports them; reuse maintained authentication components. Confirm the actual handshake instead of assuming every client accepts the same method. The owner browser session is never an agent credential. A tunnel or deployment is a separate activation step; local operation remains supported.

Use the generic external-heartbeat preset for Muse. The owner confirmed custom MCP and an editable heartbeat, with no incoming webhook. Use the same generic mechanism for ChatGPT Work, qualifying scheduled tool availability separately. An unavailable client does not prevent completing the API, templates, or other qualified clients, but its live gate stays open.

### M2. AgentOS integration

Keep AgentOS canonical for its logical agents, declarations, assignments, host/runtime lifecycle, credentials, and enforcement. The workspace owns collaboration, plan readiness, approval, claims, and result acceptance. Workspace membership can change independently of an AgentOS declaration. Cached AgentOS observations must show their source revision and age.

Inspect and map capabilities, read-definition, plan/status, supported authoring, task submission, inspection, cancellation, and output through the actual controller contract. Prepare changes with expected source revisions and durable idempotency keys. Show desired, effective, and observed state separately. Reuse supported preview/apply and grant boundaries for configuration controls; do not fabricate a parallel host-management system.

The inspected controller uses Unix/SSH transport, has a 64 KiB task-input limit, and reports unknown acceptance, bounded output, and gaps. Session binding, steering, full event replay, provider setup, and the live model/tool journey remain qualification issues. Respect limits and declared unsupported operations. A scoped context reference helps only if that runtime has qualified tools to fetch it. MCP/tool delivery through the selected AgentOS version is an explicit dependency, not an assumed feature.

Implement the workspace-side adapter and tests in this repository. Keep any required AgentOS changes in a separately documented upstream package with exact missing operations and acceptance criteria. Existing instructions prohibit modifying or deploying AgentOS during work on this app unless separately authorized. Continue independent workspace work while such a dependency is pending.

Qualify a disposable managed agent through submission, disconnection, receipt lookup, reply/Resource creation, and actual tool/file evidence. Never substitute a direct Hermes demo for proof of AgentOS integration. A direct runtime connection may still be offered separately with an accurate label.

### M3. Finish daily collaboration and planning

Finish named channels and threads, scoped agent discovery, forwarding, unread position, drafts, stable scroll, attachments, deep links, and a useful human Inbox. Build one bounded model-assisted speaker selector behind deterministic Auto routing. It can select or invite permitted participants but cannot grant access or approve work. Preserve independent-round staging, explicit missing respondents, and late-correction labels.

Add source-linked conversation briefs with coverage and omission indicators. Decisions, requirements, unresolved questions, and proposed alternatives are explicit records. Preserve original conversations and exact Resource revisions. Context manifests distinguish initial input, later retrieval, and corrections. A summary never silently becomes an authoritative requirement.

Complete Resource browsing, permission-aware search, backlinks, syntax-highlighted pastes, section retrieval, file upload/download, and pinned/latest navigation. Keep one Resource identity across its links. Show unsupported file extraction honestly and retain downloadable originals.

Expand the task model from its current simplified representation to progressive plans with milestones, dependencies, alternatives, questions/blockers, acceptance criteria, pinned requirements, and execution policies: Automatic, Isolated worktree, Exclusive, or Manual. Add expected write areas, shared locks, capable-worker requirements, and exact prerequisite code. A title remains enough to capture a task. Display why something is blocked and what action would unblock it.

Review AI plan changes as diffs, including dependencies and approval invalidation. Routine permitted task capture or commentary is distinct from applying structural plan changes. Preserve the canonical task across conversation/project references. Add compact board and focused dependency views after the outline works; avoid making a graph compulsory.

### M4. Complete coding and GitHub delivery

Create a reviewed execution profile capable of running project build/test commands in an isolated environment. Start with supported local Codex and selected repositories. Define filesystem, process, network, credential, timeout, and cleanup behavior. Separate worktrees do not isolate host secrets or services. Verify actual test execution, output, exit status, and evidence against the recorded commit. Keep dirty work on interruption.

Extend work sessions to select qualified workers across connections, reserve shared resources, and account for shared provider capacity. Preserve a finite approved task set and reserve capacity for review/integration. Manual and webhook start use one deduplicated operation. An expected reset remains a scheduling hint. New work, a new scope, or unresolved external execution cannot be smuggled into a retry.

Use a server-side GitHub adapter and scoped publisher, with selected-repository GitHub App support as the target connection. Keep existing authorized read access usable. Record required scopes and support exact-remote write qualification only after authorization. Publish attempt branches and draft PRs; keep publication credentials out of agent worktrees. Reconcile uncertain publication by stable attempt/branch identity before retrying.

Track task/attempt/base/head/PR/check/review relationships. Receive authenticated events with deduplication and out-of-order handling, then periodically reconcile missed events. A new head invalidates outdated check/review assumptions. PR links distinguish fulfillment from related context. Closed-unmerged PRs do not satisfy dependencies.

Verify candidate branch combinations serially in a disposable integration checkout. Record the exact candidate and target commits and combined checks. Manual merge and task acceptance release the appropriate dependencies only after ancestry/integration proof. No automatic merge or deployment is added by this plan.

### M5. Shared memory and roadmap scale

Build small typed memory records on the Resource/decision foundation. Support facts, preferences, decisions, and lessons, each with sources, scope, author, accepted/proposed state, and correction/retraction history. Agents can propose and retrieve memories within grants. Memory does not change authorization or replace approved task requirements.

Use private-agent, conversation, project, and explicitly shared workspace scopes. Apply request scope as well as account access. Index and retrieve bounded sections; invalidate stale summaries/search entries on revocation or retraction. Do not claim revocation erases context already retained by an external provider.

Finish database pagination, text search, query indexes, bounded event reconciliation, and UI virtualization. Use a reproducible workload of 50 agents, 10,000 tasks, 100,000 messages, and 10,000 Resource revisions. Proposed local targets are warm paged API reads under 500 ms p95, common writes/claims under one second p95 at ten concurrent clients, and useful navigation under one second. Record hardware and distributions; revise targets explicitly if evidence warrants it. Never fetch the whole workspace to render one inbox.

Automatic memory extraction/consolidation and semantic retrieval are later work unless measured retrieval failures justify them. The first release should make explicit shared knowledge dependable.

### M6. Operational release and coherent interface

Keep operational screens secondary to messages, Resources, plans, and review. Finish the human Inbox and return-after-absence digest with source links and clear actions. Reopen the last location, retain drafts/scroll, provide keyboard and screen-reader behavior, and test real browser interactions on desktop and phone widths. Add installable web-app metadata; do not let a service worker cache private API responses or replay queued execution on reconnect.

Extend backup/restore to new identities, grants, connection metadata, Resources, events, and attempt receipts. Specify protected secret custody separately; a database dump alone cannot promise restoration of external OAuth/provider access. Restore with dispatch paused, quarantine uncertain runs, and require deliberate connection reactivation. Preserve existing startup/shutdown commands and add health, migration, connection diagnostics, and bounded redacted logs.

Run the original A/B/C scenarios and the new external-participation tests. Label real service/model evidence separately from fixture coverage. Publish an acceptance matrix stating what is ready and what still needs external setup. Leave the app idle with no test agents consuming capacity.

## Critical acceptance scenarios

| Scenario | Required result |
| --- | --- |
| An unfamiliar MCP client joins | No product-name branch or new domain code; its own grant, inbox, Resource, and reply work |
| Two agents use the same preset | Independent credentials, inboxes, and histories; editing one does not alter the other |
| Agent changes pickup method | Pending IDs and history survive; polling and immediate wakeup cannot double-claim |
| Request waits overnight | A correction, cancellation, or resolution before pickup prevents obsolete execution |
| External client sends duplicate claim/result | Stable original receipt; no duplicate accepted response or task output |
| Worker disappears after starting | Unknown stays visible; lease expiry does not authorize duplicate external execution |
| Broad account token is used in a group | Request context still excludes private DMs, staged peer answers, and unrelated private memories |
| Membership/grant is revoked | New tool reads, search snippets, event payloads, previews, and downloads deny access |
| Agents hand work around a group | Existing allowance and authority follow the handoff; no self-renewing loop |
| Substantive owner follow-up arrives | New bounded burst may proceed; Stop remains authoritative until deliberately continued |
| AgentOS source changes during setup | Expected-revision conflict is visible; no stale declaration is applied |
| GitHub events are duplicated or missed | Reconciliation produces truthful current state without repeating publication |
| PR head or prerequisite changes | Stale verification/approval is invalidated or held for review; exact ancestry is checked |
| Backup is restored | Record and file checksums agree; old pending or uncertain work does not automatically run |

## External setup and workarounds

| Dependency | Prepare independently | Needed for the live gate |
| --- | --- | --- |
| Cloud reachability | Authenticated gateway, deployment configuration, TLS/reverse-proxy instructions, local protocol tests | Chosen reachable host and authorized activation; keep existing loopback app usable |
| Muse / ChatGPT Work | Generic connection grants, templates, heartbeat instructions, MCP contract tests | Actual account connection and scheduled tool/result round trip; each product recorded separately |
| AgentOS | Adapter, capability mapping, fixture tests, precise upstream-gap document | Authorized disposable host/context, qualified provider/tool path, and any required upstream support |
| GitHub publication | Publisher code, selected-repository configuration, fixtures and local Git tests | Exact authorized test remote and required connection scopes |

Credentials, remote deployment, and changes to other projects remain governed by existing authorization. No broad questionnaire is needed to develop the local packages. Ask for a concrete missing setup only when its prepared live test is the next dependent step. A workaround must not weaken context/permission checks or convert fixture evidence into a live-pass claim.

## Implementation workflow and next batch

Start with W01, then W03's migrations and query foundations, then W02's scoped access, then W04-W06. This produces the reusable account/inbox/API/setup foundation. W11, the AgentOS contract mapping, can proceed once W01 is stable so integration needs shape the implementation early. Dependency readiness alone does not authorize simultaneous edits to shared files. Serialize schema, migration, command-dispatch, and shared frontend edits unless ownership has been split explicitly. Adapter and interface work may proceed in parallel only after shared types and migrations are settled. Respect the prior limit of two coding workers and one integration owner if implementation is delegated.

Each package should leave an integrated runnable checkpoint, update its evidence, and rerun the checks relevant to its change. Reuse the existing suite; add tests for new boundaries and failure cases rather than counting shallow tests. Reserve a final integration/release pass. Do not replace unfinished required gates with an expanding list of optional features.

The connected release is complete when a real polling agent and a real immediate managed agent exchange useful work through shared records, an approved coding task yields a verified draft PR, a substantial roadmap stays responsive, memory respects its scopes, and interruption/restore preserve truth. AgentOS, Muse, ChatGPT Work, and GitHub each retain their own qualification result; a passing substitute does not mark an untested integration complete.

Later work includes additional adapter families, multiple human owners, automatic memory consolidation, semantic search when justified, richer collaborative document editing, a plugin marketplace, and deeper shared AgentOS login/navigation. None should delay the defined connected release.
