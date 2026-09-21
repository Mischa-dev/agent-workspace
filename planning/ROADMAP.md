# Delivery roadmap

September 21, 2026. Revised around the owner's decisions. Planning only; the acceptance cases below have not been run against a new product.

[PRODUCT_PLAN.md](PRODUCT_PLAN.md) owns product behavior and fixed boundaries. [CONTRACTS.md](CONTRACTS.md) defines routing, context, delivery, and the proposed connector interface. [RESEARCH.md](RESEARCH.md) records evidence and limits. Build one application in the following three stages.

## Stage gates

| Stage | User-visible outcome | Required evidence |
| --- | --- | --- |
| A. Messaging and continuity | Talk naturally with two real agent identities, preserve context over time, share a Resource, and stop work | Real model responses; Auto follow-ups; bounded discussion; independent opinions; private-context isolation; offline catch-up; correction and interruption evidence |
| B. Discussion to work | Convert a discussion into reviewed tasks and one approved coding attempt | Reviewable plan change; pinned requirements; bounded human approval; actual tool/file changes; verification results; one linked draft PR; no auto merge or deployment |
| C. Coordination | Use a finite session for compatible parallel work and recover when interrupted | Two workers; shared locks; actual prerequisite commits; disposable integration verification; duplicate-trigger handling; uncertain dispatch and restart recovery |

Stage A can use two identities on one qualified runtime with separate sessions. Record the other connector as pending rather than describing that as cross-runtime proof. Target Hermes through AgentOS and Codex first. The stage-C integration gate includes qualification of the selected AgentOS connection; if blocked, record completed coordination cases separately and leave that gate open.

Use one calm, compact messaging-first design throughout. Refine it with the cases below. No language comparison or three-visual-options selection gate remains.

## Work packages

| ID | Package | Depends on | Evidence to complete |
| --- | --- | --- | --- |
| A0 | One interaction design and connector qualification recipe | None | Specify DM/group/composer/side-panel/Inbox/phone states; use the selected stack; record packet and connector expectations from actual interfaces |
| A1 | Shared identity, Resources, and durable commands | A0 | PostgreSQL records; actor-derived authorization; Resource revisions and checked writes; transactional jobs; idempotent commands; a small real restore |
| A2 | Messaging, Auto, and response requests | A1 | DMs/groups/channels/threads, drafts/unread/forwarding, deterministic routing and replaceable selector, exchange focus, six-turn allowance, Stop, and concise run cards |
| A3 | Real sessions and inspectable context | A1, A2 | One qualified connector serving two distinct identities; isolated sessions; bounded packets and manifests; correction/retrieval records; actual responses |
| A4 | Stage-A continuity and usability acceptance | A2, A3 | Pass A-T01 through A-T12 on the chosen design; retain live transcripts and receipts; name any connector still pending |
| B1 | Spaces, projects, progressive planning, and reviewable changes | A4 | Cross-project Resource/conversation links, title-only capture, task kinds, outcome/criteria/context, outline, milestones, dependencies, explicit decisions/alternatives, preview/apply, bounded agent reads |
| B2 | Bounded scopes and one worker | B1, A3 | Human approval tied to task/spec/context revisions; claim and attempt; exact baseline; isolated writable worktree; real tool execution; checkpoint/result; no worker-held publication credentials |
| B3 | Draft PR delivery and stage-B acceptance | B2 | GitHub connection, task/attempt/PR links, durable events, checks tied to exact commits, retained Resource outputs; pass B-T01 through B-T06 |
| C1 | Parallel claims and shared-resource reservations | B3 | Two configurable worker slots; transactional claim/lock acquisition; overlap/exclusive rules; conservative unknown scope; stale-attempt rejection |
| C2 | Bounded work sessions and capacity admission | C1 | One Start work session command behind button and webhook; preset/scope binding; trigger deduplication; default 60-minute window and two workers; quota/host bounds and 20% review reserve |
| C3 | Prerequisite commits and integration verification | C1, B3 | Wait for manually accepted/merged prerequisites; verify commit ancestry; test candidate combinations in a disposable checkout; never write target branch |
| C4 | AgentOS mapping and interrupted-run recovery | C2, A3 | Qualify configured Hermes-through-AgentOS connection; preserve its authority; inspect uncertain receipts, stopped/unknown states, and event gaps after restart |
| C5 | Stage-C acceptance and return-after-absence workflow | C2, C3, C4 | Pass C-T01 through C-T08; preserve dirty work; restore linked records with dispatch inactive; show truthful results and next actions |

