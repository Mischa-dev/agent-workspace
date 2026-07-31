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

All subprocesses clear the inherited environment, disable system Git
configuration and interactive prompts, set a known PATH and locale, and cap
captured error output. Repository-controlled hooks are not executed by these
commands.

## Locking and safety

An advisory lock at `<mirror>/agent-workspace.lock` serializes only branch and
worktree metadata changes. It is released before a workspace is returned, so
no agent execution occurs while the repository lock is held.

Project slugs accept lowercase ASCII letters, digits, and single hyphens only.
Base refs reject option-like, revision-range, reflog, backslash, and whitespace
forms. Worktree targets must be immediate children of the canonicalized
repository worktree root.

Removal fails closed when porcelain status reports tracked or untracked
changes. No dirty worktree is reset or deleted automatically.

## Verification record

Deterministic temporary-repository tests prove mirror creation, exact base
capture, two concurrently requested linked worktrees, collision-safe unique
branches, scoped locking, unsafe input rejection, dirty preservation, clean
removal, and prune. The complete suite passed on macOS and on the Red Hat 10.1
ARM64 validation host with `git version 2.47.3` at commit `3117b81`.
