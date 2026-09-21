# A shared workspace for people and AI agents

September 21, 2026. Product plan, version 0.2. Working title only. Revised to apply the owner's written decisions.

The owner chose a fresh start during this review. This plan describes a new app. NANC, the existing Agent Workspace, and AgentOS are research inputs, not repositories to merge automatically. No implementation, migration, deployment, agent run, or persistent integration was started.

This document owns product behavior and fixed boundaries. [The delivery roadmap](ROADMAP.md) owns staged delivery and acceptance tests. [The context and connector contract](CONTRACTS.md) owns context packets, delivery semantics, and the proposed versioned integration. [The research notes](RESEARCH.md) separate evidence from unverified behavior.

## 1. What this app should be

One private workspace where Mischa can talk with agents, let them collaborate, save useful material, develop large plans, and turn approved work into reviewed results.

The everyday experience should be familiar. Open a DM, talk in a group, drop a file, jot down an idea, or check a project. More structure becomes available when the work needs it. A casual conversation should not require a project, a task, a repository, or an execution budget.

The hard design problem is continuity. A conversation should lead naturally to a note, a task, a worker run, and a pull request, while each remains independently useful. The app must preserve those connections across different agents, providers, machines, and sessions.

The strongest daily-use test is returning after four days away. Within a few minutes, Mischa should understand what changed, what needs a decision, which work finished, and what can happen next.

### Confirmed requirements

- A fresh application with a coherent design, drawing lessons from the earlier projects.
- Full messaging, including DMs, groups with several agents, forwarding, and attachments.
- Agents that respond immediately, check in on a schedule, or participate only when started.
- Spaces, projects, shared notes, pastes, files, and material reused across projects.
- Large roadmaps developed progressively with AI, including dependencies, decisions, and blockers.
- Controlled parallel coding, including isolated worktrees and tasks that must run sequentially.
- A manual or webhook trigger for using available Codex capacity on prepared work.
- Strong GitHub integration and eventual inclusion in the AgentOS experience.

### Settled starting decisions

Start with one owner and several agents. Make the data model capable of additional human participants without building enterprise administration. Build a responsive web app that can be installed on the desktop. Prioritize desktop work and usable phone messaging, capture, and review.

Use React/TypeScript, a TypeScript backend, and PostgreSQL. Choose one calm, compact, messaging-first visual direction. Auto participation is the default. Reopen the last location; use Messages on first launch. Work sessions require a human-approved bounded scope, start with two workers, and produce draft PRs and verification results. They do not merge or deploy automatically.

Keep the product usable independently of AgentOS. In an AgentOS installation it can be the collaboration application, with a shared login and navigation later. AgentOS continues to own its agent declarations and execution environment.

## 2. A few independent things, connected well

| Thing | Purpose | Relationship rules |
| --- | --- | --- |
| Workspace | The account and access boundary | Start with one personal workspace |
| Agent | A recognizable participant | Exists independently of a model session or project |
| Conversation | A DM, group, or named channel | May have no project, or link to several |
| Space | A lasting area such as Personal, Homelab, or Research | Collects material and conversations across projects |
| Project | An effort with an intended outcome | Connects plans, tasks, conversations, repositories, and Resources |
| Resource | A note, paste, file, image, or saved result | Exists without a message or project; has stable identity and revisions |
| Task | A piece of tracked work | One canonical record, regardless of how many views show it |
| Response request | A durable request for an agent to answer | Separate from message visibility, reading, and execution permission |
| Session | Isolated continuation for one agent in one context | Conversation-scoped; task execution and independent opinions use separate sessions |
| Run | One agent invocation | Links to its task or response request, context manifest, progress, and output Resources |

Spaces and projects overlap through explicit links. They do not form a compulsory folder hierarchy. A Resource can appear in several places without copies. A task can be referenced by several projects, but has one accountable owner and one execution state. Start with one primary project per task and additional project links. Notes, pastes, specifications, files, and saved outputs are Resource kinds, not competing object systems.

Conversation membership controls conversation access. Resource access is evaluated separately. Adding a link does not silently grant access, and placing a conversation in a space does not expose private message history. Search, previews, summaries, and agent context all use the same access checks.

For AgentOS-managed agents, the workspace participant points to the AgentOS logical identity. The app stores its display details and membership, while AgentOS owns the declaration. External agents receive a workspace identity tied to their authenticated connector. One provider connection can serve several agent identities without making them the same person in chat.

