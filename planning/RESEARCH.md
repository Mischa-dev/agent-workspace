# Research and source review

Reviewed September 21, 2026; revised after the owner's written decisions. Findings distinguish documented behavior, inspected local implementation, and qualification still needed. No production service, authenticated third-party workflow, or model run was tested in this planning revision. [CONTRACTS.md](CONTRACTS.md) maps the actual inspected interfaces to the proposed adapter.

## The source conversation

Read all four turns of [Collaborative Agent Platform](https://chatgpt.com/c/6ab0a6e3-0194-83e8-b25d-02ec49a18203) through the Codex app's conversation reader. The public web URL did not return the conversation, but the app supplied the complete discussion.

Requirements carried forward:

- Immediate, scheduled, and on-demand agents share durable communication.
- DMs and group chats remain useful without task management.
- Spaces and persistent Resources exist independently of conversations.
- Resources and conversations can support several projects without copying.
- Delivery, reading, acceptance, and completion are different events.
- Agents get bounded catch-up context with access to originals.
- The collaboration app and shared workspace form one product, independently useful and connectable to AgentOS.

The current conversation adds advanced task planning, parallel coding, capacity-triggered execution, and deeper GitHub integration. The owner selected a fresh start, then settled Auto participation, separate scoped sessions, one Resource model, a TypeScript backend, a single messaging-first visual direction, and human-approved work scopes producing draft PRs without automatic merge or deployment. Those decisions supersede the earlier proposal's alternatives.

## Local projects

### NANC

Inspected [TASK_SYSTEM_PLAN.md](</Users/mischanelson/Documents/ChatGPT/nanc ( not another notion clone(temp name lol)/TASK_SYSTEM_PLAN.md>), [task-layer documentation](</Users/mischanelson/Documents/ChatGPT/nanc ( not another notion clone(temp name lol)/docs/TASK_SYSTEM.md>), [AgentOS bridge documentation](</Users/mischanelson/Documents/ChatGPT/nanc ( not another notion clone(temp name lol)/docs/AGENTOS_BRIDGE.md>), [MASTER_PLAN.md](</Users/mischanelson/Documents/ChatGPT/nanc ( not another notion clone(temp name lol)/MASTER_PLAN.md>), and selected task command/read source.

Useful implemented foundations include shared command authorization, idempotency receipts, version conflicts, task readiness, approval scopes, required context revisions, and attempt/result distinctions. The inspected command code checks execution approval and prerequisite readiness. Changes affecting downstream tasks invalidate relevant execution state.

Its documentation explicitly limits the current system: GitHub links are references, cross-task resource reservations are not enforced, automatic scheduling is absent, and the bridge has not qualified a live provider-backed AgentOS worker. Those are new work for this proposal.

Carry forward the semantic rules and behavioral cases. Do not carry over the old navigation, every roadmap item, workspace-wide agent-token scope, or source architecture by default. The documented 256-card roadmap is an existing planning document, not this app's implementation backlog.

The initial review policy retains human acceptance. Within one approved task, ordinary implementation and checks proceed without repeated permission; across task boundaries, accepted completion and the exact prerequisite commit remain gates. With no automatic merge, dependent coding waits for manual merge while independent work continues. Automated checked completion is a later possibility, not an initial prerequisite or default.

### Existing Agent Workspace

Inspected [README.md](/Users/mischanelson/Documents/Agent-workspace/README.md), selected sections of [PROJECT_SOURCE_OF_TRUTH.md](/Users/mischanelson/Documents/Agent-workspace/PROJECT_SOURCE_OF_TRUTH.md), and source entries for messages, work items, and GitHub.

This project already overlaps substantially: Slack-like channels and threads, separate work items, attributed participants, external agents, Codex-backed execution workers, worktrees, and GitHub delivery. Its source includes structured message nodes and participant/task references. Its project record describes substantial prior implementation and qualification.

That history is a source of reusable lessons, not current live proof. Its project-centered organization also differs from the new requirement for standalone DMs, spaces, and Resources. Before extracting any implementation, inspect its dependencies and prove the selected behavior in the new model. A fresh start does not require re-solving receipt deduplication or structured mentions from memory.

### AgentOS

Inspected [PROJECT.md](/Users/mischanelson/Documents/ChatGPT/AgentOS/PROJECT.md), current ownership and qualification sections of [FOUNDATION.md](/Users/mischanelson/Documents/ChatGPT/AgentOS/FOUNDATION.md), and [docs/TASKS.md](/Users/mischanelson/Documents/ChatGPT/AgentOS/docs/TASKS.md).

AgentOS owns durable agent intent and logical identities, component assignments, lifecycle/control, and desired/effective/observed explanations. Hermes owns runtime execution mechanics. NixOS and systemd realize host isolation and services. The accepted management direction is Rust with a React/TypeScript interface.

Task contracts include durable submission, runtime acceptance, uncertain outcomes, idempotency, inspection, cancellation, and bounded output. Cancellation acknowledgement is not confirmed termination. Output gaps and the lack of qualified replay remain explicit. Current local documentation still says the real model-and-tool task journey is pending.

This supports a clear integration contract. It does not establish that a new workspace can already run unattended through AgentOS. Target Hermes through AgentOS and a separate Codex workspace connector, each with its own acceptance cases. The Codex connector need not wait for a native AgentOS Codex adapter. In this revision, actual AgentOS task source was re-read to verify the submission fields, byte limit, durable dispatch, and uncertain-acceptance behavior.

## What to learn from Linear

| Documented behavior | Useful lesson | Where this proposal differs |
| --- | --- | --- |
| Human assignee and agent delegate are separate | Preserve accountability when execution moves between agents | Allow richer agent collaboration without forcing every message into an issue |
| Agent sessions expose working, input-needed, error, complete, and stale states | Make the agent's operational state visible | A scheduled agent needs a known delayed-delivery state, not an immediate-response promise |
| Structured activities retain snapshots of input | Keep the input a worker actually received | Separate conversation edits, response requests, and pinned execution context |
| Issue relations distinguish blockers, related work, and duplicates | Use typed relationships and clear blocker explanations | Preserve prerequisite history and validate executable dependency graphs |
| Sub-issues support decomposition, with optional status propagation | Make large work expandable | Do not close unfinished children merely because a parent is marked done |
| Projects have visual dependencies | Show project sequencing | Model execution eligibility and shared resources separately from dates |
| GitHub integration links PRs, checks, reviews, and issues | Bring delivery evidence into the task | Task acceptance can require more than PR state |

Sources: [agent setup and delegation](https://linear.app/developers/agents), [agent sessions and activities](https://linear.app/developers/agent-interaction), [interaction guidance](https://linear.app/developers/agent-best-practices), [issue relations](https://linear.app/docs/issue-relations), [parent and sub-issues](https://linear.app/docs/parent-and-sub-issues), [project dependencies](https://linear.app/docs/project-dependencies), [GitHub integration](https://linear.app/docs/github).

These documents support specific behaviors. They do not establish that Linear enforces this proposal's cross-task worktree/resource scheduling rules. Treat that as our requirement, not a missing feature claimed from incomplete research. Linear labels its agent API a developer preview, so pin and recheck contracts if ever integrating directly.

The most useful design lesson is predictable behavior with good visibility. The most tempting mistake would be reproducing a full team issue tracker before making the owner's conversations and shared context work well. That judgment comes from this project's goals, not a claim that Linear is poorly designed.

## What to learn from Grok Bot

The official introduction describes chat, routines/triggers, and bot-to-bot messages. The multi-team guide presents reusable bots in project channels with a separate Notion task system. Its author explicitly describes that arrangement as experimental. The small team limit in the example is the author's own setup choice, not an established product maximum. [Grok Bot 101](https://x.ai/bot/guides/grok-bot-101), [multi-team guide](https://x.ai/bot/guides/how-i-run-multiple-teams-of-grok-bots).

Borrow recognizable participants, easy conversational access, reuse across groups, and visible handoffs. Combine them with shared Resources and tasks in this app.

The accessed guides do not disclose exact responder routing, concurrency, memory isolation, or loop limits. Their screenshot media could not be retrieved by the web tool. This review therefore establishes documented workflows, not a verified visual or hands-on audit of Grok Bot. An accessible reference can inform later refinement, but it is not a blocker or a reason to reopen the chosen messaging-first visual direction.

Auto is the selected default. Deterministic routing uses explicit recipients, replies, and exchange focus before a replaceable selector handles ambiguity. Get independent opinions and Discuss together have distinct context rules. Test their usability and tune routing thresholds; do not treat the earlier Lead default as an outstanding choice. These are this product's rules, not an inferred description of Grok Bot internals.

## Codex capacity and execution

The fetched [Codex app-server documentation](https://learn.chatgpt.com/docs/app-server) describes rich-client integration, thread/turn events, approvals, and account rate-limit reads and notifications. Returned limits can include multiple buckets, usage percentage, duration, and reset timestamps. Missing observations remain unknown.

Use these as connector capabilities, not a promise about a particular account's current balance or the cost of an upcoming task. This review did not read the owner's usage balance or consume a reset. Pin the installed protocol version before implementation. The documentation labels network WebSocket transport experimental; prefer a managed local adapter boundary and qualify the exact transport selected.

For this revision, `codex --version` returned `codex-cli 0.148.0`. A local `codex app-server generate-json-schema` invocation generated the non-experimental protocol schema without starting a provider session. It includes thread start/resume/read, turn start/steer/interrupt, rate-limit read/update, and multi-bucket response fields. Steering requires an expected turn ID. No inspected field proves exactly-once dispatch after a crash; `clientUserMessageId` must not be treated as such a guarantee. [The inspection receipt](CONTRACT_INSPECTION.json) records schema/source digests and observed fields. This is interface evidence only.

Keep ChatGPT-backed Codex capacity separate from API billing. Do not silently switch to a paid API when subscription capacity ends. A manual reset prediction can trigger a finite session, but the scheduler must use actual observations and rate-limit responses to decide what runs.

## GitHub delivery

[GitHub's webhook guidance](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks) supports secret validation, event/action checks, prompt acknowledgment, asynchronous processing, and delivery-ID handling. The same ID is retained on redelivery. Store acceptance durably before acknowledging, and distinguish a duplicate received event from a failed job needing processing again.

[GitHub's merge queue documentation](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue) describes checks against the current target and preceding queued changes. Availability depends on repository ownership and plan. The proposal needs a basic serial integration route even when the native feature is unavailable.

The selected initial policy is draft PRs and verification without automatic merge/deployment. Integration composes candidate commits only in a disposable checkout. Human review/manual merge remains the gate for target-branch changes and dependent coding. Native merge-queue integration is deferred.

## Limits of this review

The earlier projects were inspected read-only. Historical acceptance records were not rerun, and live services were not requalified. Public research used primary documentation for product and technical claims. No interviews or broad complaint study supports conclusions about user-population preferences. Usability judgments here are hypotheses tailored to Mischa's described workflow.

The next evidence should come from stage A using the single chosen design and two real identities with isolated sessions. Schema inspection cannot prove routing quality, runtime memory isolation, correction handling, stop behavior, or recovery. The roadmap turns these into acceptance cases, followed by discussion-to-work and coordination stages. More documentation alone will not establish usability.
