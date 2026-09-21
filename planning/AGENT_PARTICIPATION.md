# External agent participation and remaining work

September 21, 2026. Proposed continuation of the existing product plan, based on the owner's clarification about Muse, ChatGPT Work, Codex, Hermes, and OpenClaw. This document records the design rationale; the current implementation and qualification state are tracked in CONNECTED_RELEASE.md. Existing Auto participation, context boundaries, bounded execution approvals, and the AgentOS ownership boundary remain in force.

Implementation order, chosen defaults, and release gates now live in [NEXT_IMPLEMENTATION_PLAN.md](NEXT_IMPLEMENTATION_PLAN.md), with concrete packages in [NEXT_BACKLOG.json](NEXT_BACKLOG.json). This document retains the design rationale.

The active release is deliberately narrower: connect agents to the existing collaboration/task system and deploy it reliably. Broader AgentOS configuration, GitHub automation, memory, and scale sections below describe future extensions, not mandatory launch work.

## Current implementation

The application has a verified local core: real Codex DMs and groups, durable messages and response requests, revisioned Resources, spaces/projects, task proposals and dependencies, bounded work scopes, local execution records and isolated worktrees, GitHub PR reads/links, and recovery/backup checks. Two identities currently use the same local Codex runtime with separate sessions. See [ACCEPTANCE.md](../ACCEPTANCE.md) for the distinction between live and fixture evidence.

External accounts, the shared inbox, replies/results/handoffs, file operations, MCP/HTTP, OAuth, heartbeat pickup and signed webhook notifications are implemented. Shared space boards, channels/threads and project outline/board/dependency views are also implemented. See [CONNECTED_RELEASE.md](CONNECTED_RELEASE.md) for the current evidence and remaining host/client qualification. A generic protocol test is not proof that Muse or another actual client is configured.

Remaining work includes deployment to the selected reachable host and live client setup, plus the explicitly deferred GitHub automation, broader execution profiles, generated briefs, and scale extensions.

## One durable inbox, several ways to wake

Every agent has a durable inbox regardless of whether the workspace can start its runtime. The inbox holds response requests, assigned work, review requests, questions, and selected events. Each item points to its canonical conversation, task, or Resource. Do not duplicate the authoritative task graph inside the inbox.

| Pickup mode | Behavior |
| --- | --- |
| Immediate | Commit a request, then signal a qualified connector. The connector claims through the shared admission path before running. |
| Scheduled | The agent's scheduler wakes it. It checks its inbox and claims eligible requests through the same admission path. |
| Manual | An agent checks when its owner opens a session or asks it to. Pending requests remain visible in the meantime. |

These are capabilities of an agent connection, not fixed product categories. One connection can support immediate delivery and a fallback sweep. No second scheduler should independently start the same work. A notification is a hint that work exists; the database owns the request, cancellation, and result state.

Separate five decisions:

1. Can this agent access the conversation or Resource?
2. Is the agent being asked to respond, merely informed, or assigned a task?
3. Is the requested action within its permissions and any required execution approval?
4. When can its runtime wake and what can it actually do?
5. Which attempt has successfully claimed the current request revision?

Reading never completes a response request. Group membership never means every message starts a model turn. Preserve Auto as the ordinary default: clear mentions/replies and conversation focus choose recipients; ambiguous new questions normally choose one relevant agent. Background updates, quoted mentions, forwarded messages, and acknowledgments do not start recursive discussions. All agent handoffs share the current discussion allowance. A schedule check does not renew that allowance.

A recurring job can carry an owner-approved standing scope for checking and processing eligible inbox items. Each check-in must enforce its bounds. It cannot grant repository approval, replenish an exhausted conversation exchange, or create unlimited work. Polling intervals and quiet hours belong in connection settings, not in the meaning of messages.

## Identity, connections, and capabilities

Create workspace identities for independently acting agents or roles chosen by the owner. Product names are examples, never built-in account types. The owner can create multiple agents using one product, rename an agent, or replace its connection without rewriting its collaboration history. Keep provider authentication and usage pools separate from workspace authorship and access grants.

An agent identity has a stable ID, display name, purpose, memberships, and allowed workspace actions. Its connection records how it authenticates, where it runs, how it wakes, supported operations, and the latest successful capability check. Each installation gets a revocable credential or OAuth grant. Never hand external agents the owner cookie or allow a tool argument to choose the authenticated author.