## 3. How the app should feel

Use a compact navigation rail with Messages, Inbox, Spaces, Projects, and Agents. Pin favorite conversations and projects below it. Put global search and quick capture within easy reach. Runs appear in their conversation or project, with one compact global activity drawer.

Reopen the last accessible location and its draft/reading position. On first launch, open Messages with a clear New message action. If the previous location no longer exists or access was removed, return to Messages with a short explanation. Inbox collects questions, reviews, failures, and meaningful updates without becoming the compulsory home.

Use quiet neutral backgrounds, restrained borders, compact rows, readable text, and a single accent for selection and primary actions. Keep status colors supplementary to text. Use a conversation area with an optional side panel for Resources, tasks, and PRs. On phones, the side panel becomes a full-width detail view with a reliable return to the conversation. A persistent composer, obvious Stop control, and clear recipient indicator matter more than decorative dashboards. Refine this one direction through use, without a three-option selection gate.

| Place | Primary experience |
| --- | --- |
| Messages | Conversation list, readable transcript, reliable composer, optional context panel |
| Space | Direct access to notes, pastes, files, and linked conversations |
| Project | Overview, plan, tasks, Resources, and activity, with GitHub where relevant |
| Task | Outcome, next action, context, blockers, evidence, and current worker |
| Inbox | Questions, reviews, failures needing action, and meaningful updates |
| Agent | Identity, capabilities, access, connection, and honest availability |

Keep the transcript central. Open a linked Resource, task, or PR in a side panel without losing the conversation or draft. Use the same object card everywhere. Backlinks answer where a Resource is used. A message menu offers Reply, Forward, Save as note, Create task, Link to existing task, and Copy link.

Show ordinary task fields first. Reveal dependencies, execution policy, resource locks, and review conditions when preparing agent work. The default plan view is a collapsible outline. Boards and a focused dependency map are alternative views of the same records. A giant graph should never be the compulsory home screen.

Design draft recovery, keyboard navigation, screen-reader announcements, narrow screens, unread markers, scroll position, search, and empty/error states alongside the happy path. Agent output arriving in the background must not move the user's reading position.

## 4. Auto participation with understandable behavior

The central distinction is between **being able to read a message** and **being asked to answer it**. Store the conversation separately from response requests. A visible message does not automatically start every member.

