# Context, delivery, and connector contracts

September 21, 2026. Planning contract v1. Proposed workspace interfaces are not implemented. [PRODUCT_PLAN.md](PRODUCT_PLAN.md) owns product behavior; [ROADMAP.md](ROADMAP.md) owns acceptance tests. Source/interface inspection is recorded in [CONTRACT_INSPECTION.json](CONTRACT_INSPECTION.json).

## 1. Routing and exchange continuity

Store an exchange ID, originating request, participants, current pending questions, active-focus marker, routing-policy version, turn allowance, and stop state. Focus belongs to an exchange in a conversation/thread. Incoming progress, tool events, reactions, or unrelated background output never changes it. Each browser composer can retain its visible exchange anchor; the server checks that anchor against current state.

Apply routing in this order:

1. Explicit recipient override and actual mentions determine recipients. Forwarded/quoted nodes are excluded from mention extraction. A named recipient overrides the author of a replied-to message; the reply still determines context.
2. A direct reply without a new addressee returns to the relevant question or speaker in its exchange. A DM has its participant as the default recipient.
3. A natural follow-up such as "yes, use the second option" resolves against the focused exchange and its pending question. Preserve that anchor even if another agent posted a later background update.
4. For a clearly new question, use a unique relevant capability/topic match among eligible participants. "Everyone" creates requests for the current permitted agent roster, recorded at send time. Later joiners are not silently added.
5. If intent is ambiguous, use at most one bounded selector call for that routing decision. A clear unresolved question/focus may supply a deterministic fallback if the selector fails. If two possible pending questions remain equally plausible and guessing could change the meaning, ask one short clarification in the conversation and leave the response request pending. Do not broadcast or execute guessed repository work.

The replaceable selector returns recipient IDs, exchange ID or proposed new exchange, a short reason, and uncertainty. It receives no credentials, private DM history, or unrelated project corpus. Validate membership, contextual access, availability, and allowance after selection. Retry of the same message revision reuses its recorded routing decision; changing the decision is an attributed reroute with cancellation/supersession of the earlier pending request.

Within Discuss together, substantive responses can produce a bounded invitation to another relevant participant. It shares the original exchange. An agent's tool progress or routine acknowledgment cannot create such an invitation. Decisions and execution permission remain separate records governed by ordinary authorization.

Reserve one turn transactionally immediately before each new response invocation, against a six-turn initial exchange allowance. A definitive failure before dispatch can release the reservation. A started or uncertain invocation retains it; reconnect and delivery retry cannot create a fresh allowance. Handoffs carry the exchange ID. If eight agents are requested with six turns available, six may start and two remain visibly queued for Continue. User follow-ups within that discussion preserve the allowance; only an explicit new topic creates a new exchange. Continue adds six by default to the existing exchange and is deduplicated like other commands.

A response invocation can make tool calls within its own finite run budget; they are not extra discussion turns. Selector calls have their own bounded timeout and at most one retry for a transient failure, with no recursive model fan-out. Execution tasks consume work-session budgets rather than discussion turns. Turning a conversation into a task requires a recorded task and approved scope, not an agent classifying its own response as execution to escape the discussion cap.

## 2. Context packets and manifests

The packet is assembled from shared records for a specific run, not from another agent's unrestricted memory. Its access ceiling is the intersection of actor permission, conversation/task scope, and run-specific restrictions. Even an agent permitted to read a private DM in that DM cannot retrieve it from a group run.

| Packet section | Required content |
| --- | --- |
| Request | Message/request IDs and revisions, current request text, exchange, and selection reason |
| Conversation brief | Brief revision, source message IDs/ranges, covered cursor, uncovered intervals, and creation time |
| Recent messages | Relevant verbatim messages with authors, revisions, timestamps, reply links, and forward markers |
| Decisions and questions | Explicit records with proposed/approved/superseded status, owner, constraints, and unresolved questions |
| Work and Resources | Task/specification revisions, required Resource revisions/sections, and relevant links |
| Continuation | Prior state for this agent in this conversation/thread or this task attempt, with provenance |
| Limits | Context omissions, retrieval cursors, allowed operations, and budgets |

A summary is not a substitute for originals or pinned requirements. When space is tight, retain the request, constraints, authoritative decisions, and required inputs. Reduce advisory context first and report omissions. If required context cannot fit or cannot be retrieved, block execution with a specific reason rather than silently truncating it. AgentOS's currently inspected submission limit is 65,536 UTF-8 bytes; packet encoding must respect that adapter limit.

