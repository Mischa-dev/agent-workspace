# Shared-helper audit plan — agent-workspace permission paths

Companion to `verdicts.md`. The Nov 6 recommendation for PR #194 was: stop
patching read paths one by one; audit every read path onto one shared helper
that enforces the *target's own* effective permissions. This plan records
what that audit finds on current main (`9227e49`), because the answer is
mostly "already done" — and then specifies the remaining work in order.

## 1. What already exists (the helper layer is built)

The refactor the Nov 6 triage asked for has largely happened in substance,
driven by the #194/#100 fix rounds rather than by one audit PR. The canonical
helpers on current main:

**SQL predicates (the layer read paths should route through):**

| Helper | Where | Semantics |
|---|---|---|
| `conversation_visible_to(conv, actor)` | migration 0037 lineage | Private DM/group/thread visibility; AGENTS.md already mandates it for every conversation query |
| `pull_request_visible_to(pr, actor)` | migration `0050` | Repo/PR visibility for work-item projections |
| `actor_can_access_project(viewer, connection, target)` | migration `0042` | **The** target-access predicate: for a person, enabled project membership **and** enabled org membership, active actor/user; for an agent, defers to `agent_connection_can_access`. Caveat: excludes archived projects by design |
| `agent_connection_can_access(connection, project)` | SQL | Cross-project agent reach (approvals, sponsored runs) |
| `custom_role_allows(role_id, '<perm>')` | SQL | In-query custom-role permission check (`files.rs` READABLE, `comments.rs` moderation) |

**Rust resolvers:**

| Helper | Where | Role |
|---|---|---|
| `roles::effective(grant, limit, kind)` | `roles.rs:96` | Role permissions ∩ agent limit — the base narrowing op |
| `roles::project_holding(conn, actor, project)` | `roles.rs:275` | Caller's effective role **in the target project**, limit-cut — the target-scope resolver |
| `roles::standing(conn, org, actor)` | `roles.rs:320` | Workspace-scope equivalent |
| `roles::issuer_still_grants(...)` | `roles.rs:360` | Accept-time re-verification that an invitation's issuer still grants the role |
| `memberships::require_grantable(...)` | `memberships.rs:414` | Refuses any role grant not a subset of the caller's standing **and** project holding |
| `policy::authorize(...)` + `agent_limit_hides(...)` | `policy.rs:1074,788` | Central decision point; applies the current agent limit, can deny with `DenyReason::AgentLimit` |

**MCP layer:**

| Helper | Where | Role |
|---|---|---|
| `common::accessible_project_ids(state, principal)` | `mcp/common.rs` | Every project reachable *now*, via `actor_can_access_project` |
| `common::parse_id(value)` | `mcp/common.rs:440` | UUID **or** canonical URL ending in one |
| Middleware `keys_to_ids` / `scoped_project` + `PRIMARY_SCOPES` | `mcp.rs:4408,4792` | Resolves a resource's own project (with its visibility predicate) and writes it into the principal before the tool runs |
| `project_by_key` / `repository_by_name` | `mcp.rs:4588,4550` | Key resolvers that **refuse ambiguity** (`LIMIT 2` → None) — the house convention |
| `files::readable_file` + `READABLE` | `files.rs:216,136` | File reads checked against the file's own place (incl. parent task's `tasks.view`) + agent limit |
| `work_items::comments::is_comment_moderator` | `comments.rs:757` | Shared, agent-limit-aware moderation check |

**Design rule (already the de-facto convention; make it explicit):** a read
or write path never hand-rolls a membership join. It either (a) filters in
SQL with the target's predicate (`conversation_visible_to`,
`pull_request_visible_to`, `actor_can_access_project`), or (b) resolves in
Rust through `project_holding` / `standing` cut by `roles::effective` and
decides through `policy::authorize`. A new hand-rolled
`project_memberships`-only join in a query is a review red flag — two of the
three surviving pattern items below are exactly that shape or its
check-then-act variant.

## 2. Which paths route where (adjudicated state)

Correctly routed today (findings defeated by this routing): file reads
(`readable_file`), task-comment moderation (`is_comment_moderator`), role
propagation (`manage_refusal` → `authorize` per project), invitation issue /
replace / accept (`authorize_manage` + `require_grantable` +
`issuer_still_grants`, owner-only for built-in admin), OAuth agent
connections (`managed_agent` subset checks), approvals and sponsored runs
(`scoped_project` + `agent_connection_can_access`), task moves
(`actor_can_access_project` on the actual destination +
`projects_showing_tasks`), message-removal thread lookups (entry lock via
`lock_message_conversation`), browser work-item PR projection
(`pull_request_visible_to`), connected-app grant management (org-membership
inner join in `owned_grant`), archived-project *non*-archived sibling path
(`accessible_project_ids`).

## 3. Remaining gaps, in implementation order