Grok Bot documents bot-to-bot messaging and reusable project teams. Its public guides do not establish its internal speaker selection or loop prevention. Auto is this product's chosen behavior, not a claim about Grok Bot's implementation. [Grok Bot 101](https://x.ai/bot/guides/grok-bot-101), [multi-team guide](https://x.ai/bot/guides/how-i-run-multiple-teams-of-grok-bots).

### Default behavior

| Event | Initial behavior |
| --- | --- |
| DM to an agent | Create a response request for that agent |
| Explicit mention or reply addressed to an agent | Request that agent's response |
| Ordinary follow-up | Continue the relevant exchange or answer its pending question; background progress cannot change that focus |
| Unaddressed new question | Select one relevant available participant, using deterministic evidence first |
| Ask several agents or everyone | Create explicit response requests for those participants and show any queued by the exchange limit |
| Active discussion needs another perspective | Invite a relevant permitted participant within the same exchange and remaining allowance |
| Agent posts an update | Make it available as context; no automatic all-member wakeup |
| Agent asks another for help | Create an explicit handoff in the same conversation, within its authority |
| Forwarded or quoted message | Preserve attribution and context; embedded mentions do not trigger runs |
| User cancels or changes a request | Update the pending request, and signal or steer a running responder where supported |

Auto requires no permanent lead or per-message recipient selection. A small recipient indicator shows the current target and can be overridden. Explicit mentions and replies win. If a reply names a different agent, use the named recipient and keep the replied-to message as context. Otherwise bind follow-ups to the active exchange and its pending question, then use clear capability and topic matches for new questions. Store the routing reason. A background update never becomes the active exchange just because it is newest.

Invoke one replaceable model-assisted selector only when those rules cannot choose confidently or an active discussion needs another participant. Give it permitted participant descriptions, the current message, and bounded conversation context. It selects speakers, not permissions, task readiness, or execution approval. Do not ask every agent to assess every message. Validate the selector's output server-side. A timeout uses deterministic context when available; if no defensible recipient remains, preserve the message and offer one brief in-conversation clarification. Never broadcast because routing failed.

Provide two actions. **Get independent opinions** gives selected agents the same initial request and permitted shared context, with isolated sessions and answers withheld from other respondents until their initial submissions are frozen. This prevents access through search and retrieval as well as the prompt. **Discuss together** lets selected agents read, build on, and challenge one another's published responses. Later discussion does not overwrite the initial independent answers. A connector must demonstrate isolation before the app describes its results as independently produced.

Use threads for extended work, preserve substantive answers in the conversation, and collapse tool activity into a run card. Display status summaries and actual actions, not private model reasoning. Users can expand logs when troubleshooting.

An exchange starts with six agent turns, configurable. Each started response invocation consumes a reserved turn. Every handoff and invited participant inherits the same exchange ID and allowance. Retries, restarts, and delayed check-ins cannot reset it. Selectors have a separate small call/time bound; execution tasks have separate budgets. Deduplicate requests, combine compatible pending follow-ups, and allow one active writer per session. If more agents are requested than turns remain, show the remaining requests as queued. At the limit offer Continue, which adds an explicit six-turn allowance by default. Ordinary acknowledgments and progress do not create new turns or exchanges.

Stop immediately closes admission for that exchange, cancels queued responses, and forwards interruption to active connectors. Show Stop requested until termination is observed; preserve received text and flag any output arriving after Stop. A later scheduled check-in cannot restart the stopped exchange. Continuing it is a deliberate user action. The detailed routing, allowance, and delivery rules are in [CONTRACTS.md](CONTRACTS.md).

### Immediate and occasional agents

All agents have durable inboxes. Delivery may use an event notification, scheduled polling, or a manual catch-up call. The request waits even if no process is running.

Keep unread position, fetched, accepted, working, and answered separate. Fetching messages cannot complete a request. A scheduled agent reads intervening messages before acting and can report that a request was superseded or already resolved.

Use accurate labels such as Queued, Next check-in at noon, Working, Waiting for you, and Connection unavailable. Do not infer availability from a profile existing or a process having been healthy yesterday. Record observation times. Capability checks decide which connectors support push, polling, streaming, steering, cancellation, and status recovery.

Muse, ChatGPT Work, and other external products are desired participants. Their actual connection methods remain to be qualified. A product name alone is not evidence of a supported inbound webhook or unattended execution API.

### Shared records, separate sessions

Agent identity outlives its model sessions. Bind continuation to agent, conversation/thread, and access scope. Private DMs, group conversations, independent-opinion rounds, and repository tasks do not share a runtime session or unrestricted memory. A familiar agent in a group does not bring its private DM history with it. Explicitly forwarding authorized material is a separate action.

Keep original messages durably, with revision history. Each response receives the request and selection reason, a source-linked brief with explicit coverage, relevant recent messages verbatim, recorded decisions/constraints/questions, selected task and Resource revisions, and its scoped continuation state. Allow bounded permission-checked retrieval of older messages and Resource sections. Summaries are navigation aids; only explicit approved records define requirements and authority.

Every response/run records a context manifest. It distinguishes material prepared by the workspace, sent by the connector, acknowledged by the runtime where observable, and retrieved later. A correction received after start is a separate event with delivery status, never retroactively inserted into the original manifest. The UI must not claim the model understood a correction merely because a transport accepted it.

Before starting a queued response, check whether its question was resolved, cancelled, superseded, or affected by a newer correction. For a running response, steer only if supported; otherwise mark the correction pending, request interruption where necessary, and start corrected work only after the previous run is terminal or safely reconciled. Changes to executable requirements also invalidate the affected approval. [CONTRACTS.md](CONTRACTS.md) specifies these transitions.

## 5. Shared material that survives the conversation

Resources are first-class records. An agent can upload a log, save an idea, or write a document without sending a message. The Resource then has a stable link and can be attached anywhere the recipients have access.

Start with plain text and Markdown notes, syntax-highlighted pastes, and files. Defer a sophisticated collaborative document editor. Keep each format's canonical representation explicit. Importing a rich NANC document must retain its original data and report any lossy conversion.

Each Resource records title, kind, author, timestamps, visibility, revisions, and provenance. Files include type, size, and checksum. Generated outputs point to their run and source context. A summary is labeled as a summary and links back to the material it summarizes. Every write supplies its expected revision. A conflict retains the proposed edit and presents the current revision for reconciliation; last-writer-wins overwrite is not permitted. The same rule applies through UI, HTTP, and MCP.

Ordinary browsing links can follow the latest revision. Message attachments and executable task requirements pin the selected revision. Later edits produce a visible newer-version indication. No reviewed specification changes invisibly underneath a running task.

Temporary material has an expiry and a Keep action. A task cannot depend on an expiring Resource without retaining the required version. Storage cleanup respects those references. A Resource removed from a space still exists if other references need it; deleting content is a separate action with a visible consequence.

Forwarding shows whether it will share a link or an explicit snapshot, and whether the recipients can open it. Sharing private material requires a real access change or an authorized copy, not merely another link. Treat pasted instructions, repository text, and imported documents as content; none grants an agent new authority.

Agents discover Resources through titles, descriptions, search, outlines, and bounded section reads. Context packets include selected Resources and reasons for including them. Avoid loading the whole workspace into every turn. Preserve original sources so a future agent can check a summary.

## 6. Advanced planning with a small everyday interface

Use one task model with kinds such as feature, bug, research, maintenance, and ordinary task. A title is enough for capture. Execution requires a clearer contract.

An executable task records the desired outcome, acceptance criteria, relevant context, next action, constraints, accountable owner, required capabilities, and review policy. Coding work also records repository, target branch, source baseline, execution mode, and resource requirements.

Large plans grow from project outcomes into milestones, features, and executable tasks. Leave distant work broad and explicitly tentative. Distinguish a checklist one worker can complete from tasks needing independent ownership or dependencies.

AI planning should produce a reviewable change proposal. Show tasks to create, changes to existing work, new dependencies, open questions, and any affected approvals. Apply an accepted batch atomically with stable IDs and retry protection. Search for existing tasks before creating duplicates. A prose roadmap can contain live task references but is not a second writable status database.

### Keep different kinds of state separate

| Question | State |
| --- | --- |
| Where is the work? | Inbox, planned, active, review, done, or cancelled |
| Is it part of the chosen approach? | Selected, unresolved, or retained alternative |
| Can prerequisites be satisfied now? | Derived readiness with explanations |
| May it run unattended? | Approved scope and execution policy |
| Is a worker doing it? | Claim, lease, attempt, and runtime observation |
| Has it met the required outcome? | Verification and acceptance evidence |
| Has code reached its destination? | PR, merge, and release state |

The UI combines these into a short explanation such as "Waiting for the API change to merge" or "Ready, outside the approved batch." Every excluded queue item explains what would make it eligible.

### Dependencies, decisions, and changes

Parent/child means grouping. Explicit prerequisite edges control readiness. Reject dependency cycles and show the cycle path. Cancelled prerequisites do not count as completed. Dependencies may cross projects within the same authorized workspace.

A decision preserves the question, alternatives, selected option, and rationale. Unchosen branches remain inspectable and stay out of the execution queue. A task cannot depend on an excluded branch without a visible conflict.

Required context is revision-pinned. Changes to the outcome, criteria, required context, or permission boundary invalidate affected execution approval. Cosmetic edits do not. A changed prerequisite pauses dependent dispatch and flags running work for reconciliation.

The first scheduler requires prerequisite acceptance and, for dependent code changes, the required commit on the configured target branch and worker baseline. With the initial no-auto-merge policy, code-dependent tasks wait for a manual merge; unrelated eligible tasks continue. Record the observed merge commit and target head and verify ancestry before dispatch. If code was reverted or its contract changed, revalidate the dependency. Stacked unmerged execution is deferred. A dependency arrow alone does not transport code into a worktree.

### Bounded approval without permission fatigue

Normal conversation, saving Resources, drafting plans, and routine permitted workspace edits use ordinary access permissions. They do not require repository execution approval. Routing a question to a coding agent cannot itself authorize repository work.

A human-approved scope freezes eligible task revisions, repositories, allowed actions, execution limits, and review conditions. Initial scopes permit bounded implementation, checks, and draft PR creation. Automatic merge and deployment are disabled. Routine steps inside the scope proceed without repeated approval. New tasks discovered by agents enter a proposed queue and do not silently expand the scope.

The initial completion policy is human acceptance with recorded verification evidence. A check pass, agent review, or draft PR is evidence, not an acceptance grant. Show Ready for review when the required result is present. A later separately approved checked-completion policy may automate a narrow gate, but it is outside stages A-C and cannot imply merge permission.

Keep serial steps that should run together inside one coherent work package where appropriate. Across task boundaries, explain the real gate, such as Waiting for your review or Waiting for the prerequisite to merge. This preserves useful autonomy without inventing permission to remove a blocker.

## 7. Parallel execution that produces mergeable work

Expose a simple choice on prepared tasks: Automatic, Isolated worktree, Exclusive, or Manual. Underneath, use structured scheduling information rather than trusting a Boolean flag.

| Requirement | Why it matters |
| --- | --- |
| Task dependencies and exact baseline | A worker must receive the code it depends on |
| Expected write areas and shared contracts | Separate branches can still produce incompatible changes |
| Exclusive resources | Migrations, shared databases, deployment targets, ports, and devices may conflict |
| Worker capabilities and permissions | A ready task may still lack a suitable worker |
| Host and provider capacity | Parallel work shares CPU, memory, storage, and usage limits |
| Verification and integration plan | Several isolated successes can still fail together |

Start with declared file areas, explicit resource locks, and conservative behavior for unknown scope. A schema-changing task can reserve the migration sequence while unrelated docs work continues. Detect changes outside a declared area and surface them for reconciliation. A worktree isolates checked-out files; it does not enforce database or host isolation.

Use transactional task claims and resource reservations. Add lease generations so a stale worker cannot publish an accepted result after another attempt has taken over. Runtime fencing or confirmed termination is needed for external side effects; a database lease expiring cannot stop an old process.

Uncertain dispatch stays uncertain until reconciled with the runtime. Never respond to a timeout by blindly starting a second worker. Preserve dirty worktrees and checkpoints on interruption. Release exclusive resources only when safe.

Completed branches enter a review/integration queue. An approved integration attempt may combine candidate commits in a disposable checkout and run the necessary combined checks. This is verification, not a merge into the target branch. Report exact candidate and target commits and preserve the draft PRs for human review and manual merge. Reserve capacity so integration and review can happen before admitting more implementation work.

Use a serial integration-verification path that works for a personal private repository. Native GitHub merge-queue integration is deferred; its availability depends on the repository and plan. No stage in this roadmap requires enabling automatic merges. [GitHub merge queue documentation](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/configuring-pull-request-merges/managing-a-merge-queue).

## 8. The use-available-capacity workflow

Call the user-facing action **Start work session**. It opens a finite execution window over an approved queue. A manual button, authenticated webhook, or AgentOS schedule can invoke the same operation.

Start with two concurrent execution workers and a 60-minute session window, both configurable. The first setup shows the frozen approved task set, preferred workers, finite time boundary, usage reserve, and draft-PR policy. Save the settings as a reusable preset so later sessions need only the shared Start work session action. A preview explains which work can start and which tasks are blocked. An authenticated webhook invokes the same command with the same preset and idempotency rules; it cannot expand approval.

Initially reserve the final 20% of the session's configured time or usable measured capacity for checks, review, and integration, and stop new implementation admission when either reserve threshold is reached. This is a configurable admission policy, not a prediction of exact model cost. Recompute from timestamped observations. With unknown telemetry, use the explicit time bound and host/concurrency limits. Each session needs a stop time; a reusable preset can supply it.

1. Deduplicate the trigger and create a durable session record.
2. Check task eligibility, runtime health, host capacity, and provider observations.
3. Atomically claim a compatible set and reserve required resources.
4. Dispatch through the runtime adapter with stable attempt IDs and idempotency keys.
5. Record progress, output Resources, checks, and checkpoints. Refill available slots as work finishes, preserving the reserve.
6. Recompute eligibility when dependencies, approvals, capacity, or the plan change.
7. Stop admitting work at the configured boundary; preserve or finish active work according to policy.
8. Present a digest of results awaiting review, accepted results, draft PRs, blocked items, interruptions, and remaining work. Never report unmerged drafts as shipped.

Two useful presets are "Use remaining capacity before reset" and "Start when capacity returns." They differ in when they admit new work. The user's expected reset is a scheduling hint, not proof that capacity is available.

Current official Codex app-server documentation exposes account rate-limit reads and update notifications, including usage percentage, window duration, and reset timestamps. Prefer those observations when the selected connector supports them. Support several quota buckets and treat missing values as unknown. These fields do not predict the precise cost of the next task. [Codex app-server documentation](https://learn.chatgpt.com/docs/app-server).

Keep usage accounting at the provider/account boundary because workers can share capacity with other sessions. Respect rate-limit responses and backoff. Use task-size ranges and checkpointability for packing the queue, then tune from observed runs. Maintain explicit retry and time limits. If telemetry is unavailable, use manual time/concurrency bounds and show that limitation.

The useful objective is completed, verifiable work. When there are no prepared coding tasks, the queue can include approved research, review, documentation, or task refinement. It should not manufacture work just to consume capacity.

## 9. GitHub is part of the work record

Start with repository connections and live PR/check/review information. Add authorized branch publication and draft PR creation through the execution connector. GitHub remains authoritative for repository and PR state; the workspace owns task intent, planning, and acceptance.

Use a scoped GitHub App with selected repositories. Keep credentials on the server or managed execution host. Separate observation permissions from code-writing permissions. The initial integration exposes draft PR publication and verification, with no automatic merge/deployment operation. A connector installation is not permission to expand an approved task scope.

Support many PRs for one task and several task links on one PR. Mark which links fulfill requirements and which are merely related. A closed unmerged PR is not task completion. An opened PR means an output exists. Required checks, reviews, merged commits, and task acceptance remain explicit.

Store task, attempt, repository, base commit, branch, head commit, PR, check results, and relevant Resource links. Show them together in the task and project review views. Invalidate stale review/check assumptions after a new head commit. Closing or deleting an external object leaves a visible historical reference.

Receive verified webhooks into durable storage, acknowledge promptly, and process asynchronously. Deduplicate deliveries and reconcile current GitHub state when events are late, missing, or reordered. Mirror only selected events into the human inbox. [GitHub webhook guidance](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks).

Defer full bidirectional GitHub issue synchronization until link-based workflows are dependable. If added, define which system owns each field and how conflicts work. Linear is inspiration, not a required service dependency. Its many-to-many PR links and visible review state are useful references. [Linear GitHub integration](https://linear.app/docs/github).

## 10. How this joins AgentOS

| Owner | Responsibilities |
| --- | --- |
| New workspace app | Conversations, Resources, spaces, project/task graph, collaboration access, task readiness, work-scope approval, canonical claims, review, and accepted results |
| AgentOS | Logical agent declarations, skills/tools/models, credentials, hosts, runtime lifecycle, schedules, resource enforcement, and observed execution |
| Runtime such as Hermes or Codex | Model/tool loop, runtime sessions, tool execution, and runtime-specific state |
| Connector | Maps identities and run IDs, delivers bounded context, reports events, and reconciles receipts |

The workspace approves what may be done. For AgentOS-managed execution, AgentOS also enforces which actor can do it and where; both permission checks must pass. Standalone Codex execution uses its connector's reviewed host/sandbox profile with the same workspace approval, without requiring an AgentOS installation. The app can expose AgentOS settings through its API without storing competing agent configuration.

The app computes task eligibility and owns canonical claims and the durable work-session coordinator. AgentOS supplies managed worker availability, schedules, runtime lifecycle, and host enforcement. Its schedule triggers the same Start work session operation. The app's durable jobs handle delivery and session admission without maintaining a second AgentOS declaration or host supervisor. There is one active coordinator per work session, with durable ownership. Standalone operation can use the Codex connector without requiring AgentOS to be installed.

Use the proposed `workspace-connector/v1` contract in [CONTRACTS.md](CONTRACTS.md) for capability discovery, start, inspect, input delivery, checkpoint, result, cancel, and events. Include workspace/task/attempt IDs, specification revision, immutable context references, allowed actions, budgets, cancellation state, and a stable dispatch key. AgentOS returns its own execution receipt and observed state. This is our adapter contract; it is not a claim that AgentOS or Codex natively implements every field or operation.

Keep cancellation requested, cancellation acknowledged, and termination observed distinct. Store only durable output that the connector actually received, and mark stream gaps honestly. External agents can use HTTP/MCP collaboration operations without being hosted by AgentOS.

Target Hermes through AgentOS and a Codex execution connector first. They are separate integrations to qualify. Two distinct agent identities may use isolated sessions on one qualified runtime while the second connector is pending. This preserves useful stage-A delivery without pretending to prove cross-runtime operation. Do not bypass AgentOS authority to make the Hermes demonstration work.

The current local AgentOS contract supports durable submission, inspection, cancellation, and bounded output; it does not expose a qualified conversation-session/steering contract and still records the real model/tool journey as pending. The installed Codex 0.148.0 schema exposes thread start/resume/read, turn start/steer/interrupt, and rate-limit operations. Generating that schema verifies interface shape only. [CONTRACTS.md](CONTRACTS.md) records the inspected capabilities, unsupported behavior, and live qualification gates.

## 11. Starting architecture

Use a modular application with one database and explicit typed records. Keep messages, Resources, tasks, execution integration, GitHub, and access control as modules sharing commands and policy. Avoid a general-purpose graph engine or a collection of microservices at the start.

Use React/TypeScript for the frontend, a TypeScript backend, and PostgreSQL. Store uploaded bytes outside message rows using straightforward file storage behind a storage interface, with versioned metadata and a backup manifest. Use a transactional outbox for delivery and durable background jobs. Start search with PostgreSQL. Do not introduce a vector database or a separate search service without demonstrated need. Keep realtime delivery replaceable; the durable event cursor is the recovery mechanism.

TypeScript is settled for this application's backend. AgentOS can remain Rust across the versioned adapter boundary. No backend-language comparison is part of this plan. Keep each module's data and commands explicit, using shared policy and transaction helpers rather than a generic universal-object layer.

HTTP and MCP use the same application commands, permissions, stale-write checks, and idempotency rules. Add small task-specific tools such as `inbox_read`, `conversation_read`, `message_send`, `resource_put`, `resource_read`, `plan_preview`, `plan_apply`, `task_context`, `claim_next`, `checkpoint`, and `submit_result`. Responses include pagination, revisions, omitted content, and actionable errors.

Basic chat, writing, planning, search, and review remain usable when an AI provider is unavailable. Backups include the database, Resource versions, relation metadata, and execution receipts. Restore starts with outbound dispatch disabled until reconciliation prevents replay of old work.

## 12. Prove the product in three stages

**A. Messaging and continuity.** Mischa opens a group with two real agent identities. Auto handles natural follow-ups without progress updates stealing focus. The agents exchange bounded responses, one checks in later, and they share a revision-pinned Resource. Interrupt a response and inspect the correction/context record. Demonstrate private-DM isolation and independent opinions. Either initial connector can provide both identities through isolated sessions while the other is being qualified.

**B. Discussion to work.** Turn that discussion into a reviewable task proposal. Mischa approves one bounded scope. One real worker changes a disposable repository and returns a linked draft PR with verification evidence. The task remains awaiting human review. No automatic merge or deployment occurs.

**C. Coordination.** Run two compatible tasks in separate worktrees, queue a conflicting shared-resource task, and hold a code-dependent task until its prerequisite is manually merged and accepted. Verify candidate integration in a disposable checkout. Restart the coordinator during uncertain dispatch and reconcile without duplicate work. Preserve interrupted workers' changes and publish an honest session digest.

These stages are delivery gates, not three disconnected products. Each extends the same conversations, Resources, tasks, and run records. Broad document databases, a marketplace, public sharing, voice rooms, elaborate organization structures, and universal provider support stay outside this initial sequence.

## 13. Fixed boundaries and behavior we can tune

| Fixed boundary | Configurable behavior and initial value |
| --- | --- |
| Shared records with isolated sessions; no private-context crossover | Context size, recent-message window, brief refresh thresholds |
| Auto routes speakers but never grants permissions | Confidence thresholds, capability matching, recipient overrides |
| Bounded exchanges share allowance across handoffs | Six agent turns; Continue adds six by default |
| One canonical Resource with immutable revisions and checked edits | Presentation, retention defaults, optional temporary expiry |
| Human-approved repository scope and canonical claims | Two workers, 60-minute window, 20% review/integration reserve |
| Draft PRs and recorded verification; no automatic merge/deploy in stages A-C | Task ordering, worker selection, retry limits within scope |
| React/TypeScript, TypeScript backend, PostgreSQL modular monolith | Layout density, notification preferences, connector implementations |
| AgentOS retains declaration, credential, host, and runtime authority | Which qualified connector supplies the first two identities |

Routing quality, useful context sizes, reserve size, and layout details need measured use. Keep them versioned and replaceable behind explicit contracts. Record why an agent was selected and what it received so mistakes can be diagnosed. Evaluate one visual direction with concrete scenarios and refine it; no promise of a perfect first design is needed.

The next bounded implementation milestone is stage A's first durable slice: two real agent identities, one DM and one group, Auto follow-ups, isolated context manifests, one shared Resource, delayed catch-up, and Stop/interruption. The roadmap defines its checks and prerequisites. This revision remains planning-only; it does not start that milestone or require another questionnaire.