Keep conversation sessions and execution sessions separate. Do not equate a provider chat ID with an agent identity or canonical task ID. Multiple connections may later serve one logical agent, but that requires explicit routing and the same claim protection. Start with one connection per independently acting identity.

Capabilities should state observed support for inbox reads, replies, Resource access, task proposals, repository execution, isolated sessions, cancellation, in-run corrections, tool access during scheduled runs, and capacity telemetry. A declared capability remains unqualified until tested. Unknown remains visible. Agents can discover another agent's role and capabilities within their permitted directory without seeing its secrets.

Show useful states: waiting for next check-in, last seen, next scheduled check if verified, working, awaiting review, paused, connection failed, and outcome unknown. A daily polling agent is not broken because it is currently asleep. Avoid a misleading universal online/offline dot.

## Reusable integration building blocks

The owner explicitly wants an extensible system for creating custom connections. Adding an agent that uses an existing supported protocol should require configuration and setup instructions, with no product-specific changes to messaging, tasks, memory, or authorization.

Keep these concerns independently configurable:

| Building block | Responsibility |
| --- | --- |
| Agent identity | Name, role, durable history, inbox, memberships, and permissions |
| Connection | Authenticated client or runtime binding, transport, credential reference, version, and observed capabilities |
| Subscription | Which permitted conversations, assignments, review requests, or changes appear in the agent's inbox |
| Participation policy | When those events request a reply or action, subject to Auto routing, approvals, and budgets |
| Wakeup policy | External heartbeat, external schedule, supported immediate notification, manual pickup, or a configured combination |
| Connector adapter | Translation of an actual external protocol into the shared lifecycle and checked commands |
| Setup template | Reusable defaults, setup instructions, and a connection test for a common configuration |

Access is a prerequisite for a subscription. Subscribing never grants permission, and a wakeup never constitutes a task claim. Keep these checks in the application, outside adapters and setup templates.

The external participant interface should work through generic MCP and HTTP access. A client that checks the shared MCP inbox needs no runtime-start adapter. When immediate notification is available, a configurable webhook adapter can send a bounded request reference with the correct authentication. Integrations that require a proprietary protocol can add a small connector implementing the versioned contract. Adapters declare supported operations; unsupported cancellation, isolation, or steering remains explicit.

Start with these concrete adapters and an ordinary code registry inside the modular application. Do not introduce arbitrary uploaded executable plugins or a large workflow engine merely to make integrations extensible. Externally hosted bridges can use the checked API where a new in-process adapter is unnecessary. New adapter code must not change task readiness, claims, permissions, or message semantics.

Offer an Add agent flow that asks for a name and role, a connection method, and the intended work scope. Use sensible subscription and participation defaults. For a polling client, issue its connection grant and generate a short reusable heartbeat instruction: identify the current account, list pending requests, claim eligible work, retrieve context, and reply or record a blocker. Keep secrets in the connection setup, outside heartbeat text. Provide a Test connection action that distinguishes successful authentication, tool calls, and an actually observed scheduled check-in.

For an immediate connection, the same flow configures its supported wakeup mechanism and tests a request through to a returned result. Advanced schedule, event filters, and transport settings remain optional. The interface should not require choosing or even knowing a product brand.

Muse is one instance of the generic MCP plus external-heartbeat configuration. Hermes is one possible instance of a managed runtime connection with immediate wakeup. Templates may prefill either setup, but must remain editable and must not determine core behavior. Users can save their own templates and instantiate several agents from one template with distinct grants. Changing the display name or template name cannot change routing or access.

Prove extensibility by connecting an arbitrarily named test participant through the generic MCP path without adding a product type or conditional branch. Instantiate the same template twice and verify separate inboxes and permissions. Change one agent's pickup method and verify pending request IDs and history survive. Add a test adapter through the connector contract without modifying domain policy. Retain the separate claim protection when polling and immediate notifications overlap.

## Agent tools and delivery reliability

Expose a versioned application API with an MCP adapter. Both call the same checked application services as the UI. Preserve the modular application; this does not require a separate microservice or message broker.

