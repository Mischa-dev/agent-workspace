# Finding ledger

Base `dc695b5707ef6f74111f92b2e79ef2c884af1b75`. IDs follow the uploaded agent-workspace Part 1 order. Muse Part 2 numbering differs. The JSON ledger preserves every original claim.

| Verdict | Count |
|---|---:|
| already fixed | 111 |
| fixed | 21 |
| invalid | 3 |
| duplicate | 1 |
| design decision | 12 |
| blocked | 43 |

Every blocked entry remains open work. Its evidence explains what was inspected and what is missing. A source trace is not proof that every historical exploit regression ran.

## AW-001 · P1 · PR #6 · Reject failed or interrupted provider turns

Verdict: **already fixed**.

Sweep location: `apps/server/src/provider.rs:433`. Lines are the sweep anchor, not current offsets.

provider/codex_app_server.rs parse_provider_event compares status to completed; provider.rs drive_turn refuses !completed. Interrupted and failed notifications cannot complete runs.

## AW-002 · P1 · PR #6 · Preserve approval details before asking the user

Verdict: **fixed**.

Sweep location: `apps/server/src/provider.rs:498`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-003 · P1 · PR #6 · Preserve notifications queued before the turn loop

Verdict: **already fixed**.

Sweep location: `apps/server/src/provider.rs:348`. Lines are the sweep anchor, not current offsets.

tools/codex-bridge-spike/src/lib.rs next_raw_event pops notifications before reading stdout, preserving queued order.

## AW-004 · P1 · PR #6 · Cancel pending approvals during startup recovery

Verdict: **already fixed**.

Sweep location: `apps/server/src/recovery.rs:142`. Lines are the sweep anchor, not current offsets.

recovery.rs selects recovering alongside starting/working/blocked, excludes unstarted work, cancels approvals transactionally on interruption; main.rs also calls cancel_terminal_run_approvals.

## AW-005 · P1 · PR #6 · Reconcile runs already marked recovering

Verdict: **already fixed**.

Sweep location: `apps/server/src/recovery.rs:32`. Lines are the sweep anchor, not current offsets.

recovery.rs selects recovering alongside starting/working/blocked, excludes unstarted work, cancels approvals transactionally on interruption; main.rs also calls cancel_terminal_run_approvals.

## AW-006 · P1 · PR #6 · Route pre-response approvals through the application

Verdict: **invalid**.

Sweep location: `apps/server/src/provider.rs:112`. Lines are the sweep anchor, not current offsets.

Production codex_app_server.rs bridge configuration disables automatic_server_request_responses. Pre-response approval requests fail closed, rather than being automatically approved. Routing them gracefully remains a separate functional limitation.

## AW-007 · P1 · PR #17 · Reject retries while the provider turn is unresolved

Verdict: **fixed**.

Sweep location: `apps/server/src/scheduler.rs:275`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-008 · P1 · PR #19 · Record the new pinned provider response contract

Verdict: **fixed**.

Sweep location: `apps/server/src/provider/codex_app_server.rs:598`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-009 · P1 · PR #35 · Extend the lease beyond the credentialed network operation

Verdict: **fixed**.

Sweep location: `apps/server/src/github.rs:1055`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-010 · P1 · PR #36 · Require publication by the selected Agent Run

Verdict: **fixed**.

Sweep location: `apps/server/src/github.rs:1287`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-011 · P1 · PR #41 · Restrict merge-SHA fallback to merged close deliveries

Verdict: **already fixed**.

Sweep location: `apps/server/src/github.rs:3183`. Lines are the sweep anchor, not current offsets.

github.rs bounded_job_payload retains merge_commit_sha only for closed+merged pull_request events; reconcile_merge_commit_identity requires live REST merged state. Existing tests cover synchronize/review events.

## AW-012 · P1 · PR #68 · Pass --dbname to pg_restore

Verdict: **already fixed**.

Sweep location: `tools/host-maintenance/src/lib.rs:898`. Lines are the sweep anchor, not current offsets.

tools/host-maintenance/src/lib.rs restore supplies --dbname= with PGDATABASE. The fake pg_restore regression verifies the environment and argument order.

## AW-013 · P1 · PR #68 · Sanitize audit FKs when omitting credential rows

Verdict: **already fixed**.

Sweep location: `tools/host-maintenance/src/lib.rs:606`. Lines are the sweep anchor, not current offsets.

tools/host-maintenance/src/lib.rs sanitizer nulls session_id, mcp_credential_id and mcp_oauth_grant_id before restoring post-data; regression asserts the complete SQL.

## AW-014 · P1 · PR #71 · Clear all excluded audit foreign keys before post-data

Verdict: **duplicate**.

Sweep location: `tools/host-maintenance/src/lib.rs:905`. Lines are the sweep anchor, not current offsets.

Same audit-FK defect as AW-013; the sanitizer already nulls all three excluded foreign keys.

## AW-015 · P1 · PR #76 · Skip symlinks during storage admission

Verdict: **already fixed**.

Sweep location: `apps/server/src/storage_admission.rs:43`. Lines are the sweep anchor, not current offsets.

storage_admission.rs skips descendant symlinks and has symlinks_are_skipped_without_following_targets. Root-symlink bypass is separately tracked as AW-069.

## AW-016 · P1 · PR #76 · Preserve ignored files before removing worktrees

Verdict: **fixed**.

Sweep location: `apps/server/src/git_runtime.rs:575`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-017 · P1 · PR #90 · Preserve the OAuth grant's approved scopes

Verdict: **design decision**.

Sweep location: `apps/server/src/mcp/oauth.rs:980`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs agent_grant_scopes strips mutating scopes from read-only consent, but full consent deliberately uses the agent connection permissions. Scope response wording and exact full-scope semantics need a product decision; no silent change.

## AW-018 · P1 · PR #90 · Keep webhook signing keys out of database records

Verdict: **already fixed**.

Sweep location: `apps/server/migrations/0038_reply_requests.sql:69`. Lines are the sweep anchor, not current offsets.

external_agents.rs writes encrypted webhook_secret_id and NULL plaintext on create/rotate; secrets startup migration handles legacy plaintext when keyring exists. Old deployment without a master key remains an operator migration prerequisite.

## AW-019 · P1 · PR #90 · Reject private webhook destinations

Verdict: **already fixed**.

Sweep location: `apps/server/src/external_agents.rs:506`. Lines are the sweep anchor, not current offsets.

replies.rs validates HTTPS public destinations, validates all resolved addresses and pins checked DNS; exact trusted origins are operator-configured. No project-manager exception authorizes arbitrary private addresses.

## AW-020 · P1 · PR #90 · Keep webhook signing secrets out of browser responses

Verdict: **design decision**.

Sweep location: `apps/server/src/external_agents.rs:547`. Lines are the sweep anchor, not current offsets.

external_agents.rs returns a newly generated signing key once to its authorized manager; read views omit it. One-time issuance versus the broad no-browser-secrets wording needs an explicit policy decision.

## AW-021 · P1 · PR #90 · Recheck visibility when loading private reply metadata

Verdict: **fixed**.

Sweep location: `apps/server/src/conversations.rs:1675`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-022 · P1 · PR #96 · Keep human project and organization roles aligned

Verdict: **already fixed**.

Sweep location: `apps/server/src/memberships.rs:360`. Lines are the sweep anchor, not current offsets.

memberships.rs calls sync_organization_role in the same transaction; explicit/custom workspace roles are preserved intentionally.

## AW-023 · P1 · PR #96 · Authorize approvals against the selected project

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:1628`. Lines are the sweep anchor, not current offsets.

mcp.rs PRIMARY_SCOPES resolves approvalId to its target project. mcp/approvals.rs loads that project, current organization membership and agent_connection_can_access. NULL-run approvals remain a separate functional gap, not this cross-project claim.

## AW-024 · P1 · PR #96 · Authorize sponsored runs in the resolved project

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:1632`. Lines are the sweep anchor, not current offsets.

external_execution.rs uses the resolved conversation project and agent_connection_can_access, while credentials remain anchored to the home project.

## AW-025 · P1 · PR #99 · Parse resource URLs before resolving their project

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:1736`. Lines are the sweep anchor, not current offsets.

mcp/common.rs parse_id accepts UUIDs and canonical trailing-UUID links; middleware uses it. Some handler-level parsers remain UUID-only, a separate functional cleanup.

## AW-026 · P1 · PR #99 · Require current organization membership to manage agent grants

Verdict: **already fixed**.

Sweep location: `apps/server/src/connected_apps.rs:153`. Lines are the sweep anchor, not current offsets.

connected_apps.rs owned_grant joins enabled organization_memberships for the manager, so leftover project membership is insufficient.

## AW-027 · P1 · PR #99 · Enforce read-only access for agent grants

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/oauth.rs:467`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs agent_grant_scopes keeps only read/events for read-only consent and has an explicit read-only-consent regression.

