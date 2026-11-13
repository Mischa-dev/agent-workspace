# Codex findings — context bundle (agent-workspace)

Reference material only. This branch exists so the draft patches and prior analysis
from the Nov 9–10, 2026 Codex review sweep are reachable by link. Nothing here is
proposed for merge as-is; the patches are drafts that a reviewer may use, rework,
or discard.

## Contents

- `prior-analysis-agent-workspace-verdicts.md` — the full prior analysis of all 48
  P1 findings (each traced against `main` at `9227e49`, grouped by family), with
  evidence for each conclusion.
- `audit-plan.md` — notes from that analysis on the shared-helper layer and the
  read-path patterns the findings cluster around.
- `patches/` — 8 draft patches. Each applies cleanly to `main` at `9227e49`
  (`git apply --check`), is rustfmt-clean, and the full set was compile-verified
  together (`cargo check -p agent-workspace-server`, zero errors, Nov 10, 2026).
  Caveats recorded in the verdicts doc: the queries are runtime `sqlx`, so SQL
  correctness is not compiler-checked; the mail patch still needs its Postgres
  integration test, and the briefing/SQL patches should get their integration
  tests run before landing; no test suite was run in the sandbox.

## Patch files

| Patch | Item | Change |
|---|---|---|
| patch-pr104-briefing-archived-org.patch | sweep #9 | Archived-projects query gains the enabled org-membership join |
| patch-pr104-project-key-ambiguity.patch | sweep #7 | `project_reference` returns None on >1 match (mirrors `project_by_key`) |
| patch-pr104-task-key-ambiguity.patch | sweep #8 | `task_by_key` refuses two distinct tasks (same task twice is fine) |
| patch-pr36-retry-publication.patch | sweep #36 | Retry reuse UPDATE clears `published_commit`/`published_at`/`published_by_actor_id`/`publish_failure_category` |
| patch-pr76-ignored-files.patch | sweep #42 | Removal paths refuse worktrees holding ignored files |
| patch-pr128-dns-timeout.patch | sweep #43 | `lookup_host` wrapped in `WEBHOOK_TIMEOUT` in `webhook_client` |
| patch-pr157-mail-owner-only.patch | sweep #46 | `transport_for_actor` requires `role='owner'` for every candidate |
| patch-pr19-pinned-contract-doc.patch | sweep #34 | Compat doc pins the `thread/read` turns[] contract recovery depends on |

Deliberately not drafted during the prior analysis: PR #172 (OAuth refresh-token
grace replay — traced and documented in the verdicts file; changes
parallel-session semantics), PR #35 / PR #17 items (multi-path), the PR #6
approvals item (spans parser + privacy test), two permissions-family items that
need a signature/helper change (specified in `audit-plan.md`), and all items the
analysis marked as design questions.
