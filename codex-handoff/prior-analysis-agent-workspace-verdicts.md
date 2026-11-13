# Agent-workspace P1 adjudication — verdicts

Adjudication of the 48 agent-workspace P1 items from
`~/workspace/proactive/codex-sweep-2026-11-09.md`, traced item-by-item against
current main. Full traced evidence per item lives in `parts/cluster-A.md` …
`parts/cluster-E.md`; this file is the consolidated verdict list.

## Baseline (verified Nov 10, 2026)

- Current main is still `9227e49d41ad3b892f5d2bae47d34e323e7d9efa` (the PR #204
  merge) — identical to the Nov 7/8 checks. Nothing has merged since.
- PR states re-verified via `gh api` REST for all 25 source PRs: **all merged**,
  including PR #194 (merged 2026-11-06T15:31:54Z, head `b17928be`). The sweep's
  "(merged)" labels are correct in outcome; the important correction is the
  reverse of the usual one — several findings were raised against PRs whose own
  later fix rounds (and follow-up PRs like #100 "Fix review findings from PRs
  96–98") landed the defeating checks *before or during merge*, so
  code-presence matching alone kept them listed as standing.
- Method per item: read the claim, trace the full path in current main
  (callers, guards, helpers, state machine), verdict on the code as it is now.

## Tally

| Verdict | Count | Share |
|---|---|---|
| CONFIRMED | 14 | 29% |
| FALSE POSITIVE (defeating check named) | 29 | 60% |
| DESIGN CALL (tradeoff stated) | 5 | 10% |
| CANNOT REPRODUCE | 0 | — |
| **Total** | **48** | |

The sweep over-counted by ~3.4×. The dominant cause is not sloppy sweeping —
it is that this codebase's fix rounds routinely add the exact check Codex
asked for, often with a comment naming the attack, and neither the thread
state nor code-presence matching can see that.

## The #194 pattern — final count

- 18 of the 48 items belong to the permissions/read-path family (all of
  cluster D). Of those, **3 are confirmed instances of the #194 hole**
  (a read/write path substituting a weaker or stale check for the target's
  own effective permission): briefing archived-projects (standing leak),
  timeline reply metadata (TOCTOU variant), task-link write (TOCTOU variant).
- **All five literal PR #194 findings are FALSE POSITIVE on current main.**
  The checks Codex asked for exist, are agent-limit-aware, and in three cases
  carry comments describing the claimed attack verbatim.
- Two further confirmed cluster-D items (ambiguous project/task keys) are an
  integrity family, not the permission hole: the caller legitimately reaches
  both targets; the resolver silently picks one.
- Conclusion for the pattern: the shared-helper refactor recommended Nov 6
  has *largely already happened in substance* (see audit-plan.md) — what
  remains is three stragglers and a resolver-ambiguity convention gap, not a
  systemic hole.

## Worst confirmed item

**PR #172 — OAuth refresh-grace branch mints a permanent token**
(`apps/server/src/mcp/oauth.rs:938-954`). Replaying a stolen refresh token
within the 300 s grace window still issues a brand-new refresh token with
`used_at` NULL and a fresh schema-default 30-day life. Commit `101030e`
narrowed it (shared retirement on normal rotation), but revocation is
victim-triggered: the integration test itself asserts a thief's grace-branch
rotation succeeds and yields the grant's sole live token, so a thief who
rotates first can keep rolling the grant while the legitimate app is dormant.
Runners-up: PR #35 (credentialed GitHub publish can execute twice — 30 s
non-renewed lease vs ~150 s of bounded token/ref/push work, `github.rs:54`),
PR #6 item 2 (every Codex approval reaches the human as a generic sentence —
command, path, reason and scope all discarded, `codex_app_server.rs:1441`),
PR #157 (reset tokens can still transit a merely-joined workspace's SMTP when
that workspace is the person's computed default, `mail/mod.rs:316`).

---

## Verdicts by group

Line numbers are current main. "Pattern" tags: `194-pattern` = the #194 hole;
`resolver-ambiguity` = wrong-target family; `—` = outside both families.

### Group P — permissions / read-path family (cluster D, 18 items)