## AW-028 · P1 · PR #100 · Wait for the claimed WorkOS session check

Verdict: **design decision**.

Sweep location: `apps/server/src/auth/workos.rs:542`. Lines are the sweep anchor, not current offsets.

auth/workos.rs conditional UPDATE claims one provider check; concurrent requests deliberately retain the session until the check resolves. Fail-closed waiting changes latency and availability policy.

## AW-029 · P1 · PR #104 · Reject ambiguous project keys before archiving

Verdict: **fixed**.

Sweep location: `apps/server/src/mcp/admin.rs:539`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-030 · P1 · PR #104 · Reject ambiguous task keys before rewriting IDs

Verdict: **fixed**.

Sweep location: `apps/server/src/mcp.rs:2001`. Lines are the sweep anchor, not current offsets.

Reviewed and revised PR #211. task_by_key now groups by task ID before LIMIT 2, and resolve_among distinguishes missing, unique and ambiguous lookup results. It keeps the archived-original fallback for an unreadable moved target, but cannot use it after an ambiguous shared lookup. historical_aliases_cannot_hide_a_conflicting_task_key covers a current key and two historical alias rows across workspaces.

## AW-031 · P1 · PR #104 · Check current org membership for archived projects

Verdict: **fixed**.

Sweep location: `apps/server/src/mcp/briefing.rs:353`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-032 · P1 · PR #104 · Recheck access to the actual move target

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/task_move.rs:673`. Lines are the sweep anchor, not current offsets.

mcp/task_move.rs already_moved filters the actual destination with actor_can_access_project and projects_showing_tasks; it does not trust the supplied destination alone.

## AW-033 · P1 · PR #105 · Recheck conversation visibility inside the write transaction

Verdict: **fixed**.

Sweep location: `apps/server/src/mcp/tasks.rs:2623`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-034 · P1 · PR #107 · Route thread lookups through conversation visibility

Verdict: **invalid**.

Sweep location: `apps/server/src/message_edits.rs:804`. Lines are the sweep anchor, not current offsets.

message_edits.rs removal paths enter through lock_message_for_change and the locked origin conversation. Child lookups are structurally limited to threads of that origin; lack of a repeated literal predicate does not itself establish the claimed leak.

## AW-035 · P1 · PR #109 · Apply PR visibility to browser work-item reads

Verdict: **already fixed**.

Sweep location: `apps/server/migrations/0050_pull_request_visibility.sql:25`. Lines are the sweep anchor, not current offsets.

work_items.rs WORK_ITEM_SELECT linked-PR aggregation applies pull_request_visible_to and work_item_view suppresses code details when code permission is absent.

## AW-036 · P1 · PR #128 · Bound DNS resolution before blocking the reply worker

Verdict: **fixed**.

Sweep location: `apps/server/src/replies.rs:1064`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-037 · P1 · PR #128 · Preserve the deployed private wakeup target

Verdict: **design decision**.

Sweep location: `apps/server/src/replies.rs:1093`. Lines are the sweep anchor, not current offsets.

replies.rs already has exact-origin trusted webhook configuration, including tests for the tailnet address. Blanket CGNAT acceptance would change SSRF protection; deployment must configure its specific trusted origin.

## AW-038 · P1 · PR #156 · Clear drafts whenever the authenticated session ends

Verdict: **design decision**.

Sweep location: `apps/web/src/auth/session.ts:173`. Lines are the sweep anchor, not current offsets.

auth/session.ts logout clears local drafts before awaiting the request. Session expiry detaches the draft owner; owner-scoped storage retains the same person offline outbox. Erasing that outbox on expiry is a product policy change.

## AW-039 · P1 · PR #157 · Keep reset tokens off workspace-controlled SMTP

Verdict: **fixed**.

Sweep location: `apps/server/src/mail/mod.rs:314`. Lines are the sweep anchor, not current offsets.

Locally incorporated reviewed PR #211 commits 7cd01f8 and 9de9350, with authorship retained. See PR-211-review.md for the per-item code trace, tests and limits. Full local verification is recorded separately; this is not a claim that main or the remote PR was changed.

## AW-040 · P1 · PR #162 · Account for preview images before storing them

Verdict: **already fixed**.

Sweep location: `apps/server/src/link_previews.rs:274`. Lines are the sweep anchor, not current offsets.

link_previews.rs fetch_job calls files::require_room before storing images; files accounting includes link_previews.image_bytes.

## AW-041 · P1 · PR #172 · Avoid granting a permanent branch during refresh grace

Verdict: **design decision**.

Sweep location: `apps/server/src/mcp/oauth.rs:867`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs refresh grace still issues a live replacement refresh token. Shortening grace-issued expiry or access-only responses can overwrite a shared client token store with an unusable answer. Explicitly deferred by assignment; preserve pending Mischa decision.

## AW-042 · P1 · PR #181 · Keep bearer tokens out of traced URL paths

Verdict: **already fixed**.

Sweep location: `apps/server/src/bug_reports.rs:70`. Lines are the sweep anchor, not current offsets.

lib.rs logged_path uses MatchedPath, so bug-report bearer paths log /api/v1/bug-reports/{token}; raw secret is not the traced path.

## AW-043 · P1 · PR #186 · Stub the new pinned-Node check in the CI area test fixture

Verdict: **already fixed**.

Sweep location: `tools/ci/checks.sh:275`. Lines are the sweep anchor, not current offsets.

tools/ci/test_areas.py stubs test_pinned_node.sh and asserts it was dispatched. Current checks.sh validates areas before execution.

## AW-044 · P1 · PR #194 · Enforce task-view permissions when reading task attachments

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:50`. Lines are the sweep anchor, not current offsets.

files.rs READABLE task branch requires tasks.view in the target project, with hidden_by_agent_limit applied afterward. roles_and_access::task_files_follow_who_can_see_tasks covers the path.

## AW-045 · P1 · PR #194 · Apply agent limits when authorizing task-comment moderation

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items/comments.rs:781`. Lines are the sweep anchor, not current offsets.

work_items/comments.rs is_comment_moderator checks agent_limit_hides ChannelsModerate before custom-role moderation. Regression an_app_moderates_only_within_its_agent_limit exists.

## AW-046 · P1 · PR #194 · Avoid promoting custom project invitees to workspace Admin

Verdict: **already fixed**.

Sweep location: `apps/server/src/invitations.rs:664`. Lines are the sweep anchor, not current offsets.

invitations.rs separates custom role_id from built-in Admin, constrains issuance by require_grantable and accept-time issuer_still_grants. roles_and_access has custom-admin invitation regressions.

## AW-047 · P1 · PR #194 · Bound OAuth agent credential issuance by grantable permissions

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/oauth.rs:1324`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs managed_agent bounds the agent grant by both manager standing and project_holding. roles_and_access::connecting_an_app_as_an_agent_stays_within_the_project_role covers narrowed project roles.

## AW-048 · P1 · PR #194 · Check project management permission before propagating roles

Verdict: **already fixed**.

Sweep location: `apps/server/src/roles.rs:1705`. Lines are the sweep anchor, not current offsets.

roles.rs apply_to_their_projects authorizes MembershipManage through manage_refusal per target and checks grant subset of project_holding; inaccessible projects are skipped.

## AW-049 · P2 · PR #6 · Enforce one active run per execution profile

Verdict: **design decision**.

Sweep location: `apps/server/src/api.rs:431`. Lines are the sweep anchor, not current offsets.

scheduler.rs now admits against provider_account max_concurrent_runs and one active turn per conversation; pooled adapters own account concurrency. Restoring the old one-process-per-profile restriction would conflict with the current provider-account design.

## AW-050 · P2 · PR #6 · Persist provider-start failures in the visible timeline

Verdict: **already fixed**.

Sweep location: `apps/server/src/api.rs:628`. Lines are the sweep anchor, not current offsets.

scheduler.rs fail_run inserts run_failed plus a conversation entry, retains worktree and publishes an update. Provider-start failures use this path.

## AW-051 · P2 · PR #6 · Handle queued runs before entering recovery

Verdict: **already fixed**.

Sweep location: `apps/server/src/recovery.rs:41`. Lines are the sweep anchor, not current offsets.