The first tool set should cover identity/capability lookup; bounded inbox and conversation reads; explicit request claim/renewal; reply and handoff; Resource search/read/create/revise; task read/propose/update within grants; and result submission. Suggested names are `inbox.list`, `request.claim`, `request.renew`, `message.reply`, `request.handoff`, and `result.submit`. These names describe proposed operations, not currently available tools. Successful results return stable IDs, revisions, and links.

Requests need their origin, intended recipient, kind, expected outcome, source revisions, reply destination, parent handoff, eligibility, expiry, and allowed actions. The claim creates the exact context manifest and attempt generation. Tool access remains scoped to that attempt and current permissions, even when the external client has a longer-lived login.

Persist wakeup intent with the request, then deliver with stable deduplication keys. Commit claims atomically; recheck cancellation, resolution, permissions, revisions, and budget. Overlapping polling and push cannot start separate admitted attempts for the same request. A missed webhook leaves the inbox item pending.

An expired lease after execution started creates uncertainty, not permission to execute again. Reconcile the runtime receipt or request deliberate recovery. Accept duplicate result submission idempotently; preserve late or stale results as evidence without satisfying newer work. Track parent requests and causal chains to detect handoff loops. Coalesce redundant notifications, and avoid model calls for empty inbox checks where the runtime permits deterministic checks.

MCP supplies tools and context to a connected client. It does not establish a general ability to launch a sleeping third-party app. Client notification support also does not prove that a model turn will run. The client's scheduler or a qualified runtime connector owns wakeup. See the [MCP architecture documentation](https://modelcontextprotocol.io/docs/learn/architecture).

The current server is loopback-only. Cloud participants require a reachable authenticated HTTPS endpoint or a supported connection bridge. A local MCP process or private host name alone is insufficient for a cloud client. Keep local operation supported; qualify remote access as a separate deployment step. Do not expose the owner API as the agent API.

## Example handoff

Muse posts a message asking ChatGPT Work to investigate a project issue and attaches the relevant Resource revision. The workspace creates a response request under the permitted exchange and shows when ChatGPT Work is expected to check in.

Muse uses its owner-confirmed custom MCP connection and editable heartbeat to check its own account's inbox. It has no incoming webhook. Its integration therefore needs outbound MCP access and a working heartbeat schedule; the workspace does not need to initiate an inbound connection to Muse.

At the scheduled time, ChatGPT Work queries its inbox, claims the still-current request, and receives the issue, relevant conversation context, requested outcome, constraints, and accessible Resource references. If the issue was resolved or cancelled overnight, there is no eligible request to execute.

ChatGPT Work saves a report as a Resource and replies to the originating thread. If coding is needed, it proposes or hands off a linked task. A coding agent claims it only within an approved execution scope. Completion returns to the original conversation, and Muse can learn about it through its own qualified pickup mode. If any participant cannot perform an action, it records a useful blocked reason or a permitted handoff instead of silently abandoning the item.

## Configuration and AgentOS

The workspace owns collaboration records, inbox requests, task readiness, approvals, and canonical claims. AgentOS owns its declarations, runtime configuration, host lifecycle, credentials, and enforcement. The workspace can display and request AgentOS configuration changes through a versioned adapter; it should not maintain a second editable authoritative copy.

For externally managed products, store the connection and observed capabilities. Provide setup instructions and a connection test for settings the workspace cannot change. Record who owns a schedule and whether it is configured or merely desired. Writing instructions into a file named `HEARTBEAT.md` is not proof that a scheduler exists or will execute them.

Configure membership, purpose, pickup mode, permissions, and budget through a compact agent settings page. Reveal advanced runtime controls only for connections that support them. Shared provider capacity should be represented as a shared pool; independent identities do not imply independent quotas. Use observed usage where available and bounded admission where it is unknown.

## Git and task execution

Keep the traceable chain: task and approved revision -> attempt -> repository and base commit -> isolated checkout -> commit and verification evidence -> PR -> review/integration outcome. Multiple attempts and PRs may belong to one task.

An agent can investigate, plan, or review through collaboration tools without possessing a writable repository. A qualified execution connection owns the checkout and enforces the approved operations. Worktrees support isolation but do not grant permission or isolate secrets by themselves. Never let independent workers share one writable checkout.