1. **PR #90 — Recheck visibility when loading private reply metadata** —
   `conversations.rs:2660`, `replies.rs:714` — **CONFIRMED** (narrow TOCTOU,
   metadata only). `timeline_requests` takes no viewer and has no
   `conversation_visible_to`; the visibility check happens in a prior,
   separate pool call. Pattern: `194-pattern` (race variant). Helper it
   should use: `conversation_visible_to` inside `timeline_requests` (add a
   viewer parameter). No patch drafted — signature change touches callers;
   small but not one-file.
2. **PR #96 — Keep human project and organization roles aligned** —
   `memberships.rs:387` — **FALSE POSITIVE**. The `sync_organization_role`
   call the claim says was removed is present, in the same transaction;
   remaining divergences (explicit org roles, custom roles) are deliberate
   and documented. Pattern: —.
3. **PR #96 — Authorize approvals against the selected project** —
   `mcp.rs:4692`, `mcp/approvals.rs:480,730` — **FALSE POSITIVE**. An
   `approvalId` scope lookup exists in `PRIMARY_SCOPES`; person context joins
   the approval's own conversation project and requires
   `actor_can_access_project`; agent context binds the resolved project plus
   `agent_connection_can_access`. Residual (not the claim): approvals with
   `agent_run_id IS NULL` aren't fetchable via `approval_get` at all.
   Pattern: —.
4. **PR #96 — Authorize sponsored runs in the resolved project** —
   `external_execution.rs:238` — **FALSE POSITIVE**. Middleware resolves the
   conversation's project into `principal.project_id` first; the handler
   joins membership on that project and requires
   `agent_connection_can_access`. Pattern: —.
5. **PR #99 — Parse resource URLs before resolving their project** —
   `mcp/common.rs:440` (`parse_id`) — **FALSE POSITIVE** for the claimed
   middleware path: `parse_id` accepts UUID-or-URL-ending-in-UUID and is
   unit-tested; `scoped_project`/`first_scope` route through it. Residual:
   a few handlers (`approvals::get`/`respond`, `external_execution`) still
   parse bare UUIDs — cleanup ticket, not the claimed failure. Pattern: —.
6. **PR #99 — Require current organization membership to manage agent
   grants** — `connected_apps.rs:147` — **FALSE POSITIVE**. `owned_grant`'s
   manager branch inner-joins `organization_memberships … AND enabled`; a
   disabled org membership drops the manager row entirely. Pattern: —.
7. **PR #104 — Reject ambiguous project keys before archiving** —
   `mcp/admin.rs:1048` (`project_reference`) — **CONFIRMED**.
   `LIMIT 2` fetched, then `matches.first()` silently wins; two orgs sharing
   a key → archive/restore can hit the wrong org's project. Sibling
   `project_by_key` (mcp.rs:4588) already refuses ambiguity. Pattern:
   `resolver-ambiguity`. **Draft patch:** `patch-pr104-project-key-ambiguity.patch`.
8. **PR #104 — Reject ambiguous task keys before rewriting IDs** —
   `mcp.rs:4625` (`task_by_key`) — **CONFIRMED**. `ORDER BY preference
   LIMIT 1` picks an unspecified organization's task when two hold the same
   key+number; both neighbouring resolvers refuse ambiguity, this one
   doesn't. Pattern: `resolver-ambiguity`. **Draft patch:**
   `patch-pr104-task-key-ambiguity.patch`.
9. **PR #104 — Check current org membership for archived projects** —
   `mcp/briefing.rs:363` — **CONFIRMED**. The `includeArchived` branch
   hand-rolls a `project_memberships`-only join, bypassing
   `actor_can_access_project`; a person whose org membership was disabled
   keeps listing that workspace's archived projects (key, name, open-task
   counts) indefinitely. The cleanest standing #194-pattern leak of the 48.
   Pattern: `194-pattern`. Helper it should use: the person-branch of
   `actor_can_access_project` minus its archived exclusion — i.e. add the
   enabled `organization_memberships` join (the predicate itself excludes
   archived projects, so it cannot be applied verbatim). **Draft patch:**
   `patch-pr104-briefing-archived-org.patch`.