recovery.rs first clears leases for created/queued work, then selects only started states and recovering. Unstarted work returns to scheduler admission.

## AW-052 · P2 · PR #6 · Recover from an unresumable provider session

Verdict: **blocked**.

Sweep location: `apps/server/src/provider.rs:70`. Lines are the sweep anchor, not current offsets.

provider.rs still propagates resume_thread failure while retaining the provider_sessions mapping. A safe reset/replacement flow needs a lost-provider-history regression and a user-visible continuity policy; no such change is included.

## AW-053 · P2 · PR #7 · Enforce the ten-character Project key limit

Verdict: **already fixed**.

Sweep location: `apps/server/migrations/0004_m1_identity_projects.sql:12`. Lines are the sweep anchor, not current offsets.

Migration 0003_m0_consistency.sql tightens projects_project_key_check to ^[A-Z][A-Z0-9]{1,9}$, which supersedes the initial migration quoted by the finding.

## AW-054 · P2 · PR #8 · Acquire parent and child Conversation locks in one order

Verdict: **already fixed**.

Sweep location: `apps/server/src/conversations.rs:2012`. Lines are the sweep anchor, not current offsets.

conversations.rs lock_conversation_scope obtains project, then parent, then child locks and rechecks visibility after waits; writes enter through lock_conversation_for/lock_message_conversation.

## AW-055 · P2 · PR #8 · Enforce or paginate the active Channel limit

Verdict: **design decision**.

Sweep location: `apps/server/src/conversations.rs:1596`. Lines are the sweep anchor, not current offsets.

conversations.rs lists at most MAX_CHANNELS_PER_PROJECT=100 while channel creation is not capped. Correcting the product behavior requires choosing a write cap or paginated channel navigation; no silent channel-admission restriction added.

## AW-056 · P2 · PR #8 · Paginate Threads before building the sidebar projection

Verdict: **blocked**.

Sweep location: `apps/server/src/conversations.rs:1667`. Lines are the sweep anchor, not current offsets.

conversations.rs sidebar query still applies a global MAX_THREADS_PER_SIDEBAR=1000 before grouping. Complete per-channel pagination and large-project regressions are not implemented in this branch.

## AW-057 · P2 · PR #18 · Expose failure acknowledgement outside the worker composer

Verdict: **already fixed**.

Sweep location: `apps/web/src/threads/ThreadPage.tsx:774`. Lines are the sweep anchor, not current offsets.

The failure acknowledgement is now a RunCardActions button in ThreadRunControls.tsx, outside RunComposer. The old showComposer nesting no longer controls it.

## AW-058 · P2 · PR #35 · Mint a separate token for each GitHub operation

Verdict: **design decision**.

Sweep location: `apps/server/src/github.rs:1248`. Lines are the sweep anchor, not current offsets.

github.rs publication still uses one repository-scoped installation token for ref inspection, push and confirmation, all parts of the same explicit publication operation. Per-HTTP-request credential minting changes rate/credential lifetime policy without demonstrating a new authority grant.

## AW-059 · P2 · PR #36 · Project an existing pull request onto retry runs

Verdict: **blocked**.

Sweep location: `apps/server/src/conversations.rs:2567`. Lines are the sweep anchor, not current offsets.

conversations.rs and run_views.rs still join github_pull_request_requests by agent_run_id, while retry reuse keeps a workspace identity. Showing an existing workspace PR on retries requires a shared projection change and retry/PR integration regression; absent here.

## AW-060 · P2 · PR #36 · Exclude archived Work Items from pull-request creation

Verdict: **blocked**.

Sweep location: `apps/web/src/threads/ThreadPage.tsx:607`. Lines are the sweep anchor, not current offsets.

useThreadRunActions.tsx still sends every linkedWorkItems ID to PR creation, including archived chips. A create-PR regression for mixed active/archived links is not yet included; no claim that PR #211 covers it.

## AW-061 · P2 · PR #41 · Serialize recovery before locking the job

Verdict: **already fixed**.

Sweep location: `apps/server/src/github.rs:4455`. Lines are the sweep anchor, not current offsets.

github.rs exact redelivery reads checksum without the old delivery FOR SHARE lock, then updates failed/exhausted jobs before updating delivery. The specific two-shared-lock upgrade deadlock is absent.

## AW-062 · P2 · PR #52 · Load the systemd environment without shell evaluation

Verdict: **blocked**.

Sweep location: `deploy/redhat/agent-workspace-service-command:20`. Lines are the sweep anchor, not current offsets.

deploy/redhat/agent-workspace-service-command still sources the root-owned EnvironmentFile as Bash. Exact systemd EnvironmentFile parsing and wrapper parity tests are needed; a partial shell parser would be unsafe.

## AW-063 · P2 · PR #68 · Relax parent permission checks for restore roots

Verdict: **blocked**.

Sweep location: `tools/host-maintenance/src/lib.rs:501`. Lines are the sweep anchor, not current offsets.

Host-maintenance restore-root validation still checks directory ownership/permissions along the path. Relaxing trusted root-owned ancestors needs clean-restore fixtures and a precise trust rule; not safely changed in this pass.

## AW-064 · P2 · PR #68 · Reject unexpected data archive members

Verdict: **blocked**.

Sweep location: `tools/host-maintenance/src/lib.rs:935`. Lines are the sweep anchor, not current offsets.

tools/host-maintenance/src/lib.rs safe_archive_entry rejects absolute/traversal paths but does not restrict data archive contents to the captured runtime categories. An allowlist needs restore-format compatibility and archive-member regressions, absent here.

## AW-065 · P2 · PR #70 · Meter the board work-item read path

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items.rs:922`. Lines are the sweep anchor, not current offsets.

work_items.rs list and board handlers both call check_expensive_read before executing their queries.

## AW-066 · P2 · PR #70 · Count malformed OAuth attempts before extraction

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/oauth.rs:499`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs token still uses Form<TokenRequest> before its in-handler rate limiter. Malformed bodies can avoid that specific ceremony budget. Middleware-level accounting needs careful separation from refresh sender/app quotas and malformed-body regressions.

## AW-067 · P2 · PR #76 · Exclude open pull-request worktrees from retention

Verdict: **design decision**.

Sweep location: `apps/server/src/retention.rs:160`. Lines are the sweep anchor, not current offsets.

retention.rs excludes active runs and provider sessions, but published retained workspaces remain eligible regardless of an open remote PR. Whether PR review requires a local worktree after publication is a retention policy decision; commits remain published remotely.

## AW-068 · P2 · PR #76 · Tolerate vanished files during admission scans

Verdict: **fixed**.

Sweep location: `apps/server/src/storage_admission.rs:38`. Lines are the sweep anchor, not current offsets.

storage_admission.rs now skips NotFound only for descendants, including directories removed after stat; missing roots and other errors still fail. Root symlinks are refused. Regressions use real directory deletion at metadata lookup and a symlink to a populated root.

## AW-069 · P2 · PR #78 · Reject symlinked data roots before skipping entries

Verdict: **fixed**.

Sweep location: `apps/server/src/storage_admission.rs:45`. Lines are the sweep anchor, not current offsets.

storage_admission.rs now skips NotFound only for descendants, including directories removed after stat; missing roots and other errors still fail. Root symlinks are refused. Regressions use real directory deletion at metadata lookup and a symlink to a populated root.

## AW-070 · P2 · PR #81 · Persist external connection IDs on MCP write audits

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:61`. Lines are the sweep anchor, not current offsets.

mcp/writes.rs audit metadata now carries external_agent_connection_id from the principal.

## AW-071 · P2 · PR #81 · Preserve scopes when downgrading to viewer

Verdict: **blocked**.

Sweep location: `apps/web/src/external-agents/ExternalAgentSettings.tsx:329`. Lines are the sweep anchor, not current offsets.

ExternalAgentSettings.tsx existing-connection changeRole still uses viewerScopes defaults for viewer downgrade. Preserving the intersection needs UI role-toggle tests and deciding whether role changes reset preset scopes; not changed.

## AW-072 · P2 · PR #82 · Deliver events for replies without fresh mentions

Verdict: **already fixed**.

Sweep location: `apps/server/src/conversations.rs:1377`. Lines are the sweep anchor, not current offsets.

replies.rs ask_for_replies now asks thread_followers on human thread messages even when mentioned_actor_ids is empty, so fresh mentions are not required for follow-ups.

## AW-073 · P2 · PR #82 · Purge expired external-agent events

Verdict: **already fixed**.

Sweep location: `apps/server/migrations/0034_m9_external_agent_events.sql:37`. Lines are the sweep anchor, not current offsets.

retention.rs expire_agent_events deletes expired external_agent_events in bounded batches and schedules database retention work.

## AW-074 · P2 · PR #84 · Audit rejected human-required approval responses

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/approvals.rs:217`. Lines are the sweep anchor, not current offsets.