Route GitHub publication through a scoped publisher. Start with draft PRs for authorized remotes, without automatic merge or deployment. GitHub is authoritative for PR/commit/check state; the workspace owns task approval and acceptance. Consume authenticated GitHub events idempotently and periodically reconcile missed events. Bind checks and review evidence to the actual commit. Task completion and dependency release require the configured acceptance/integration conditions, not an agent saying "done" or a PR merely being closed.

Complete a reviewed execution profile for project build/test commands before claiming unattended general coding. Keep shared locks, exact prerequisite commits, and verification of combined branches. Changing requirements or code baselines must invalidate stale approval and evidence as appropriate.

## Shared memory

Start with the existing revisioned Resources plus explicit decisions and a small typed memory record. A memory records its claim or preference, source links, scope, proposer, acceptance status, update time, and any expiry or superseding record. Distinguish owner-confirmed facts from agent suggestions and preserve corrections instead of silently overwriting them.

Support private agent, conversation, project, and explicitly shared workspace scopes. Apply access checks before retrieval, search results, snippets, and summaries. Linking or recalling a memory never broadens access. A private DM does not become shared memory merely because the same agent joins a group.

Add bounded `memory.search`, `memory.propose`, and permitted revision/retraction operations after inbox participation works. Use PostgreSQL search first and measure retrieval failures before adding embeddings. Memory proposals cannot approve tasks or change standing permissions. External products may retain their own context; the workspace cannot promise to erase it or provide strict independent opinions through a client with uncontrolled shared memory. Qualify that boundary explicitly.

## Recommended delivery order

| Milestone | Complete when |
| --- | --- |
| 1. External participation | Generic agent registration, separate connection grants, reusable setup templates, checked API/MCP tools, durable pull inboxes, replies/handoffs, and request claims work with a real external client. Adding another client on the same protocol requires no product-specific code. Connection settings show capability and schedule evidence. |
| 2. Mixed-runtime collaboration | One real scheduled participant and one real immediate participant complete the example handoff with retained context and return links. Qualify Hermes through AgentOS first; add other runtimes without changing the core request model. |
| 3. Complete code delivery | Approved project tests, scoped draft-PR publication, GitHub event reconciliation, exact-commit checks, prerequisite integration, and review states work on an authorized test remote. |
| 4. Memory and scale | Explicit decisions and scoped memory work across agents; pagination/search and a large roadmap remain usable. Extend briefs, recipient selection, and return-after-absence UX; launch channels/threads are implemented. |

Memory provenance and access boundaries should influence the schema now. A broad automatic memory extraction service can follow verified agent participation.

Acceptance must include: two connections racing for one request; duplicate wakeups and results; cancellation/correction while an agent is asleep; interrupted work with an expired lease; revoked access during retrieval; a forwarded mention; a handoff loop exhausting its existing allowance; an empty heartbeat; a scheduled client without its expected tool; and a missed GitHub event recovered by reconciliation. Test actual cloud reachability and unattended tool access before labeling a product integrated.

## Current external evidence

- The owner confirms that Muse supports a custom MCP connection and an editable heartbeat file, and has no incoming webhook. Plan Muse as a scheduled pull participant with its own workspace identity and credential. Its connection to this application still needs an end-to-end test. [Muse's official product page](https://ai.meta.com/muse/) independently describes background monitoring and connected apps. Muse here is the personal agent product at the supplied URL, not the similarly named model or coding CLI.
- [ChatGPT Work scheduled tasks](https://learn.chatgpt.com/docs/automations) documents recurring checks and workflows using plugins. The particular workspace connection, tool permissions, and scheduled execution environment must still pass a real round trip.
- [Hermes webhooks](https://hermes-agent.nousresearch.com/docs/user-guide/messaging/webhooks) documents incoming event-triggered runs. This does not automatically make the installed AgentOS adapter support every current Hermes feature.
- [OpenClaw inbound webhooks](https://docs.openclaw.ai/automation/cron-jobs/webhooks) documents wake and agent-turn endpoints. Admission acknowledgment is distinct from a completed result. This app has not implemented or qualified an OpenClaw connection.

No external agent account, recurring automation, remote endpoint, or new runtime integration was enabled during this planning review.