These dependencies describe product delivery. They are not an instruction to start workers now. During later implementation, split packages into coherent changes and settle shared record/permission contracts before parallel edits. C2 and C3 can proceed independently after C1, followed by integration in C5.

## Next bounded implementation milestone

Deliver A0-A4 as a local stage-A slice using the chosen TypeScript stack and one visual direction. Qualify the two intended connector boundaries; whichever first proves the required isolation may supply both initial identities. Do not wait for a second provider merely to obtain a second identity.

The milestone is complete when Mischa can reopen Messages, talk in a DM and group, use a natural follow-up after a background update, request independent opinions or a short discussion, share a pinned Resource, wait for an offline participant, inspect what each run received, and stop/correct a response without losing it. Include drafts, unread state, stable scroll, keyboard operation, phone layout, and offline/error states.

Keep repository execution, GitHub publication, advanced task graphs, and the work-session scheduler out of this slice. They follow through the same records in B and C. Authenticate the runtime for the demonstration through its proper flow. Actual credentials and supported host setup are implementation prerequisites, not assumed planning facts.

## Stage-A acceptance specifications

Each case records inputs, IDs/revisions, context manifests, routing decisions, and observed output. Structural assertions can be automated. Natural-language routing needs a repeatable scenario set and manual readability review, not a promise of perfect accuracy.

| ID | Setup and action | Expected behavior and evidence |
| --- | --- | --- |
| A-T01 | Open a fresh account, revisit after reload, then remove access to the previous location | First launch opens Messages; later launch restores the last accessible location, draft, and reading position. Unavailable location falls back with explanation. Verify keyboard and phone composer/detail navigation |
| A-T02 | A asks which option to use. B posts unrelated progress. User says "the second one" | Route to A's pending question and original exchange. B does not take focus. Recipient indicator and routing reason agree. No selector call when the anchor resolves intent |
| A-T03 | Give two relevant agents an ambiguous new question; separately test two equally plausible pending questions and a selector timeout | At most one selector evaluates the message and chooses one relevant recipient where defensible. Ambiguity affecting meaning produces one short clarification, not broadcast or execution. Preserve the message; recipient override works |
| A-T04 | Mention B while replying to A; forward text mentioning C; ask "everyone" | B gets the request with A's message as context. Forwarded mention does not wake C. Everyone requests the recorded permitted roster, with allowance overflow visibly queued |
| A-T05 | Start six-turn discussion; invite another relevant participant, hand off repeatedly, emit reactions/progress, and restart before the sixth dispatch | Relevant invitation is possible without a user mention. Started/uncertain turns plus reservations never exceed six. Handoffs/restart do not reset allowance. Progress/reactions cause no wakeup. Seventh turn waits for deduplicated Continue |
| A-T06 | Get independent opinions from two identities on one runtime, then Discuss together | Separate initial sessions share the same permitted input snapshot. Peer answers are inaccessible through search, Resource retrieval, continuation, messages, or shared writes. Initial answers freeze before publication; later discussion cannot alter them |
| A-T07 | Put a synthetic unique marker only in a private DM, then run a group with that agent and request older context | Group packet, brief, continuation, retrieval, search, and runtime memory exclude the marker. Manifest access uses the same boundary. Session IDs differ. Revocation blocks new retrieval without claiming prior exposure was erased |
| A-T08 | Queue for an offline agent; fetch without answering; resolve/cancel before claim | Fetch does not complete it. Claim checks current state and closes stale requests without a model invocation. A valid request reads intervening messages and responds once in the original exchange |
| A-T09 | Correct a request before dispatch, during a turn, and just after completion | Pending request gets a new context generation. Qualified steering targets the expected turn; unsupported steering queues visibly and interrupts when needed. Initial manifest is immutable. Post-completion answer is labeled as predating correction; follow-up is queued |
| A-T10 | Replay send/claim/result operations, interrupt, press Stop, then reconnect an offline agent | Same logical command returns the same IDs. Started/uncertain invocation is not relaunched. Stop closes admission and cancels queued requests. Requested/acknowledged/observed termination stay separate; partial and late output survive |
| A-T11 | Two editors submit different changes against Resource revision 3; attach revision 3 in two conversations | One expected-revision write commits; the other conflicts and preserves its proposed edit. Attachments still open revision 3 and show newer-version availability. Linking does not alter permissions; previews/search/summaries cannot leak restricted content |
| A-T12 | Start from a long conversation with an incomplete brief; retrieve an older section; send a late correction | Packet includes request/reason, brief coverage, verbatim recent messages, explicit decisions/questions, pinned references, and scoped continuation. Omissions are visible. Manifest distinguishes initial context, later retrieval, and correction delivery. Original sources remain available |