mcp/approvals.rs load_context has an explicit include_human_required_for_denial path and respond inserts insert_human_required_denial_audit before refusing.

## AW-075 · P2 · PR #84 · Make production approvals routable before enqueueing them

Verdict: **design decision**.

Sweep location: `apps/server/src/provider.rs:608`. Lines are the sweep anchor, not current offsets.

provider.rs provider_approval_policy explicitly keeps execution approvals human-required while questions may target any authorized approver. Unit test provider_approval_policy_keeps_execution_approvals_human_required pins this safety policy.

## AW-076 · P2 · PR #84 · Hash the description before consuming it

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/writes.rs:270`. Lines are the sweep anchor, not current offsets.

mcp/writes.rs normalizes then restores arguments.description before hashing the complete arguments; the old take-without-restore hash defect is absent.

## AW-077 · P2 · PR #84 · Notify every addressed approver of resolution

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/approvals.rs:627`. Lines are the sweep anchor, not current offsets.

mcp/approvals.rs calls enqueue_external_approval_resolved(transaction, approval_id), which targets the approval rather than only the responding connection.

## AW-078 · P2 · PR #88 · Clear modal state when Details stops being a sheet

Verdict: **already fixed**.

Sweep location: `apps/web/src/shell/AppShell.tsx:52`. Lines are the sweep anchor, not current offsets.

AppShell.tsx listens to the mobile-width media query and closes drawers when it stops matching. contextOpen is also conditional on actual context content.

## AW-079 · P2 · PR #90 · Reject replies after their claim lease expires

Verdict: **fixed**.

Sweep location: `apps/server/src/replies.rs:263`. Lines are the sweep anchor, not current offsets.

replies.rs final completion/interim renewal and mcp/replies.rs progress require lease_expires_at > clock_timestamp(). New Postgres regression expires a still-claimed row and checks progress and both reply modes are refused without posting; a current lease still works.

## AW-080 · P2 · PR #90 · Avoid colliding with an existing DMSPACE project key

Verdict: **blocked**.

Sweep location: `apps/server/src/direct.rs:607`. Lines are the sweep anchor, not current offsets.

direct.rs space_project still inserts reserved DMSPACE with ON CONFLICT DO NOTHING, then requires a direct-kind project. The global organization/key unique index can collide with an older standard DMSPACE project. Needs migration/compatibility handling and bootstrap collision regression.

## AW-081 · P2 · PR #90 · Bind reply identity into the idempotency reservation

Verdict: **fixed**.

Sweep location: `apps/server/src/conversations.rs:1030`. Lines are the sweep anchor, not current offsets.

conversations.rs binds request ID, generation and final/interim mode into the message request hash. New Postgres regression verifies exact interim replay and refusal to reuse its key for final completion. Older pre-upgrade reply keys may conflict safely once rather than replay under unverifiable context.

## AW-082 · P2 · PR #90 · Stop wakeups after disabling an agent connection

Verdict: **already fixed**.

Sweep location: `apps/server/src/replies.rs:584`. Lines are the sweep anchor, not current offsets.

replies.rs delivery still_true checks agent_connection_can_access for every queued delivery; the SQL helper rejects disabled/revoked connections.

## AW-083 · P2 · PR #90 · Resolve reply requests when removing a group member

Verdict: **already fixed**.

Sweep location: `apps/server/src/direct.rs:515`. Lines are the sweep anchor, not current offsets.

direct.rs change_group_members calls replies::withdraw_requests for removed members, covering pending/claimed/expired/released requests in the group and its threads.

## AW-084 · P2 · PR #91 · Apply imported theme state atomically

Verdict: **already fixed**.

Sweep location: `apps/web/src/theme/AppearancePage.tsx:219`. Lines are the sweep anchor, not current offsets.

ThemeProvider.tsx uses functional setPreferences updates, so setTheme then setOverrides in one event both apply. The closure-over-old-preferences claim no longer describes the provider.

## AW-085 · P2 · PR #91 · Synchronize the mobile context drawer with its callers

Verdict: **already fixed**.

Sweep location: `apps/web/src/shell/AppShell.tsx:245`. Lines are the sweep anchor, not current offsets.

AppShell.tsx now accepts contextKey and uses it to open the context drawer when callers request details. Callers can dismiss through onContextDismiss.

## AW-086 · P2 · PR #91 · Place the unread divider before the first unread entry

Verdict: **already fixed**.

Sweep location: `apps/web/src/conversations/Timeline.tsx:280`. Lines are the sweep anchor, not current offsets.

Timeline.tsx emits the first unread divider without requiring a previous entry. It deliberately skips own messages.

## AW-087 · P2 · PR #92 · Give each new-item trigger its own focus target

Verdict: **blocked**.

Sweep location: `apps/web/src/work-items/WorkItemsPage.tsx:200`. Lines are the sweep anchor, not current offsets.

WorkItemsPage.tsx renders the same newButton/ref in header and emptyAction. A focus restoration regression is needed before changing the dialog trigger contract.

## AW-088 · P2 · PR #93 · Authorize membership events for the correct recipients

Verdict: **design decision**.

Sweep location: `apps/server/src/realtime.rs:188`. Lines are the sweep anchor, not current offsets.

realtime.rs authorizes project.membership.updated via RealtimeSubscribe for current project members; payload is bounded membership invalidation metadata. Changing its recipients requires deciding whether the affected actor must receive loss-of-membership invalidation after access is gone.

## AW-089 · P2 · PR #96 · Resolve pull-request identifiers from their own table

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:1652`. Lines are the sweep anchor, not current offsets.

mcp.rs identifier_project branches on pull_request_get and resolves github_pull_requests with pull_request_visible_to, rather than work_items.

## AW-090 · P2 · PR #96 · Serialize expired WorkOS session checks

Verdict: **already fixed**.

Sweep location: `apps/server/src/auth/workos.rs:487`. Lines are the sweep anchor, not current offsets.

auth/workos.rs uses a conditional workos_checked_at UPDATE to claim one five-minute session check; its deliberate concurrent fail-open behavior is AW-028.

## AW-091 · P2 · PR #97 · Exclude agents that cannot answer mentions

Verdict: **blocked**.

Sweep location: `apps/server/src/conversations.rs:2019`. Lines are the sweep anchor, not current offsets.

replies.rs agents_to_ask still selects active external agents by membership and visibility without requiring an answering-capable connection. Mention-picker parity and delivery tests are needed before changing discoverability.

## AW-092 · P2 · PR #98 · Advertise the scopes required by WorkOS registration

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/oauth.rs:80`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs protected-resource metadata advertises workspace scopes when WorkOS is enabled, and parse_scopes normalizes workspace:read/workspace:write to local read/write. Unit regression accepts both spellings; the local server supports the advertised resource scopes.

## AW-093 · P2 · PR #99 · Reject mark_read on read-only grants

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/talk.rs:1171`. Lines are the sweep anchor, not current offsets.

mcp/talk.rs mark_read begins by refusing !principal.can_write; optional read-time marks also guard can_write.

## AW-094 · P2 · PR #99 · Propagate failures when posting a thread's opening message

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/talk.rs:1726`. Lines are the sweep anchor, not current offsets.

mcp/talk.rs post_optional still maps every non-Ok send result into None after thread creation. Propagating errors safely requires rollback/idempotent-origin behavior tests; not implemented.

## AW-095 · P2 · PR #100 · Keep resource and authorization-server scopes compatible

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/oauth.rs:83`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs protected-resource metadata advertises workspace scopes when WorkOS is enabled, and parse_scopes normalizes workspace:read/workspace:write to local read/write. Unit regression accepts both spellings; the local server supports the advertised resource scopes.

## AW-096 · P2 · PR #101 · Precache lazy route chunks for the first offline launch

Verdict: **blocked**.

Sweep location: `apps/web/public/sw.js:77`. Lines are the sweep anchor, not current offsets.

public/sw.js cacheShell still extracts asset URLs only from index.html and does not traverse lazy route chunks. An offline-first-install browser regression and explicit precache/budget policy are needed.

## AW-097 · P2 · PR #101 · Restore focus after closing a persisted navigation drawer

Verdict: **blocked**.

Sweep location: `apps/web/src/shell/AppShell.tsx:110`. Lines are the sweep anchor, not current offsets.

AppShell.tsx keeps navTriggerRef inside each remounting shell. Cross-route persistent drawer focus restoration needs a shared trigger/fallback regression, absent here.

## AW-098 · P2 · PR #104 · Keep key-based task-move retries idempotent

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp.rs:1907`. Lines are the sweep anchor, not current offsets.

