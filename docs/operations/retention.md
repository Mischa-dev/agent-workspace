# Private-beta retention and disk admission

Policy version 1 protects recoverable work before reclaiming space. Cleanup is
an owner-operated maintenance action, not an emergency deletion mechanism.

## Fixed database windows

| Category | Eligibility | Action |
| --- | --- | --- |
| Idempotency reservations | expired for at least 1 day | delete |
| Invitation capabilities | active and expired | revoke in place |
| OAuth authorization codes | used or expired, at least 1 day old | delete |
| OAuth access tokens | expired or revoked, at least 7 days old | delete |
| OAuth refresh tokens | used, expired, or revoked, at least 30 days old | unlink retained rotation references, then delete |
| Published outbox rows | published at least 30 days ago | delete |
| Completed temporary GitHub jobs | completed or exhausted at least 30 days ago | delete |

These windows never make durable product history eligible. Actor attribution,
audit history, Messages, Work Items, approvals, Agent Runs, provider sessions,
and unique generated content remain preserved.

Run `retention-cleanup` without `--apply` first. The JSON output contains only
policy category counts. Applying requires the literal `--apply true`, takes a
transaction-level PostgreSQL advisory lock, reuses the owner-only database-name
file, and is safe to retry. Concurrently consumed rows simply cease to match.
The operator retains the before/after JSON with the accepted release evidence;
no token prefix, identifier, path, repository name, or content is printed.

Retained-worktree cleanup is a separate server command because it must combine
the application database with the checked Git runtime:

    agent-workspace-server retention-worktrees
    agent-workspace-server retention-worktrees --apply true

The first form is a dry run. Candidates must be retained for seven days, have
no non-terminal Agent Run, have no new or active provider session, and have a
recorded published commit. The stored mirror and worktree paths must exactly
match the registered data-root derivation. Apply revalidates the database under
a repository-scoped advisory lock, takes the repository metadata lock, checks
the worktree is clean and on the expected branch, and requires `HEAD` to equal
the recorded published commit. Git performs one final dirty-tree refusal during
removal. Any mismatch or race preserves the worktree; successful retries mark
the worktree removed and its logical workspace cleaned before pruning metadata.
The aggregate `preservedWorktrees` count includes every registered live tree
that was not removed, including dirty, unpushed, active, ambiguous, and
too-young trees that never become candidates.

## Storage reporting and admission

Run `storage-report` with the exact registered data, release, backup, database,
and executable paths shown in the host-maintenance README. The report uses
allocated filesystem blocks, PostgreSQL's current database size, and the
filesystem's available blocks. It does not follow symlinks and reports only
aggregate categories.
Registered root path components must not be symlinks. Internal symlinks are
reported only as a count and are never traversed or resolved.

New disk-growing operations are admitted only while both conditions hold:

- allocated application data is below the configured data-root quota;
- available filesystem space is above the configured minimum-free watermark.

Crossing either threshold must not disable reads, interruption, recovery,
backup, or cleanup reporting. It also never authorizes deleting or resetting a
dirty worktree, an unpushed branch, a non-terminal run, an active provider
session, or an ambiguous Git relationship. Clean worktree cleanup requires a
separate repository-scoped lock plus fresh database and Git revalidation.

Attachment upload remains disabled in version one. A future upload path starts
with a 25 MiB per-object limit, a configurable 2 GiB per-Project aggregate,
MIME/content validation, and the same admission watermark.