### Step 1 — Briefing archived-projects query (standing leak; patch drafted)
`mcp/briefing.rs:363`: the `includeArchived` extension hand-rolls
`project_memberships`-only. `actor_can_access_project` can't be applied
verbatim (it excludes archived projects — that's why this branch exists),
so the patch adds the missing half of its person-branch: the enabled
`organization_memberships` join. Longer term, fold this into an
archived-inclusive variant of `common::accessible_project_ids` so there is
one function to audit. Patch: `patch-pr104-briefing-archived-org.patch`.

### Step 2 — `timeline_requests` takes a viewer (TOCTOU, metadata)
`replies.rs:714` / `conversations.rs:2660`: change the signature to accept
the viewer's actor id and add
`AND conversation_visible_to(<conversation>, $viewer)` to both queries,
matching `fetch_conversation_access`. Small, but touches callers and the
function's tests — its own commit, not folded into step 1.

### Step 3 — Executor-generic `record_visible` + in-transaction recheck (TOCTOU, write)
`mcp/tasks.rs:3960` is pool-only (`&AppState`); the write in
`change_record_link` runs later, inside a transaction, with no recheck.
Re-parameterize `record_visible` over an executor (the pattern
`accessible_project_ids_in` in `mcp/common.rs` already demonstrates) and
re-run it inside the transaction before the insert/delete — or lock the
conversation row via `conversations::lock_conversation_for`
(`conversations.rs:3893`), which re-evaluates `conversation_visible_to`
after locking. This is the one genuinely new helper variant the audit
needs; everything else is routing.

### Step 4 — Resolver-ambiguity convention (integrity; patches drafted)
`project_by_key` and `repository_by_name` already refuse ambiguity;
`project_reference` (admin.rs) and `task_by_key` (mcp.rs) didn't. Patches
align both (`patch-pr104-project-key-ambiguity.patch`,
`patch-pr104-task-key-ambiguity.patch`). To keep it from recurring, the
convention belongs in one sentence in the MCP section of the contributor
docs: *key/name resolvers fetch `LIMIT 2` and return None on more than one
distinct match.* Optionally extract a tiny `resolve_unique(matches)` Rust
helper for the three call sites — worth it only if a fourth resolver
appears.

### Step 5 — Handler-level `parse_id` cleanup (functionality residual)
A few handlers still parse bare UUIDs (`mcp/approvals.rs:154,185`,
`external_execution` argument parsing) while the middleware resolves URLs
fine. Route them through `common::parse_id`. No security content; bundle
with any nearby MCP work.

### Step 6 — Separate functional gap found while adjudicating (not a finding)
Approvals with `agent_run_id IS NULL` (sponsored external-execution
approvals) inner-join `agent_runs` in both approval contexts and are
unfetchable via `approval_get`/`approval_respond` in any project. File as
its own ticket; it is a break, not a leak.

## 4. Confirmed non-pattern items — scoped follow-ups (not helper work)

| Item | Follow-up shape |
|---|---|
| PR #35 GitHub lease (github.rs) | Mirror `jobs.rs::renew_lease` (renew at ~lease/3 scoped by `lease_owner`, abort on loss) across the three GitHub job kinds + a concurrency test. Own PR. |
| PR #172 refresh grace (oauth.rs) | **Decision first**: cap grace-issued token expiry at retired `used_at + grace`, or access-token-only in the grace branch. Mechanically one function; changes parallel-session semantics. |
| PR #6 approval details (codex_app_server.rs) | Build the summary from bounded/redacted params via `run_log::redact` + `cap`, mirroring `claude_code.rs:951`; amend the adapter test that currently asserts detail-less summaries. Own PR. |
| PR #17 retry guard (provider/scheduler) | Store a machine-readable unresolved-turn marker at interruption, or re-run `inspect_provider_for_recovery` before a retry resumes a session. Own PR. |
| PR #157 mail (patch drafted) | Land `patch-pr157-mail-owner-only.patch` **with** a Postgres integration test (owner/member × default/non-default × mail on/off), per AGENTS.md's "tests where a bug would be silent and costly: credentials". |
| PR #36 / #76 / #128 patches | Land with their neighbouring unit tests: reuse-clears-publication (scheduler), ignored-only worktree retained (git_runtime, tests at `:1516ff` are the pattern), DNS timeout constant mirroring `outbound.rs` (replies). |

## 5. Keeping it closed

The reason the sweep over-counted is also the reason this stays closed:
fix rounds here already write *pinning* tests (often asserting the exact
attack input) and comments naming the attack. Two cheap habits preserve
that: (1) when a Codex/security finding is fixed in a later round, resolve
the review thread in the same PR — 29 of 48 P1s were zombies kept alive
only by unresolved threads; (2) treat any new `project_memberships`-only
join in a read/write query as a review blocker unless the target's
predicate appears in the same query (steps 1–3 are the current exceptions
list; it should only shrink).