Persist the exact assembled packet or an immutable reference to its bytes, plus a manifest containing run ID, session binding, packet digest, request revision, brief coverage, message revisions, Resource revisions, task/specification revision, policy version, permissions checked, and omission details. Keep it under the same contextual access boundary, not a workspace-wide diagnostic endpoint.

Record separate events for packet prepared, connector sent, runtime accepted if observable, later retrieval, and later input delivery. Record known provider-thread ID, preceding context-manifest IDs, and the last app-supplied turn. If a reused provider thread contains untracked input, do not claim complete context knowledge: abandon it for this scoped run and construct a new session from authorized records. Runtime-controlled system context or compaction that the connector cannot inspect remains labeled as unobserved.

For each late correction, retain its message revision and receive time. Show whether it was queued, sent, acknowledged, rejected, or arrived after completion. Transport acknowledgment means delivered at that layer, not understood by the model. The initial manifest never changes to imply the correction was present at start. Correction manifests append to the same run history.

Before dispatch, recheck permissions, message/request versions, supersession, decisions, required revisions, and approval. During execution, revoked access prevents subsequent retrieval and protected writes; request interruption when the run's basis becomes invalid. Start a new scoped session if prior context is no longer appropriate. Removing permission cannot erase text a model already received, so do not promise retroactive removal.

## 3. Session isolation and independent opinions

Use a session key containing workspace, agent identity, conversation/thread or task-attempt identity, mode, and access generation. Keep one active writer per session. Provider credentials can be shared through a supported connection while provider thread IDs, workspace actor credentials, history, and run access remain separate.

Use conversation-scoped sessions for ordinary chat. Task attempts have their own execution sessions and explicitly selected conversation context. Continuation data is scoped to the same context. A runtime that injects unrestricted agent-wide memory cannot qualify private group/DM isolation without an isolation mechanism; do not solve that by instruction alone.

For Get independent opinions, freeze a common request/context snapshot and create separate initial-response sessions. Enforce temporary visibility restrictions on other participants' initial answers through message reads, search, summaries, output Resources, and provider continuation. The owner can inspect submitted answers, but respondents cannot access one another's initial submissions. Shared input Resources are read-only during that initial phase; outputs remain in each respondent's private staging area until publication, preventing the shared note from becoming a side channel. Disable cross-respondent messaging and shared writable tools for these initial runs.

Reveal the answers when all requested initial responses are terminal. A timeout or offline participant remains explicit; the owner can finish the round with missing answers, cancelling those pending initial requests. Late results are retained as late and do not masquerade as part of the original complete set. When more than six turns are needed, Continue releases queued initial requests without exposing already-staged answers to them.

Discuss together may start from the published initial answers, but uses new permitted conversation contexts and the remaining exchange allowance. It cannot rewrite frozen submissions. Record missing/failed participants and context differences. Independence here means isolation from peers' initial answers, not a guarantee of statistically independent models or beliefs. An external agent with uncontrollable side channels cannot receive the strict independence label.

## 4. Durable response requests and correction handling

Messages record what was said. Response requests record who is expected to act. A request includes source message/revision, recipient, exchange, routing decision, eligibility time, context generation, and lifecycle. Reading/unread position is separate. Request lifecycle is pending, claimed, running, answered, failed, cancelled, superseded, or needs reconciliation.

| Event | Required transition |
| --- | --- |
| Duplicate message delivery | Return the same committed message/request IDs; do not enqueue another response |
| Fetch pending requests | Return bounded data without marking work complete |
| Agent claims | Atomically acquire one request lease/generation; recheck whether the request still matters |
| Correction while pending | Retain both messages; supersede the old pending request and create or update the corrected one with a new context generation |
| Correction while running and steering is qualified | Send to the exact active run/turn; append a delivery record and reject a mismatched turn |
| Correction while running without steering | Queue it visibly; request interruption for substantive changes; wait for terminal state or reconciliation before corrected dispatch |
| Correction races with completion | Preserve the earlier answer labeled as predating the correction; create a follow-up request under the same exchange allowance |
| Question resolved or cancelled before offline pickup | Close/supersede its request; no stale invocation |
| Stop exchange | Close admission, cancel pending requests, request active interruptions, and retain uncertainty until observed |

An uncertain started request cannot become claimable just because its lease expired. Inspect its durable connector receipt. Stale-generation output is retained as evidence but cannot answer the current request or satisfy task acceptance. For repository work, corrections to the execution contract revoke the affected continuation permission and require a revised bounded approval. Ordinary conversation edits never gain execution authority through this process.

## 5. Proposed workspace-connector/v1