Also test an offline or failed independent-opinion respondent. Ending an incomplete round cancels pending initial requests. Late results are labeled. Already-published answers cannot enter a response still described as independent.

## Stage-B acceptance specifications

| ID | Setup and action | Expected behavior and evidence |
| --- | --- | --- |
| B-T01 | Capture a title-only task, link the same Resource to a space and two projects, then propose tasks/dependencies/alternatives from a discussion | Capture needs no execution setup. Links preserve one Resource identity and do not expand access. Preview shows changes and affected approvals. Suggestions remain proposed until the proper apply/approval command. Replay does not duplicate tasks |
| B-T02 | Create a cycle, cancel a prerequisite, select an alternative, inspect a large synthetic roadmap | Reject cycle with its path. Cancellation does not satisfy prerequisite. Unselected branches remain visible and ineligible. Outline/blocker/agent reads are bounded with continuation; UI and API agree |
| B-T03 | Approve one task scope, then change a required revision or request out-of-scope repository work in chat | Ordinary chat/Resource edits use ordinary permissions. Repository work requires approved revision/actions. Changed requirements invalidate protected continuation and acceptance. Routing or newly drafted tasks cannot grant authority |
| B-T04 | Run a real worker in a disposable repository with approved baseline and task | Verify actual files/tools, checkpoint, result Resource, and claim/attempt/run IDs. Worker lacks publication credentials and cannot write outside its execution boundary. Schema/mocks cannot pass this case |
| B-T05 | Publish the attempt branch and create a linked draft PR | Task and conversation show the same task/attempt/PR, commits, checks, context, and Resources. Result is Ready for review, not accepted/merged/shipped. No auto merge or deployment occurs |
| B-T06 | Reorder/redeliver GitHub events, change PR head, close unmerged, and link two required PRs | Processing converges to actual GitHub state. Commit-specific checks/review assumptions refresh. Closed unmerged does not complete a task. Required PR conditions and owner acceptance remain distinct from related links |

## Stage-C acceptance specifications

