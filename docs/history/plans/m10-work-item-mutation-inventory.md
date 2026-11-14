> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# M10 Work Item mutation inventory

Status: implementation baseline
Baseline: `61bb577034d77dbcdb1d06f141036f51ede4bb7f` (merged PR #86)
Recorded: August 19, 2026

## Baseline

- Rust `1.97.1`, Cargo `1.97.1`, Node `v24.18.1`, pnpm `11.18.0`, and
  PostgreSQL client `18.6` were present on the development Mac.
- GitHub CI run `32317094512` passed for the baseline commit, including its
  PostgreSQL service.
- `cargo test -p agent-workspace-server --test m4_work_items_http --no-run`
  compiled successfully before extraction.
- No PostgreSQL server was initially listening on `127.0.0.1:55432`; M10
  database evidence must therefore use a disposable local PostgreSQL 18
  cluster or CI rather than silently accepting the tests' no-database skip.

## Shared invariants

Every command below resolves the Actor from its authenticated browser session
or MCP credential. Project membership and resource identity are server-owned.
Work Item rows are locked before non-commutative changes; parent version changes
use optimistic comparison. Successful mutations keep the domain write,
activity/timeline, audit, transactional outbox invalidation, and addressed
external-agent events in one PostgreSQL transaction. The websocket is only an
invalidation signal; HTTP refetch remains canonical.

HTTP uses `Idempotency-Key`, Origin/CSRF checks, HTTP status mapping, and full
resource/conflict envelopes. MCP uses `clientRequestId`, scoped credential
authorization, stable `WriteResult` outcomes, and MCP-specific audit credential
metadata. Those transport contracts remain adapters and are intentionally not
collapsed.

## Mutation matrix

| Mutation | HTTP entry | MCP entry | Authorization and locks | Identity, writes, and side effects | Overlap / retained distinction |
| --- | --- | --- | --- | --- | --- |
| Create Work Item | `POST /api/v1/projects/:project_id/work-items` | `work_item_create` | `WorkItemCreate` for browser; `McpWrite` plus write scope for MCP; Project membership/counter lock | Normalized title/description/priority/assignee; Project-scoped idempotency; counter, default/selected type and status, board rank, Work Item v1, created activity, audit, `work_item.updated` outbox, addressed `work_item.assigned` | Canonical create command. HTTP may select type/status and returns a full resource; MCP uses defaults and returns `WriteResult`. |
| Create subtask | none | `work_item_create_subtask` | `McpWrite`; parent `FOR UPDATE` plus Project counter | Complete request hash including description; child v1 with parent ID; created activity, audit, outbox | Reuses canonical create primitives. Parent/archive validation and MCP result are distinct. |
| Update fields | `PATCH /api/v1/projects/:project_id/work-items/:id` | `work_item_update` | `WorkItemEdit` or `McpWrite`; membership and Work Item `FOR UPDATE` | Expected version; normalized title/description/priority/assignee; browser also type/status; one version bump, activity, audit, outbox, assignee-change and current-assignee external events | Canonical update command with typed optional fields. Browser keeps type/status support and full conflict envelope. |
| Set status | HTTP general update and board move | `work_item_set_status` | Same Work Item lock; allowed Project workflow transition | Resolve selected ID (HTTP) or one unambiguous ID/name/category (MCP); version bump and converged finalization | Uses canonical status transition/update primitive. Board rank allocation remains an HTTP-only command. |
| Move board rank | `POST .../:id/move` | none | `WorkItemEdit`; Work Item and affected status/rank rows locked | Expected version, transition, bounded rank allocation/rebalance, activity, audit, outbox, external change event | HTTP-only product semantic; shares locking, transition, version, and finalization primitives. |
| Archive / restore | `POST .../:id/archive`, `POST .../:id/restore` | none | `WorkItemEdit`; Work Item `FOR UPDATE` | Resource-scoped idempotency, exact version, archive attribution, one version bump/activity/audit/outbox/external change | HTTP-only; shares conflict and finalization primitives. |
| Post update / comment | `POST .../:id/comments` | `work_item_post_update` | `WorkItemEdit` or `McpWrite`; Work Item `FOR UPDATE` | Append-only comment, stable timeline sequence, audit, outbox at current version, external change event; transport-scoped replay | Canonical append primitive. HTTP accepts validated structured content with mentions/references; MCP converts bounded plain text. |
| Add criterion | `POST .../:id/acceptance-criteria` | none | `WorkItemEdit`; parent Work Item locked | Expected version and idempotency; normalized text, gap-free position, parent version bump, activity/audit/outbox/external event | HTTP-only; shares normalization/conflict/version/finalization primitives. |
| Update / complete / reorder criterion | `PATCH .../:id/acceptance-criteria/:criterion_id` | none | Same, plus criterion identity scoped to parent | Optional text/position/completed; completion Actor attribution; reorder serialization; one parent version bump and one side-effect set | HTTP-only canonical checklist semantic. |
| Remove criterion | `DELETE .../:id/acceptance-criteria/:criterion_id` | none | Same parent lock | Expected version; delete relation row, compact positions, one parent version bump and side-effect set | HTTP-only. |
| Link / unlink Thread | `POST` / `DELETE .../:id/threads/:conversation_id` | none | `WorkItemEdit`; membership and Work Item lock; same-Project thread validation | Idempotent relation change; version bump only on change; activity/audit/outbox/external event | HTTP-only. Link can be initiated from either detail context but uses one domain relation. |
| Link / unlink repository | `POST` / `DELETE .../:id/repositories/:repository_id` | none | `WorkItemEdit`; Work Item lock; same-Project repository validation | Expected version where applicable; relation role, activity/audit/outbox/external event | HTTP-only. Repository eligibility and role semantics remain explicit. |
| Choose primary repository | `POST .../:id/repositories/:repository_id/primary` | none | Same, plus readiness/trust checks and partial uniqueness | Optimistic primary swap, one version bump and side-effect set | HTTP-only; no Git/worktree mutation. |
| Link pull request | no direct Work Item HTTP route; GitHub commands/automation also create explicit links | `work_item_link_pull_request` | `McpWrite`; Work Item lock; same-Project canonical PR check | Expected version; idempotent explicit confirmed relation; bump/activity/audit/outbox only when inserted | MCP tool is intentionally distinct from GitHub webhook/publish commands; shared Work Item locking/version/finalization applies. |
| Mark blocked | none | `work_item_mark_blocked` | `McpWrite`; Work Item lock | Expected version; bounded reason and blocker attribution; version/activity/audit/outbox/external event | MCP-only semantic introduced for agent coordination; not mapped to priority/status. |
| Ready for review | none | `work_item_mark_ready_for_review` | `McpWrite`; Work Item lock; allowed Review transition | Clears blocker, moves to Review, bumps version, finalizes once | MCP convenience command remains distinct but uses canonical transition/finalization. |
| Work with Worker | `POST .../:id/work-with-worker` | external sponsored execution is a separate MCP flow | Human `WorkItemEdit` plus worker/run admission policies; locks Work Item and idempotency scope | Creates Thread/link/activity and later run identity; preserves empty linked Thread if run start fails | Deliberately not a generic Work Item update. External sponsorship has separate human approval and profile ownership. |

## Existing characterization coverage

- `apps/server/tests/m4_work_items_http.rs`: HTTP create/replay/conflict,
  field edits, archive/restore, criteria, comments, timeline, links, board,
  concurrency, attribution, outbox, and policy boundaries.
- `apps/server/tests/m6_mcp_foundation.rs`: MCP schemas and result shapes,
  create/update/status/post/subtask/link/block/review, replay, stale version,
  viewer denial, OAuth, and audit attribution.
- `apps/server/tests/m9_external_agents.rs`: external-agent create/subtask
  description hash identity, Work Item event recipients, requester exclusion,
  credential lifecycle, and sponsored execution boundaries.
- `apps/server/tests/m2_concurrent_commands.rs` and
  `apps/server/tests/m2_outbox_publisher.rs`: transaction ordering and durable
  publication behavior reused by M10.

## M10 command boundary

`work_item_commands` is the internal application layer. Wire adapters retain
request parsing, browser write protection, idempotency key extraction/replay
encoding, and response mapping. The shared layer owns normalization,
authorization helpers, Work Item locking/conflict state, workflow and assignee
validation, version advancement, activity/timeline insertion, audit
attribution, outbox invalidation, and addressed external-event finalization.

No task microservice, second database, synchronization mechanism, dedicated
task UI, public API, or shared external SDK is introduced.