The workspace owns commands and receipts; adapters translate this contract into actual runtime operations. Version v1 fixes identity, permission, context, and outcome meanings. Capabilities describe optional operations and quantitative limits. Unsupported operations return explicit status, not simulated success.

| Operation | Contract |
| --- | --- |
| capabilities | Connector/runtime version, supported modes, context-byte limit, session isolation, input delivery, interruption, inspection, event replay, quota telemetry, and evidence status |
| start | Durable run ID, response-request or task/attempt reference, immutable context-manifest reference, actor/session binding, permitted operations, budgets, expected revisions, and dispatch key |
| inspect | Acceptance state, runtime IDs, observed state with time, error/uncertainty, cancellation status, and event cursor/gaps |
| deliver_input | Exact run and expected turn/generation, correction reference, idempotency key, and delivery acknowledgment; optional capability |
| cancel | Durable cancellation intent and forwarding acknowledgment, separate from observed termination |
| events | Ordered bounded events with durable cursors, duplicate-event identity, retention/gap declarations, and provenance |
| checkpoint / result | Workspace-owned callbacks with attempt generation, criteria/evidence, Resource references, remaining work, and next action; not an agent acceptance grant |

Keep workspace run ID, task attempt ID, AgentOS task ID, and provider thread/turn/run ID distinct. Do not overload one runtime identifier as the canonical task identity. The receiver resolves authorship from authentication, never an arbitrary `authorId` supplied by an agent. Secret values travel through protected connector configuration, not packets or manifests.

Commit dispatch intent and its stable key before contacting a runtime. A unique key binds input digest, actor, session, and task/request revision. Reusing a key for different input returns a conflict. A successful command transport can still report rejected or unknown acceptance. Store those states faithfully.

One durable coordinator admits work for a work session. Workspace task claims and shared-resource reservations commit transactionally. AgentOS enforces the assigned host/runtime boundary for its agents; the direct Codex connector enforces its own reviewed execution profile. Connector receipts describe execution, not a competing task graph.

A worker writes only its assigned checkout and approved local work. Keep GitHub publication credentials in the publisher, outside the worker environment; the publisher allows the approved attempt branch and draft PR operation. Verify the chosen sandbox and credential path prevent target-branch writes or deployment through an alternate route. Git worktrees alone do not enforce this. Integration verification combines commits in a disposable checkout and records checks; it cannot update the repository's target branch.

## 6. Inspected contracts and qualification gaps

| Capability | Hermes through current AgentOS | Installed Codex 0.148.0 |
| --- | --- | --- |
| Start | Source exposes durable revision/idempotency-bound submit translated to `POST /v1/runs` | Generated non-experimental schema includes `thread/start` and `turn/start` |
| Session continuation | Current submission has agent, source revision, and prompt; no conversation-session field | Schema includes `thread/resume` and `thread/read`; private-context behavior needs qualification |
| Late correction | No steer/input operation established by inspected task contract | `turn/steer` requires `threadId`, `expectedTurnId`, and input; race behavior needs live proof |
| Stop | Durable cancellation intent; runtime stop acknowledgment does not prove termination | `turn/interrupt` addresses thread and turn; observed stopping requires qualification |
| Idempotency / crash recovery | Durable receipt exists; unknown acceptance blocks resubmission; explicit reconciliation remains incomplete | `clientUserMessageId` is present but does not establish exactly-once dispatch; journal and recovery behavior need proof |
| Output | Bounded durable output pages with declared gaps; complete replay unqualified | Schema has run/turn notifications; durable capture/reconnect behavior needs proof |
| Capacity | No quota telemetry established in the inspected task interface | Schema includes rate-limit read/update and multi-bucket response fields; live account telemetry not checked |
| Real model and tools | Current local documentation records qualification pending | Interface inspection only; no model invocation or tool execution in this planning pass |

AgentOS's source-revision binding describes the agent declaration. Keep it separate from task specification revision and repository commit. Send only packet text that fits the current submission limit and separately qualify permitted Resource retrieval tools. Never bypass the controller to obtain unsupported session functionality.

For AgentOS, a fresh isolated invocation rebuilt from workspace context can eventually supply continuity without native session resume, but session/memory isolation must be proven first. If that is unavailable, qualify stage A using two Codex-backed identities with isolated sessions, and keep Hermes visibly pending. Do not silently share one global Hermes profile's private history.

Interface presence is not live qualification. Stage A needs real conversations and isolation; B needs a real repository/tool result and draft PR; C needs interruption, uncertain dispatch, and restart evidence. Runtime adapters are allowed to report a capability as unavailable until its acceptance test passes.