| ID | Setup and action | Expected behavior and evidence |
| --- | --- | --- |
| C-T01 | Prepare two compatible tasks plus a third needing the same exclusive migration/database resource as one; start two slots | Compatible tasks run in separate worktrees. Conflict waits with a named lock reason. Claims/reservations are atomic across work sessions and hosts sharing the workspace. Unrelated tasks continue |
| C-T02 | Produce a prerequisite draft PR and try its dependent; manually review/merge prerequisite and retry | Draft does not unblock. After human acceptance/manual merge, record actual merged commit and target head, verify ancestry, and create dependent checkout containing it. Changed/reverted contracts need revalidation |
| C-T03 | Verify two candidate branches together, then move the target branch | Combine only in a disposable checkout; record input commits/results. Revalidate against the new target before claiming current evidence. Integration worker never changes protected target ref |
| C-T04 | Repeat an authenticated webhook; reuse key with changed input; launch two distinct overlapping sessions | Same trigger creates one session; changed input conflicts. Distinct sessions share canonical claims, locks, and host/provider capacity. No duplicate task execution |
| C-T05 | Exhaust quota, remove/stale telemetry, pass predicted reset time, and reach reserve boundary | Timestamped observations and actual rate-limit responses govern admission. Missing values remain unknown. Time/concurrency bounds still apply; predicted reset grants no capacity. Reserve stops new implementation while allowing bounded review/integration. No automatic billing switch |
| C-T06 | Crash after dispatch intent or runtime acceptance before receipt persistence, restart, and expire lease | Inspect original receipt/key. Uncertain acceptance keeps unsafe claims/reservations held and shows Needs reconciliation. No redispatch. Stale attempt cannot finalize. Stop acknowledgment cannot release an exclusive resource |
| C-T07 | Interrupt a worker with dirty files/checkpoint; resume after observed termination | Preserve worktree, baseline, Resources, and checkpoint. Choose resume/new attempt from evidence and retain lineage. Do not overwrite work or launch over an unknown process. Unsupported steering/replay remains explicit |
| C-T08 | Trigger through qualified AgentOS connection, reconnect, then restore backup containing an active receipt | AgentOS identity/declaration stays authoritative; one task graph/coordinator remains. Gaps are explicit. Restore retains records/manifests/receipts with dispatch inactive until reconciled. Digest separates drafts, review, acceptance, and manual merges |

After these cases, review the return-after-absence flow. Mischa should find questions, draft PRs, integration evidence, interruptions, and the next eligible action without reconstructing logs. Notifications collect meaningful changes, not every internal event.

## Verification and evidence discipline

Use real PostgreSQL for conflicts, claims, reservations, outbox delivery, and restart tests. Use connector fixtures for state transitions, then real authenticated model/tool runs for qualification. A mock can prove a denial path but cannot establish runtime memory isolation or cancellation.

Freeze routing examples and expected recipients. Include background focus changes, ambiguous pronouns, forwarded mentions, and corrections. Record incorrect routing and unnecessary invocations; tune policy without changing access/approval invariants. Do not evaluate only easy canned messages.

Test authorization, concurrency, stale revisions, context boundaries, and recovery against their concrete failure modes. Check keyboard operation, drafts, stable scroll, narrow layout, and offline/error states. Avoid tests that mirror implementation and broad reruns without changed risk. Use focused checks during packages and an integrated acceptance pass at each gate.

Record each case as passed, failed, or unverified, with source/runtime version, relevant IDs, inputs, observed outcome, and gaps. Track connector capabilities separately. An older project's acceptance record does not pass this application's test.

## What remains unverified

- Auto routing quality and useful context/brief sizes in daily conversations.
- Real session/memory isolation and staged independent opinions for either connector.
- Live Codex steering, interruption, recovery, and account quota telemetry.
- Hermes-through-AgentOS continuity, output limits, and real model/tool qualification.
- GitHub publisher enforcement, PR lifecycle, and combined integration in the new app.
- Appropriate task sizing and whether the initial 20% review/integration reserve is sufficient.

These are testable questions. They do not reopen the stack, Auto default, Resource model, or visual direction.

## Beyond these stages

Preserve large roadmaps, non-coding tasks, spaces, and cross-project Resource links in the product direction. Later add richer editing, selected NANC imports, broader connectors, hosting, and sharing when useful. Keep original projects and data intact. This planning revision starts no product implementation.
