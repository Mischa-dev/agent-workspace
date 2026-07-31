# Git worktree compatibility

## Pinned host contract

- Git: `2.47.3` on the Red Hat 10.1 ARM64 validation host
- Process API: argument arrays only; no shell interpolation
- Canonical repository: `git clone --mirror <source> <mirror>`
- Branch form: `codex/run-<validated-project-slug>-<UUIDv7>`
- Worktree creation:
  `git --git-dir <mirror> worktree add -b <branch> <path> <base-commit>`
- Exact base:
  `git --git-dir <mirror> rev-parse --verify <ref>^{commit}`
- Inspection: `git -C <worktree> status --porcelain=v1 -z --untracked-files=all`
- Repair: `git --git-dir <mirror> worktree repair <path>`
- Cleanup:
  `git --git-dir <mirror> worktree remove <path>` followed by
  `git --git-dir <mirror> worktree prune --expire now`

All subprocesses clear the inherited environment, disable system and global
Git configuration, inherited credential helpers, SSH, interactive prompts,
and repository hooks, then set a known PATH and locale. Captured stdout is
bounded to 1 MiB and stderr to 8 KiB; logs retain byte counts and categories,
never Git output that could contain credentials or repository content.

## Locking and safety

The M0 spike used `<mirror>/agent-workspace.lock`. The M3 service uses an
owner-only filesystem lock at
`repositories/<repository-id>/metadata.lock`, allowing the same lock to cover
initial mirror preparation, fetch, branch/worktree mutation, removal, repair,
and prune. It is released before a workspace is returned, so no agent
execution occurs while the repository lock is held.

Project slugs accept lowercase ASCII letters, digits, and single hyphens only.
Base refs reject option-like, revision-range, reflog, backslash, and whitespace
forms. Worktree targets must be immediate children of the canonicalized
repository worktree root.

Removal fails closed when porcelain status reports tracked or untracked
changes. No dirty worktree is reset or deleted automatically.

## M3 production manager

New repositories receive an immutable GitHub numeric identity, exact
`https://github.com/<owner>/<name>.git` origin, validated default branch, and a
server-generated mirror at
`repositories/<repository-id>/mirror.git`. Registration remains `pending`
until an authorized preparation command verifies the mirror origin, performs a
pruned remote update, and resolves the exact default-branch commit. Failure is
persisted only as `git_prepare_failed`; raw Git diagnostics and paths never
enter browser DTOs.

New worktrees use
`worktrees/<repository-id>/<repository-workspace-id>`. The logical workspace
owns the immutable branch and base commit, while Git inspection verifies that
the physical tree belongs to the expected mirror and branch. Concurrent adds
serialize only their short metadata operations. Dirty removal fails without
changing the tree; clean removal is followed by a locked prune, and repair
does not reset content.

Migration `0011_m3_canonical_repository_mirrors.sql` adopts the retained M0
mirror without moving it. A startup preflight verifies its origin and default
branch, clones its refs into the UUID-scoped canonical location, and stages an
exact old/new path mapping. The migration switches new execution to the copy
and records the immutable `legacy_mirror_path`; the old mirror and all retained
clean or dirty worktrees remain in place. Its SHA-384 is
`26e2b8f8fcbc24786db29015b1b89a12b5eced0f35f5443e35267c03d940ed768da7a7529e9e66f48eb84136c66c3b75`.

## Verification record

Deterministic temporary-repository tests prove mirror creation, exact base
capture, two concurrently requested linked worktrees, collision-safe unique
branches, scoped locking, unsafe input rejection, dirty preservation, clean
removal, and prune. The complete suite passed on macOS and on the Red Hat 10.1
ARM64 validation host with `git version 2.47.3` at commit `3117b81`.

The redacted behavioral summary is committed at
`tools/git-worktree-spike/fixtures/live-evidence.json`. It contains command
shapes and outcomes only, never repository content or operator paths.

The M3 manager additionally proves idempotent canonical preparation, origin
mismatch rejection, two linked worktrees created concurrently under the
repository lock, exact mirror/branch ownership inspection, dirty preservation,
clean removal, and repair/prune. A real-PostgreSQL HTTP test covers private
session, Origin/CSRF, role policy, trust acknowledgement, pending registration,
idempotent replay, duplicate and unsafe identity rejection, authorized
activation, safe failure state, archive/replay, and path redaction. A separate
upgrade test proves that both canonical and legacy mirrors exist after `0011`.