mcp.rs rewrites task keys through aliases before task_move, while already_moved operates on the archived source UUID. Exact key-based retry semantics remain to be reproduced with the new task-move work in open PR #208; avoided overlapping its implementation.

## AW-099 · P2 · PR #104 · Detect descendants beyond the recursion cap

Verdict: **invalid**.

Sweep location: `apps/server/src/mcp/task_move.rs:204`. Lines are the sweep anchor, not current offsets.

mcp/task_move.rs binds MAX_SUBTASK_LEVELS-1 and the task hierarchy enforces that same maximum. A hypothetical deeper hierarchy outside those invariants is not evidence of a reachable truncation defect; no cap increase or broad traversal change.

## AW-100 · P2 · PR #104 · Support the admin role when adding a new member

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/admin.rs:645`. Lines are the sweep anchor, not current offsets.

mcp/admin.rs accepts built-in admin then calls memberships::add_member for a new member; add_member explicitly permits member/viewer only. Needs owner-only admin admission with organization-role synchronization tests, absent here.

## AW-101 · P2 · PR #105 · Enforce the per-task label cap when restoring labels

Verdict: **blocked**.

Sweep location: `apps/server/src/work_item_labels.rs:883`. Lines are the sweep anchor, not current offsets.

work_item_labels.rs restoration path needs a task-by-task active-label-cap regression. MAX_LABELS_PER_TASK on normal assignment alone does not establish correctness on label restoration; unverified beyond static path review.

## AW-102 · P2 · PR #106 · Reject invalid areas before dispatching checks

Verdict: **already fixed**.

Sweep location: `tools/ci/checks.sh:13`. Lines are the sweep anchor, not current offsets.

tools/ci/checks.sh validates rust,web/rust/web before dispatch and run.sh has matching input validation. tools/ci/test_areas.py checks invalid values perform no work and fail.

## AW-103 · P2 · PR #107 · Rewind to the newest message rather than newest entry

Verdict: **blocked**.

Sweep location: `apps/server/src/conversations.rs:2499`. Lines are the sweep anchor, not current offsets.

conversations.rs mark_unread_from(UnreadFrom::Newest) still uses next_entry_sequence-1, which can be a worker entry. Selecting the newest actual message needs timeline read-mark regression; not changed.

## AW-104 · P2 · PR #107 · Reopen superseded requests for the fallback message

Verdict: **blocked**.

Sweep location: `apps/server/src/replies.rs:653`. Lines are the sweep anchor, not current offsets.

replies.rs request superseding/revival needs the exact edit/removal fallback scenario regression before changing generations and lifecycle rules. No valid patch or independently sufficient dynamic evidence is available for this candidate yet.

## AW-105 · P2 · PR #107 · Schedule another mark-seen attempt after failures

Verdict: **blocked**.

Sweep location: `apps/web/src/conversations/useAutoMarkSeen.ts:67`. Lines are the sweep anchor, not current offsets.

useAutoMarkSeen.ts resets shown.current on failure but has no state change/timer, so stable inputs do not schedule another effect. Needs a retry/backoff and user-mark-unread race regression before changing it.

## AW-106 · P2 · PR #109 · Include execution availability in repository readiness

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/code.rs:365`. Lines are the sweep anchor, not current offsets.

Repository readiness is currently a repository/Git result; provider execution availability is handled separately at admission. The quoted combined-readiness expectation needs a concrete API/UI contract and unavailable-profile regression.

## AW-107 · P2 · PR #110 · Use Messages routes for direct-conversation runs

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/runs.rs:276`. Lines are the sweep anchor, not current offsets.

mcp/runs.rs run_value still emits /projects/{project}/threads/{conversation}, without a direct-parent route discriminator. Needs a DM/group run routing regression and shared canonical-link helper.

## AW-108 · P2 · PR #110 · Do not mark approvals answerable for read-only apps

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/approvals.rs:786`. Lines are the sweep anchor, not current offsets.

mcp/approvals.rs projection sets can_respond from pending/blocked state without carrying principal write eligibility. Respond itself is protected by may_respond. Needs read-only-app projection regression to correct the affordance without weakening access.

## AW-109 · P2 · PR #111 · Reject cycles beyond the traversal cap

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/planning.rs:139`. Lines are the sweep anchor, not current offsets.

work_item_planning.rs blocking_path uses recursive reachable edges plus a visited map rather than a fixed traversal-depth cutoff. This defeats the claimed long-cycle bypass.

## AW-110 · P2 · PR #111 · Reject archived endpoints before adding dependencies

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/planning.rs:94`. Lines are the sweep anchor, not current offsets.

work_item_planning.rs refuses archived dependency endpoints for additions; mcp/planning.rs locks referenced tasks and refuses archived ones before assigning milestones.

## AW-111 · P2 · PR #111 · Refuse adding archived tasks to milestones

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/planning.rs:575`. Lines are the sweep anchor, not current offsets.

work_item_planning.rs refuses archived dependency endpoints for additions; mcp/planning.rs locks referenced tasks and refuses archived ones before assigning milestones.

## AW-112 · P2 · PR #112 · Revalidate thread access before GitHub idempotency replays

Verdict: **blocked**.

Sweep location: `apps/server/src/github.rs:1441`. Lines are the sweep anchor, not current offsets.

GitHub publish/PR idempotency order needs access-revoked replay fixtures on the exact handler. No verified replay-safe patch is included; avoid claiming the separate run-projection visibility checks cover cached mutation responses.

## AW-113 · P2 · PR #120 · Preserve the membership lock order

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/admin.rs:606`. Lines are the sweep anchor, not current offsets.

mcp/admin.rs admin_row obtains project/membership locks before the membership helpers acquire their advisory lock. The shared lock-order claim needs a concurrent MCP/browser membership regression before a safe reordering.

## AW-114 · P2 · PR #120 · Complete cancellation through the run transition path

Verdict: **blocked**.

Sweep location: `apps/server/migrations/0053_run_prompt_redaction.sql:208`. Lines are the sweep anchor, not current offsets.

The prompt-redaction migration directly cancels queued runs in SQL. Comparing every terminal side effect against transition_run requires a dedicated migration-backfill regression; no historical migration edit or untested repair migration included.

## AW-115 · P2 · PR #122 · Avoid reusing a human who owns the `agent` handle

Verdict: **blocked**.

Sweep location: `apps/server/src/execution_setup.rs:78`. Lines are the sweep anchor, not current offsets.

execution_setup.rs ensure_default_worker still selects the existing agent handle without checking actor kind. A human-handle collision needs a reserved-worker identity strategy and setup regression, absent here.

## AW-116 · P2 · PR #123 · Preserve sidebar data across the route remount

Verdict: **blocked**.

Sweep location: `apps/web/src/conversations/hooks.ts:58`. Lines are the sweep anchor, not current offsets.

conversations/hooks.ts placeholderData can retain only its own prior observer data; a route remount with a cold overview key may still be empty. A remount/cached-sidebar regression is required before changing cache lifetime.

## AW-117 · P2 · PR #128 · Preserve all validated DNS addresses

Verdict: **blocked**.

Sweep location: `apps/server/src/replies.rs:1083`. Lines are the sweep anchor, not current offsets.

replies.rs webhook_client validates all resolved addresses but pins only addresses[0]. Multi-address fallback needs a deterministic resolver/client fixture proving it preserves the SSRF checks; not changed.

## AW-118 · P2 · PR #128 · Store the normalized public origin

Verdict: **fixed**.

Sweep location: `apps/server/src/config.rs:552`. Lines are the sweep anchor, not current offsets.

config.rs public_origin now stores parsed.origin().ascii_serialization after validation. New regression covers scheme-normalized spellings, hostname case and default HTTPS port removal.

## AW-119 · P2 · PR #156 · Preserve mention metadata when editing queued sends

Verdict: **already fixed**.

Sweep location: `apps/web/src/conversations/StructuredComposer.tsx:162`. Lines are the sweep anchor, not current offsets.

StructuredComposer.tsx restoreQueued merges stored mentions/workItems and restores attachments and reply target. Editing queued text no longer discards mention metadata.

## AW-120 · P2 · PR #157 · Coalesce jobs for collapsed notification rows

