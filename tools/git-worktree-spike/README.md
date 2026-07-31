# Git worktree spike

The spike uses argument-array Git subprocesses, a repository-scoped advisory
lock, UUIDv7 branch suffixes, and strict path/ref validation. The lock covers
only mirror and worktree metadata operations; callers receive the completed
workspace after the lock is released and must never hold it during agent
execution.

Dirty worktrees are preserved. Removal is permitted only after a porcelain
status check returns empty, and cleanup finishes with `git worktree prune`.