10. **PR #104 — Recheck access to the actual move target** —
    `mcp/task_move.rs:938` — **FALSE POSITIVE**. Three layers check the
    actual destination: dual `authorize_mutation`, the `already_moved` query
    filters on `actor_can_access_project` for the destination, and titles
    are blanked unless `projects_showing_tasks` passes. Pattern: —.
11. **PR #105 — Recheck conversation visibility inside the write
    transaction** — `mcp/tasks.rs:3701,3960,4030` — **CONFIRMED** (TOCTOU on
    a write). `record_visible` runs on the pool before the transaction;
    `change_record_link` inside it has no visibility predicate. Pattern:
    `194-pattern` (atomicity variant). Helper needed: an executor-generic
    in-transaction variant of `record_visible` — **none exists** (see
    audit-plan.md); not patch-drafted (helper work, not a one-liner).
12. **PR #107 — Route thread lookups through conversation visibility** —
    `message_edits.rs:1000` — **FALSE POSITIVE**. The thread queries do lack
    the literal predicate, but every path enters through
    `lock_message_for_change`, which locks and visibility-checks the only
    conversation those threads can belong to (threads are bound by
    `origin_message_id` to the locked message's conversation). Pattern: —.
13. **PR #109 — Apply PR visibility to browser work-item reads** —
    `work_items.rs:3895` — **FALSE POSITIVE**. `WORK_ITEM_SELECT`'s linked-PR
    subquery applies `pull_request_visible_to(pull_request.id, $3)`, and
    `work_item_view` additionally drops linked PRs without code capability.
    Pattern: —.
14. **PR #194 — Enforce task-view permissions when reading task
    attachments** — `files.rs:136` (`READABLE`), `files.rs:216` — **FALSE
    POSITIVE**. `file_read` goes through `files::readable_file`; the
    `READABLE` predicate's task-file branch requires
    `custom_role_allows(…, 'tasks.view')` in the file's project, and
    `hidden_by_agent_limit` applies the caller's agent limit on top.
    Pattern: — (this is the pattern's namesake claim — defeated).
15. **PR #194 — Avoid promoting custom project invitees to workspace
    Admin** — `invitations.rs:248,1126,1160` — **FALSE POSITIVE**, judged on
    the upstream issuance question per the calibration. The acceptance-side
    guard (`role == "admin" && role_id.is_none()`) is as stipulated; the
    issuance trace defeats the rest: `authorize_manage` requires org
    owner/admin + project `InvitationManage`, and `admin_invite_requires_owner`
    (invitations.rs:1160) makes built-in-admin invitations **owner-only** —
    pinned by the unit test `organization_role_owns_invitation_authority`.
    `require_grantable` bounds any invited role by the issuer's standing and
    project holding, `pick_custom_role` keeps custom roles at
    `role_id = Some(..)` so they can never satisfy the promotion guard, and
    `roles::issuer_still_grants` re-verifies at open/accept time. A
    project-scoped inviter cannot mint a workspace-Admin invite. Pattern: —.
16. **PR #194 — Apply agent limits when authorizing task-comment
    moderation** — `work_items/comments.rs:757` (`is_comment_moderator`) —
    **FALSE POSITIVE**. The shared check starts with
    `agent_limit_hides(actor_id, project_id, Permission::ChannelsModerate)`
    → not a moderator, with a comment naming this exact scenario; then
    requires owner/admin membership with
    `custom_role_allows(role_id, 'channels.moderate')`. Pattern: —.
17. **PR #194 — Bound OAuth agent credential issuance by grantable
    permissions** — `mcp/oauth.rs:1302` (`managed_agent`) — **FALSE
    POSITIVE**. `managed_agent` refuses the connection unless the agent's
    grant is a subset of the manager's limit-cut `roles::standing` **and**
    of their `project_holding` in the agent's project — the claimed-missing
    bound, with a comment naming it. Pattern: —.
18. **PR #194 — Check project management permission before propagating
    roles** — `roles.rs:1472,1612,1650` — **FALSE POSITIVE**.
    `apply_to_their_projects` evaluates each project individually:
    `manage_refusal` authorizes `MembershipManage` against the caller's own
    role *in that project* through `policy::authorize` (agent-limit-aware),
    and the propagated role must be a subset of `project_holding` there.
    Failing projects are skipped and reported, not updated. Pattern: —.

### Group S — OAuth / webhooks / auth secrets (cluster C, 8 items)

19. **PR #90 — Preserve the OAuth grant's approved scopes** —
    `mcp/oauth.rs:1214,1258` — **DESIGN CALL**. The claimed escalation
    (read-only approval → full connection scopes) no longer reproduces
    (item 20). What remains is deliberate: for a *full* grant,
    `agent_grant_scopes` returns the connection's list verbatim (test-asserted),
    bounded by the connection and disclosed in the consent UI ("uses the
    agent's own permissions"). Noted gap: the token response's `scope`
    string still reports only the grant scopes (`read write`), understating
    effective scopes. Policy decision, no patch.
20. **PR #99 — Enforce read-only access for agent grants** —
    `mcp/oauth.rs:1221,1258` — **FALSE POSITIVE**. `agent_grant_scopes`
    drops `write`/`approval_respond`/`worker_request` for read grants;
    unit-test pinned (`read_only_consent_keeps_an_agent_grant_read_only`).
    Backing-credential bypass checked: consumers gate on the filtered
    principal scopes first.
21. **PR #172 — Avoid granting a permanent branch during refresh grace** —
    `mcp/oauth.rs:921-985` — **CONFIRMED (bounded, partially mitigated)** —
    see "Worst confirmed item" above. Fix direction: cap the grace-issued
    token's `expires_at` at the retired token's `used_at + grace`, or return
    an access token only. Mechanically small, but it changes parallel-session
    semantics — **not drafted; needs Mischa's call on the tradeoff first**.
22. **PR #90 — Keep webhook signing keys out of database records** —
    migration `0038:69` vs `20261016070100_encrypted_secrets.sql` —
    **FALSE POSITIVE** on current main. All write paths store via the
    AES-256-GCM secrets store and set `webhook_secret = NULL` +
    `webhook_secret_id`; a startup migration clears legacy rows. Residual
    caveat: pre-S1 plaintext rows persist in a deployment with no master key
    configured (warned at `secrets.rs:1191`); optional follow-up: drop the
    column once migration has run in production.
23. **PR #90 — Reject private webhook destinations** —
    `replies.rs:1560,1597` — **FALSE POSITIVE**, defeated twice:
    config-time `validate_webhook_url` (https-only, public IPs only, no
    credentials) and delivery-time `webhook_client` (resolve once, reject
    if any address is non-public, pin the checked address, no redirects or
    proxies). Private destinations exist only via operator-configured
    trusted origins — project owners cannot self-authorize.
24. **PR #90 — Keep webhook signing secrets out of browser responses** —
    `external_agents.rs:588,673` — **DESIGN CALL**. The secret is returned
    exactly once, to the authorized manager who just created/rotated it;
    `ConnectionView` (every read path) has no secret field; the frontend
    keeps it in component state only. Same show-once pattern as the one-time
    credential in the same response. Tension noted with AGENTS.md's "never
    expose secrets to the browser" — the codebase treats show-once issuance
    as the exception. Policy decision, no patch.
25. **PR #100 — Wait for the claimed WorkOS session check** —
    `auth/workos.rs:657` — **DESIGN CALL**. The mechanism is exactly as
    claimed (`if !claimed { return Ok(true) }`), but it is commented,
    intentional fail-open: concurrent requests "keep the session until it
    resolves"; the claimer revokes on a negative result. Exposure is the
    requests racing one provider call per 5-minute window. Fail-closed
    would trade this for latency/spurious logouts. No patch.
26. **PR #181 — Keep bearer tokens out of traced URL paths** —
    `lib.rs:272` (`logged_path`), `bug_reports.rs:70` — **FALSE POSITIVE**.
    The TraceLayer records the MatchedPath pattern
    `/api/v1/bug-reports/{token}`, added for exactly this (commit `67b1ec9`)
    with an integration test asserting the token never appears in captured
    logs; the metrics route-class drops token segments too.

### Group R — provider / recovery (cluster A, 8 items)

27. **PR #6 — Reject failed or interrupted provider turns** —
    `provider/codex_app_server.rs:1086`, `provider.rs:762` — **FALSE
    POSITIVE**. Adapter parses strictly (`completed: status == "completed"`)
    and `drive_turn` rejects `!completed` with a conflict error that fails
    the run; recovery likewise only treats `"completed"` as completed.
28. **PR #6 — Preserve approval details before asking the user** —
    `provider/codex_app_server.rs:1441` — **CONFIRMED**. Every command /
    file-change / permission approval is summarized as the constant "The
    agent needs permission before it can continue."; command, paths, reason
    and network scope from the pinned schema are all discarded, and the
    durable `agent_approvals` record stores only that summary. The Claude
    adapter proves the house pattern (bounded, redacted detail,
    `claude_code.rs:951`). Runner-up for worst confirmed. Not drafted —
    the fix spans parsing, summary construction and the adapter's privacy
    test; it deserves its own scoped PR.
29. **PR #6 — Preserve notifications queued before the turn loop** —
    `tools/codex-bridge-spike/src/lib.rs:211` — **FALSE POSITIVE**.
    `next_raw_event()` drains the buffered `notifications` queue first, in
    arrival order; the compat doc records the same contract.
30. **PR #6 — Route pre-response approvals through the application** —
    `provider/codex_app_server.rs:1018` — **FALSE POSITIVE** for production:
    the adapter sets `automatic_server_request_responses: false`, so a
    mid-request server request fails closed
    (`ServerRequestDuringPendingRequest`) instead of being auto-answered;
    the auto-accept path is live only in the standalone spike binary.
    Residual nuance: mid-request approvals error the transport rather than
    routing to the approval flow — fail-closed, a lesser separate behavior.
31. **PR #6 — Cancel pending approvals during startup recovery** —
    `recovery.rs:182,284`, `main.rs:352` — **FALSE POSITIVE**. Cancellation
    happens in the same transaction as the Interrupted transition, plus a
    startup-wide sweep `cancel_terminal_run_approvals` for any terminal run.
32. **PR #6 — Reconcile runs already marked recovering** —
    `recovery.rs:52`, `domain.rs:241` — **FALSE POSITIVE**. The startup
    query explicitly includes `'recovering'`; same-state transition is a
    no-op and all recovery exits exist from `Recovering`.
33. **PR #17 — Reject retries while the provider turn is unresolved** —
    `scheduler.rs:701`, `provider.rs:280,343`, `api.rs:601` — **CONFIRMED**
    at code level. Recovery interrupts a local run whose turn reads
    `inProgress` (reason stored as free text only); retry admission checks
    only DB state, workspace reuse checks only Git state, and `start_run`
    resumes the provider thread and starts a new turn unconditionally —
    nothing re-inspects provider turn state. Caveat: whether Codex rejects
    a second `turn/start` in that state is undocumented provider behavior;
    the app-level guard is absent regardless. Not drafted — fix needs the
    recovery-inspection machinery wired into the retry path.
34. **PR #19 — Record the new pinned provider response contract** —
    `docs/compatibility/codex-app-server.md` — **CONFIRMED,
    documentation-only**. Recovery genuinely depends on `thread/read`
    returning `thread.turns[].id`/`.status` with exact-id matching
    (`provider.rs:563`), and the doc never pins that shape. **Draft patch:**
    `patch-pr19-pinned-contract-doc.patch`.

### Group G — git / GitHub jobs / host maintenance (cluster B, 8 items)

35. **PR #35 — Extend the lease beyond the credentialed network operation**
    — `github.rs:54,2231,2620` — **CONFIRMED**. Flat 30 s lease, never
    renewed in github.rs (the generic job system renews: `jobs.rs:696`);
    the publish path's bounded work totals ~150 s (token 20 s + ref 20 s +
    push 90 s + ref 20 s); the 1 s worker loop plus concurrent `kick_jobs`
    reclaim via the stale sweep, so a credentialed push can execute twice.
    The lease-owner guard only voids the first worker's DB completion, not
    its push. Not drafted — renewal spans three job kinds and needs a
    concurrency test.
36. **PR #36 — Require publication by the selected Agent Run** —
    `scheduler.rs:525`, `github.rs:1908,1967` — **CONFIRMED**. The retry
    reuse UPDATE sets only `state='active'`; `published_commit` is never
    cleared anywhere, and `create_pull_request` gates only on
    `published_commit IS NOT NULL`, using the predecessor's commit as the
    retry run's PR head. **Draft patch:**
    `patch-pr36-retry-publication.patch` (clear the publication fields in
    the reuse UPDATE — the publication shape constraint requires the three
    fields be nulled together).
37. **PR #41 — Restrict merge-SHA fallback to merged close deliveries** —
    `github.rs:4995,3475` — **FALSE POSITIVE**. `bounded_job_payload`
    retains `merge_commit_sha` only for `pull_request`/`closed`/`merged=true`
    (tests pin synchronize/review dropping it), and
    `reconcile_merge_commit_identity` fills only when live REST says merged.
38. **PR #68 — Pass --dbname to pg_restore** —
    `tools/host-maintenance/src/lib.rs:1190` — **FALSE POSITIVE**. Restore
    passes `--dbname=` (empty, forcing connect mode) with the target in
    `PGDATABASE` env; the wrapper forwards both; test-pinned.
39. **PR #68 — Sanitize audit FKs when omitting credential rows** —
    `tools/host-maintenance/src/lib.rs:1179` — **FALSE POSITIVE**. The
    sanitizer nulls all three FKs (`session_id`, `mcp_credential_id`,
    `mcp_oauth_grant_id`) between the data and post-data sections; the exact
    UPDATE is test-pinned. Same defect as item 40 — close together.
40. **PR #71 — Clear all excluded audit foreign keys before post-data** —
    same site — **FALSE POSITIVE** (same defect as item 39, raised twice;
    the merged state fixes both).
41. **PR #76 — Skip symlinks during storage admission** —
    `storage_admission.rs:43` — **FALSE POSITIVE**. The scan `continue`s on
    symlinks with a comment matching the finding's scenario; test
    `symlinks_are_skipped_without_following_targets` pins it.
42. **PR #76 — Preserve ignored files before removing worktrees** —
    `git_runtime.rs:761` (`worktree_is_dirty`), remove paths `:558,:580` —
    **CONFIRMED**, and empirically reproduced by the adjudicator with git
    2.43: an ignored-only worktree reports clean, and plain
    `git worktree remove` exits 0 and deletes the ignored files. Retention
    drives this in production (`retention.rs:160`). **Draft patch:**
    `patch-pr76-ignored-files.patch` — scoped to the removal paths only
    (publish/retry dirtiness keeps its narrower meaning), per the scope
    choice flagged in cluster-B.

### Group M — misc (cluster E, 6 items)

43. **PR #128 — Bound DNS resolution before blocking the reply worker** —
    `replies.rs:1611` — **CONFIRMED**. `lookup_host().await` in
    `webhook_client` has no timeout; `WEBHOOK_TIMEOUT` (8 s) is only set on
    the reqwest client built after resolution. The sequential `run()` loop
    and `FOR UPDATE` delivery rows amplify a stall. Both sibling resolvers
    bound this exact call (`outbound.rs:79` 10 s, `link_previews/guard.rs:90`
    3 s) — replies.rs is the outlier. **Draft patch:**
    `patch-pr128-dns-timeout.patch`.
44. **PR #128 — Preserve the deployed private wakeup target** —
    `replies.rs:1642,1573` — **DESIGN CALL**. The filter does reject
    100.119.56.3 (100.64/10), and the deployed receiver is real
    (`workspace-merge-plan.md:30`) — but an exact-origin operator allowlist
    already exists (`AGENT_WORKSPACE_WEBHOOK_TRUSTED_ORIGINS`, unit test
    uses this exact address, deploy doc instructs setting it). Blanket-
    allowing CGNAT would reopen SSRF to the whole tailnet/ISP CGNAT space;
    the current tradeoff is correct. Residual risk is operator migration
    only. No patch — do **not** whitelist 100.64/10.
45. **PR #156 — Clear drafts whenever the authenticated session ends** —
    `apps/web/src/auth/session.ts:179,193` — **DESIGN CALL**. The
    explicit/failed-logout sub-claim is already fixed (logout clears
    *before* awaiting, commit `72983c8`). Drafts do persist at rest after
    expiry/revocation (`setLocalDraftOwner(null)` deletes nothing), but
    owner-scoping prevents any other user reading them, a different sign-in
    wipes them, and same-person retention across a lapse is deliberate and
    tested (it's the offline outbox). The one-line fix
    (`clearLocalDrafts()` in `clearAuthenticatedQueryCache`) is draftable
    the moment Mischa makes the hygiene-vs-outbox call; not drafted as an
    uncontroversial fix.
46. **PR #157 — Keep reset tokens off workspace-controlled SMTP** —
    `mail/mod.rs:314-317`, `workspace.rs:1841` — **CONFIRMED (narrowed
    residual)** — see runners-up above. The non-default fall-through is
    fixed (`role='owner'`, commit `9fd5a3e`); the `$2` default exemption plus
    default-first ordering still selects a merely-joined workspace whenever
    it is the computed default, and `default_workspace()` is
    "chosen-or-earliest-joined" with the role discarded — contradicting the
    function's own doc comment. **Draft patch:**
    `patch-pr157-mail-owner-only.patch` (needs a Postgres integration test
    when landed: owner/member × default/non-default × mail on/off).
47. **PR #162 — Account for preview images before storing them** —
    `link_previews.rs:305` — **FALSE POSITIVE**. `fetch_job` calls
    `files::require_room` before `store.put` (commit `bfad0fb`); no room →
    text kept, picture dropped; `require_room` itself counts preview bytes
    (`files.rs:496` UNION ALL over `link_previews.image_bytes`).
48. **PR #186 — Stub the new pinned-Node check in the CI area test
    fixture** — `tools/ci/test_areas.py:37` — **FALSE POSITIVE**.
    `test_pinned_node.sh` is stubbed (and asserted at `:94`); the fix
    (commit `5864ca8`, citing the failing CI run) landed the same day
    PR #186 merged.

---

## Draft patches in this folder

Exported from a local worktree branch (`draft/aw-adjudication-fixes`, never
pushed; worktree removed after export). Each patch applies cleanly to main
`9227e49` (`git apply --check`), is rustfmt-clean under the repo's edition
2024, and the full set was compile-verified together:
`cargo check -p agent-workspace-server` finished with zero errors
(2026-11-10). Caveats: these are runtime `sqlx` queries, so SQL correctness
is not compiler-checked — the mail patch still needs its Postgres
integration test, and the briefing/SQL patches should get their integration
tests run before landing; no test suite was run in the sandbox.

| Patch | Item | Change |
|---|---|---|
| `patch-pr104-briefing-archived-org.patch` | #9 | Archived-projects query gains the enabled org-membership join |
| `patch-pr104-project-key-ambiguity.patch` | #7 | `project_reference` returns None on >1 match (mirrors `project_by_key`) |
| `patch-pr104-task-key-ambiguity.patch` | #8 | `task_by_key` refuses two distinct tasks (same task twice is fine) |
| `patch-pr36-retry-publication.patch` | #36 | Retry reuse UPDATE clears `published_commit`/`published_at`/`published_by_actor_id`/`publish_failure_category` |
| `patch-pr76-ignored-files.patch` | #42 | Removal paths refuse worktrees holding ignored files |
| `patch-pr128-dns-timeout.patch` | #43 | `lookup_host` wrapped in `WEBHOOK_TIMEOUT` in `webhook_client` |
| `patch-pr157-mail-owner-only.patch` | #46 | `transport_for_actor` requires `role='owner'` for every candidate |
| `patch-pr19-pinned-contract-doc.patch` | #34 | Compat doc pins the `thread/read` turns[] contract recovery depends on |

Deliberately **not** drafted: PR #172 (confirmed but changes parallel-session
semantics — Mischa's call), PR #35 / PR #17 items (confirmed but multi-path,
need their own scoped PRs + concurrency tests), PR #6 approvals item
(confirmed, spans parser + privacy test), cluster-D items 1 and 11 (need a
signature/helper change, specified in audit-plan.md), and all DESIGN CALL
items (no patch without the policy decision).