Verdict: **blocked**.

Sweep location: `apps/server/src/notification_email.rs:99`. Lines are the sweep anchor, not current offsets.

notification_email.rs enqueue uses recipient+dedupe_key for jobs, while collapsed rows share notification_id. Coalescing safely needs delayed collapse/update regressions and preserving already-delivered occurrences; not implemented.

## AW-121 · P2 · PR #157 · Make the status-file override visible to the server

Verdict: **blocked**.

Sweep location: `deploy/redhat/agent-workspace-daily-backup:30`. Lines are the sweep anchor, not current offsets.

The backup wrapper accepts BACKUP_STATUS_FILE but server_status.rs reads fixed data_root/backup-status.json. Aligning the override needs environment/config and wrapper tests; not silently removing an operator option.

## AW-122 · P2 · PR #158 · Permit untimed notes for `/later`

Verdict: **fixed**.

Sweep location: `apps/web/src/later/useConversationTools.tsx:90`. Lines are the sweep anchor, not current offsets.

later.rs now allows a nonempty bare note with no remindAt, as saved_items already permits. Added integration coverage showing it reads back and queues no reminder notification.

## AW-123 · P2 · PR #158 · Generate a fresh idempotency key for each `/task`

Verdict: **already fixed**.

Sweep location: `apps/web/src/later/useConversationTools.tsx:53`. Lines are the sweep anchor, not current offsets.

useConversationTools.tsx stores a per-title idempotency key and clears taskKey.current after successful /task, allowing repeated titles to create new tasks.

## AW-124 · P2 · PR #158 · Distinguish direct-space threads from project threads

Verdict: **blocked**.

Sweep location: `apps/web/src/later/useConversationTools.tsx:50`. Lines are the sweep anchor, not current offsets.

useConversationTools.tsx treats every thread kind as a project conversation. Direct-thread callers need parent-kind context and slash-command/module regressions; not changed to infer it from an opaque project UUID.

## AW-125 · P2 · PR #158 · Wire tools into the thread-panel composer

Verdict: **already fixed**.

Sweep location: `apps/web/src/threads/ThreadPanel.tsx:80`. Lines are the sweep anchor, not current offsets.

ThreadPanel.tsx passes tools.composer into StructuredComposer, including its command and scheduling callbacks.

## AW-126 · P2 · PR #158 · Search backward for the latest actual message

Verdict: **fixed**.

Sweep location: `apps/web/src/conversations/ChannelPage.tsx:211`. Lines are the sweep anchor, not current offsets.

ChannelPage, DirectConversationPage, ThreadPage and ThreadPanel now search backward for the latest message, so trailing worker/system events do not erase /later context. Existing page and full web checks cover the affected callers.

## AW-127 · P2 · PR #158 · Hash the forwarded source in idempotency payloads

Verdict: **already fixed**.

Sweep location: `apps/server/src/conversations.rs:1606`. Lines are the sweep anchor, not current offsets.

conversations.rs append_message_command_with hashes extras.forwarded_message_id and extras.reply_to_message_id, so different source messages cannot replay the same request.

## AW-128 · P2 · PR #159 · Fetch all conversations before declaring success

Verdict: **blocked**.

Sweep location: `apps/web/src/shell/markAllRead.ts:55`. Lines are the sweep anchor, not current offsets.

markAllRead still operates on capped project/direct/sidebar source lists. Complete pagination requires server list contracts and a large-account regression; this branch only fixes accurate reporting of expanded-channel failures as AW-129.

## AW-129 · P2 · PR #159 · Report failed expanded-thread requests

Verdict: **fixed**.

Sweep location: `apps/web/src/shell/markAllRead.ts:50`. Lines are the sweep anchor, not current offsets.

markAllRead returns expanded-thread failure counts while preserving successful unread results. A Vitest regression failed on the old behavior and passes with the fix.

## AW-130 · P2 · PR #159 · Release delayed pushes when notifications resume

Verdict: **blocked**.

Sweep location: `apps/server/src/notification_settings.rs:232`. Lines are the sweep anchor, not current offsets.

notification_settings.rs pause changes do not reschedule already delayed push jobs. Needs a resume-before-old-deadline regression and dedupe-safe job rescheduling; not included.

## AW-131 · P2 · PR #161 · Save handoffs only for a claim the caller actually releases

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tasks.rs:2789`. Lines are the sweep anchor, not current offsets.

mcp/tasks.rs task_release checks handoff.is_some() && !released and refuses before inserting a handoff. Release retries cannot add notes for another actor claim.

## AW-132 · P2 · PR #161 · Model ping wakeups without a required request ID

Verdict: **blocked**.

Sweep location: `sdks/runner-typescript/src/index.ts:13`. Lines are the sweep anchor, not current offsets.

sdks/runner-typescript/src/index.ts Wakeup includes ping/inbox.available in the event union but still requires replyRequestId. A discriminated union would be useful, but needs SDK consumer/type tests and runtime validation parity; not silently broadened to optional everywhere.

## AW-133 · P2 · PR #162 · Open the created task when attachment upload fails

Verdict: **already fixed**.

Sweep location: `apps/web/src/quick-add/QuickAddForms.tsx:71`. Lines are the sweep anchor, not current offsets.

QuickAddForms.tsx catches attachment upload failure into stranded {key,href,reason}, preserving a direct link to the successfully created task instead of losing it.

## AW-134 · P2 · PR #164 · Keep the current settings project visible in the rail

Verdict: **blocked**.

Sweep location: `apps/web/src/settings/SettingsArea.tsx:200`. Lines are the sweep anchor, not current offsets.

SettingsArea project picker navigates to project settings without refreshing project lastOpenedAt, so off-rail deep links may stay hidden. Needs navigation-state regression and deciding whether opening settings counts as project use.

## AW-135 · P2 · PR #164 · Normalize the returned location for legacy clients

Verdict: **blocked**.

Sweep location: `apps/server/src/projects.rs:1077`. Lines are the sweep anchor, not current offsets.

Legacy settings navigation location normalization needs old-client projection fixtures. Current switch flow refreshes lastOpenedAt but retains stored navigation metadata; no compatibility rewrite included.

## AW-136 · P2 · PR #165 · Recompute the focus trap after the lazy sidebar mounts

Verdict: **blocked**.

Sweep location: `apps/web/src/shell/AppShell.tsx:280`. Lines are the sweep anchor, not current offsets.

AppShell focus effect captures drawer elements before a lazy sidebar mounts. Keyboard handling enumerates controls fresh but from those captured elements. Needs a delayed-sidebar keyboard regression and observer/ref ownership fix.

## AW-137 · P2 · PR #166 · Preserve reset tokens after ambiguous SMTP failures

Verdict: **blocked**.

Sweep location: `apps/server/src/password_reset.rs:642`. Lines are the sweep anchor, not current offsets.

password_reset.rs deletes the token for every mail::send error, including ambiguous post-DATA SMTP failures. Keeping a token requires a transport-level ambiguous-acceptance regression and retry policy; no speculative credential-flow change included.

## AW-138 · P2 · PR #167 · Derive the agent catalog from its selected environment

Verdict: **blocked**.

Sweep location: `apps/web/src/agents/AgentProfileDialog.tsx:101`. Lines are the sweep anchor, not current offsets.

AgentProfileDialog runsOn uses draft.harness || project default and omits the selected environment harness. Needs a profile-with-environment/catalog regression before model defaults are normalized differently.

## AW-139 · P2 · PR #173 · Add the new route to the offline navigation allowlist

Verdict: **already fixed**.

Sweep location: `apps/web/src/app/App.tsx:340`. Lines are the sweep anchor, not current offsets.

public/sw.js APP_ROUTE includes design-system, so the renamed route is accepted for offline app navigation.

## AW-140 · P2 · PR #179 · Avoid logging unauthenticated callback probes

Verdict: **already fixed**.

Sweep location: `apps/server/src/auth/workos.rs:273`. Lines are the sweep anchor, not current offsets.

auth/workos.rs worth_logging excludes unmatched callback stages; failure_location also receives linking, distinguishing account-link conflicts. Dedicated unit regressions cover unmatched probes and link failures.

## AW-141 · P2 · PR #179 · Distinguish link conflicts from unlinked accounts

Verdict: **already fixed**.

Sweep location: `apps/server/src/auth/workos.rs:278`. Lines are the sweep anchor, not current offsets.

auth/workos.rs worth_logging excludes unmatched callback stages; failure_location also receives linking, distinguishing account-link conflicts. Dedicated unit regressions cover unmatched probes and link failures.

## AW-142 · P2 · PR #181 · Clear link state when the project changes

Verdict: **already fixed**.

Sweep location: `apps/web/src/settings/project/BugReportsPage.tsx:68`. Lines are the sweep anchor, not current offsets.

SettingsArea.tsx keys PageBody by project.id, so switching projects remounts BugReportsPage and clears setup/replacing/deleting state. The finding did not account for this caller guard.

## AW-143 · P2 · PR #182 · Start handshake timing before authentication

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:2952`. Lines are the sweep anchor, not current offsets.

mcp.rs authentication middleware starts timing before bearer lookup/body read. HandshakeStep watches response bodies to distinguish JSON-RPC errors inside HTTP 200. handshake_logs_show_protocol_errors_and_the_whole_wait covers both.

## AW-144 · P2 · PR #182 · Detect protocol errors before logging handshake success

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:3022`. Lines are the sweep anchor, not current offsets.

mcp.rs authentication middleware starts timing before bearer lookup/body read. HandshakeStep watches response bodies to distinguish JSON-RPC errors inside HTTP 200. handshake_logs_show_protocol_errors_and_the_whole_wait covers both.

## AW-145 · P2 · PR #194 · Let task-edit permission open task attachment uploads

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:48`. Lines are the sweep anchor, not current offsets.

files upload authorization routes task attachments through TasksEdit; roles_and_access::task_files_follow_who_can_see_tasks exercises narrowed task-only roles.

## AW-146 · P2 · PR #194 · Filter task attachments from search by task-view permission

Verdict: **already fixed**.

Sweep location: `apps/server/src/search.rs:132`. Lines are the sweep anchor, not current offsets.

search.rs computes task_projects whenever Tasks OR Files is searched and binds that list separately for task attachment hits. file_search_leaves_out_task_files_where_tasks_are_hidden covers it.

## AW-147 · P2 · PR #194 · Filter saved task previews by current task-view permission

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:59`. Lines are the sweep anchor, not current offsets.

later.rs applies roles::role_allows_sql tasks.view plus agent-limit project filtering before saved task projections.

## AW-148 · P2 · PR #194 · Apply code-view restrictions to project overview repositories

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:26`. Lines are the sweep anchor, not current offsets.

mcp/briefing.rs project_open computes sees_code via projects_allowing(CodeView) before including repositories; reads_naming_tasks_or_repositories_follow_the_role_and_limit covers it.

## AW-149 · P2 · PR #194 · Filter linked code details from task reads by code-view permission

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/reads.rs:262`. Lines are the sweep anchor, not current offsets.

mcp/reads.rs passes authorize_read RepositoryView into work_item_value and omits linked code without it; a_tasks_linked_code_needs_see_code covers this.

## AW-150 · P2 · PR #194 · Filter linked tasks by their own project's task-view permission

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tasks.rs:1606`. Lines are the sweep anchor, not current offsets.

mcp/planning.rs visible_links combines target-project task_links visibility, reachable projects and agent_limit_hides TasksView; linked_tasks_in_another_project_follow_see_tasks_there covers it.

## AW-151 · P2 · PR #194 · Filter usage batches by effective task-view permission

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:30`. Lines are the sweep anchor, not current offsets.

usage/batch.rs checks projects_showing_tasks for each task-bearing batch entry before naming/ranking it, applying current role and app limit.

## AW-152 · P2 · PR #194 · Filter linked pull-request details from run reads

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:145`. Lines are the sweep anchor, not current offsets.

mcp/runs.rs suppresses branch, publication-request and PR fields when projects_allowing(CodeView) rejects the target project; redaction includes row.branch.

## AW-153 · P2 · PR #194 · Preserve app attribution for batched queued launches

Verdict: **already fixed**.

Sweep location: `apps/server/src/launch.rs:2285`. Lines are the sweep anchor, not current offsets.

launch.rs persists made_through_app for queued and mention launches; drain_queued and tag jobs re-read current app limits before execution.

## AW-154 · P2 · PR #194 · Preserve app attribution for launches triggered by mentions

Verdict: **already fixed**.

Sweep location: `apps/server/src/launch.rs:2717`. Lines are the sweep anchor, not current offsets.

launch.rs persists made_through_app for queued and mention launches; drain_queued and tag jobs re-read current app limits before execution.

## AW-155 · P2 · PR #194 · Check task-view access before returning archive results

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items.rs:1256`. Lines are the sweep anchor, not current offsets.

work_items.rs archive response uses capabilities TasksView to redact content. roles_and_access::archiving_without_see_tasks_reveals_no_task checks the returned view.

## AW-156 · P2 · PR #194 · Reapply app limits when queued task imports execute

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/views_data.rs:545`. Lines are the sweep anchor, not current offsets.

work_items/imports.rs persists made_through_app, resolves current app access and runs import chunks within_agent_limits. GitHub reads also check CodeView; role regressions cover the read path.

## AW-157 · P2 · PR #194 · Redact task details from launch cancellation responses too

Verdict: **already fixed**.

Sweep location: `apps/server/src/launch.rs:2568`. Lines are the sweep anchor, not current offsets.

launch.rs cancel returns through views_by_id, whose read projection removes work_item without sees_tasks. cancelling_a_launch_without_see_tasks_reveals_no_task covers it.

## AW-158 · P2 · PR #194 · Redact task references in comment mutation responses too

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items.rs:4063`. Lines are the sweep anchor, not current offsets.

work_items/comments.rs read projection calls sees_tasks_in and removes task-reference titles when hidden; comment_references_show_titles_only_with_see_tasks covers it.

## AW-159 · P2 · PR #194 · Filter suggestion decision responses by task-view permission

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:135`. Lines are the sweep anchor, not current offsets.

ai/suggestions.rs decision locks and checks ai_suggestion_visible_to plus WITHIN_APP_LIMIT before returning the result. ai_suggestions_need_see_tasks covers it.

## AW-160 · P2 · PR #194 · Redact task details from bulk mutation responses

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items/planning.rs:1240`. Lines are the sweep anchor, not current offsets.

work_items/planning.rs bulk result uses effective capabilities and strips tasks without TasksView; bulk_changes_without_see_tasks_reveal_no_task covers it.

## AW-161 · P2 · PR #194 · Check destination task visibility when replaying a move

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/task_move.rs:236`. Lines are the sweep anchor, not current offsets.

mcp/task_move.rs already_moved filters destination titles through projects_showing_tasks; a_repeated_move_shows_the_title_only_where_tasks_are_seen covers it.

## AW-162 · P2 · PR #194 · Filter linked task titles from whiteboard reads

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:107`. Lines are the sweep anchor, not current offsets.

whiteboards.rs resolves linked task projects through projects_showing_tasks before exposing their titles; board_links_show_task_titles_only_where_tasks_are_seen covers it.

## AW-163 · P2 · PR #194 · Enforce member-view permission on the workspace member list

Verdict: **already fixed**.

Sweep location: `apps/server/src/workspace.rs:560`. Lines are the sweep anchor, not current offsets.

workspace.rs list_members calls caller_may DirectoryView and otherwise returns only the caller; the_workspace_member_directory_follows_directory_view covers it.

## AW-164 · P2 · PR #194 · Require source task visibility before copying task contents

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/task_move.rs:195`. Lines are the sweep anchor, not current offsets.

mcp/task_move.rs requires source task visibility and CodeView for linked-code provenance. moving_a_task_needs_its_contents_and_code_to_be_seen covers both.

## AW-165 · P2 · PR #194 · Filter restricted code links from move provenance

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/task_move.rs:166`. Lines are the sweep anchor, not current offsets.

mcp/task_move.rs requires source task visibility and CodeView for linked-code provenance. moving_a_task_needs_its_contents_and_code_to_be_seen covers both.

## AW-166 · P2 · PR #194 · Redact restricted task details from run-step reads

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:146`. Lines are the sweep anchor, not current offsets.

mcp/run_pages.rs run_steps calls run_views::run_detail and run_diff; run_views::redact clears task_title when projects_showing_tasks excludes the target project.

## AW-167 · P2 · PR #194 · Redact repository branches from MCP run reads without code access

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/runs.rs:347`. Lines are the sweep anchor, not current offsets.

mcp/runs.rs suppresses branch, publication-request and PR fields when projects_allowing(CodeView) rejects the target project; redaction includes row.branch.

## AW-168 · P2 · PR #194 · Redact task references returned by planning mutations

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/tool_permissions.rs:139`. Lines are the sweep anchor, not current offsets.

mcp/planning.rs mutation projections use the visibility-aware task_links path and app limit per target. link_changes_name_the_task_only_where_tasks_are_seen and link_changes_list_linked_tasks_only_where_the_app_sees_them cover it.

## AW-169 · P2 · PR #194 · Redact branches and commits from conversation run projections

Verdict: **already fixed**.

Sweep location: `apps/server/src/conversations.rs:4353`. Lines are the sweep anchor, not current offsets.

conversations.rs hide_pull_request clears branch, base_commit and published_commit as well as PR fields when code visibility is absent.

## AW-170 · P2 · PR #194 · Check task visibility before rendering a launch prompt

Verdict: **already fixed**.

Sweep location: `apps/server/src/launch.rs:909`. Lines are the sweep anchor, not current offsets.

launch.rs checks projects_showing_tasks before using a work item to render a launch prompt.

## AW-171 · P2 · PR #194 · Bound agent credential grants by the caller's project permissions

Verdict: **already fixed**.

Sweep location: `apps/server/src/external_agents.rs:1169`. Lines are the sweep anchor, not current offsets.

external_agents.rs require_grantable intersects manager standing with project_holding; agents_are_given_only_what_the_giver_holds_in_the_project covers it.

## AW-172 · P2 · PR #194 · Bound membership grants by the caller's project permissions

Verdict: **already fixed**.

Sweep location: `apps/server/src/memberships.rs:432`. Lines are the sweep anchor, not current offsets.

memberships.rs require_grantable checks project_holding as well as workspace standing; project_roles_are_given_only_within_the_givers_project_role covers it.

## AW-173 · P2 · PR #194 · Recheck the project grant before accepting an invitation

Verdict: **already fixed**.

Sweep location: `apps/server/src/invitations.rs:1315`. Lines are the sweep anchor, not current offsets.

invitations.rs require_issuer_still_grants rechecks the issuer project grant before accept; a_project_invitation_lapses_when_its_issuers_project_role_narrows_below_it covers it.

## AW-174 · P2 · PR #194 · Check task visibility before creating a worker thread

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items.rs:3386`. Lines are the sweep anchor, not current offsets.

work_items.rs worker-thread creation checks TasksView; working_with_a_worker_needs_see_tasks covers it.

## AW-175 · P2 · PR #194 · Redact repository mutation responses without code-view access

Verdict: **already fixed**.

Sweep location: `apps/server/src/repositories.rs:682`. Lines are the sweep anchor, not current offsets.

Repository prepare authorization requires RepositoryView alongside management; managing_a_repository_needs_see_code covers it.

## AW-176 · P2 · PR #194 · Bound agent OAuth grants by the manager's project permissions

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/oauth.rs:1349`. Lines are the sweep anchor, not current offsets.

mcp/oauth.rs managed_agent bounds the agent grant by both manager standing and project_holding. roles_and_access::connecting_an_app_as_an_agent_stays_within_the_project_role covers narrowed project roles.

## AW-177 · P2 · PR #197 · Check the thing's own module set before returning it

Verdict: **already fixed**.

Sweep location: `apps/server/src/modules/http.rs:303`. Lines are the sweep anchor, not current offsets.

modules/http.rs and mcp/module_tools.rs re-fetch the resolved thing using state::for_project for its own project; initial anywhere discovery does not authorize the response.

## AW-178 · P2 · PR #197 · Apply role module restrictions to notification reads

Verdict: **already fixed**.

Sweep location: `apps/server/src/modules/state.rs:606`. Lines are the sweep anchor, not current offsets.

modules/state.rs notification predicate combines workspace switches with held project/workspace role modules_off, filtering unread and attention reads.

## AW-179 · P2 · PR #197 · Apply workspace module settings to projectless things

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp/module_tools.rs:215`. Lines are the sweep anchor, not current offsets.

modules/things.rs shown uses viewer.workspaces[thing.organization_id] for projectless boards, rather than the anywhere union.

## AW-180 · P2 · PR #197 · Keep direct-message thread tools discoverable when Channels is off

Verdict: **already fixed**.

Sweep location: `apps/server/src/mcp.rs:2813`. Lines are the sweep anchor, not current offsets.

modules/state.rs allows_tool explicitly keeps thread_start/thread_update when Messages is on, with unit tests for the DMs-only case.

## AW-181 · P2 · PR #197 · Apply each webhook owner's effective module restrictions

Verdict: **blocked**.

Sweep location: `apps/server/src/events.rs:218`. Lines are the sweep anchor, not current offsets.

events.rs route checks workspace module switches, but webhooks.rs fan_out/visible_to check owner access and permissions without owner-effective module settings. Automations do load state::for_project. Webhook owner-role module regressions and both queue/delivery fixes remain needed.

## AW-182 · P2 · PR #199 · Preserve the reply target when flushing queued messages

Verdict: **already fixed**.

Sweep location: `apps/web/src/conversations/StructuredComposer.tsx:595`. Lines are the sweep anchor, not current offsets.

StructuredComposer.tsx flush passes item.replyTo with the original idempotency key; restoreQueued also restores the target.

## AW-183 · P2 · PR #199 · Preserve reply semantics when scheduling a message

Verdict: **already fixed**.

Sweep location: `apps/web/src/conversations/StructuredComposer.tsx:574`. Lines are the sweep anchor, not current offsets.

StructuredComposer.tsx schedule explicitly rejects replyTo with a visible error, preserving the draft instead of silently dropping reply semantics. Full scheduled-reply support remains an intentional separate feature.

## AW-184 · P2 · PR #199 · Keep the direct thread open when jumping to an older reply target

Verdict: **already fixed**.

Sweep location: `apps/web/src/conversations/ReplyQuote.tsx:50`. Lines are the sweep anchor, not current offsets.

ReplyQuote.tsx uses RevealInThread when a target is not currently rendered, keeping direct thread panels open while older entries load.

## AW-185 · P2 · PR #199 · Refresh reply quotes when their source messages change

Verdict: **already fixed**.

Sweep location: `apps/web/src/conversations/Timeline.tsx:374`. Lines are the sweep anchor, not current offsets.

useLiveOlderEntries.ts tracks quoted-message revisions, blanks removed quotes immediately and refetches older reply quotes after edits, rather than relying only on newest-page reloads.

## AW-186 · P2 · PR #199 · Refresh reply quotes before replaying a stored send response

Verdict: **already fixed**.

Sweep location: `apps/server/src/conversations.rs:1953`. Lines are the sweep anchor, not current offsets.

conversations.rs idempotency replay calls forwarding::refresh_reply_quote before responding, so removed/edited source text is not replayed from the cached body.

## AW-187 · P2 · PR #203 · Preserve resolvable references in custom-field exports

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items/imports.rs:2038`. Lines are the sweep anchor, not current offsets.

work_items/imports.rs name_field_values rewrites page references with work_item_fields::exported_page, retaining resolvable links with labels.

## AW-188 · P2 · PR #203 · Replace saved-view clauses when overriding custom-field filters

Verdict: **blocked**.

Sweep location: `apps/server/src/mcp/tasks.rs:476`. Lines are the sweep anchor, not current offsets.

mcp/views_data.rs apply_view removes overridden built-in clauses, but does not remove same-field custom-field clauses before mcp/tasks.rs ANDs explicit fields. Needs a saved-view override regression and field-id normalization; not fixed.

## AW-189 · P2 · PR #203 · Preserve person references through CSV formula escaping

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items/imports.rs:2197`. Lines are the sweep anchor, not current offsets.

work_items/imports.rs field_cell doubles literal leading apostrophes; unguarded removes exactly a guard or doubled quote for custom fields. Person/Text/URL/Link/SingleChoice import paths use it, while choices use JSON arrays.

## AW-190 · P2 · PR #203 · Distinguish literal apostrophes from CSV formula guards

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items/imports.rs:2123`. Lines are the sweep anchor, not current offsets.

work_items/imports.rs field_cell doubles literal leading apostrophes; unguarded removes exactly a guard or doubled quote for custom fields. Person/Text/URL/Link/SingleChoice import paths use it, while choices use JSON arrays.

## AW-191 · P2 · PR #203 · Preserve text values through CSV formula escaping

Verdict: **already fixed**.

Sweep location: `apps/server/src/work_items/imports.rs:533`. Lines are the sweep anchor, not current offsets.

work_items/imports.rs field_cell doubles literal leading apostrophes; unguarded removes exactly a guard or doubled quote for custom fields. Person/Text/URL/Link/SingleChoice import paths use it, while choices use JSON arrays.
