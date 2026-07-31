# M0 walking skeleton

## Durable slice

Migration `0002_walking_skeleton.sql` creates the minimum M0 records and
constraints for Actors, Organizations, Projects, memberships, Conversations,
Messages, ordered conversation entries, execution workers, provider
connections and profiles, provider sessions, repositories, logical
workspaces, physical Git worktrees, Agent Runs, significant events, and
approvals.

Application-generated UUIDv7 values identify all new records. A database
trigger enforces that every Thread has one parent Channel in the same Project.
Conversation entry sequences are allocated transactionally. A partial unique
index allows at most one non-terminal Agent Run per Thread.

Development authentication resolves the fixed seeded human Actor server-side
and is rejected unless the process is in development mode and bound to
loopback. Browser requests never supply Message authorship.

## Run and recovery behavior

Starting a run records the exact base commit, creates a unique
`codex/run-<project>-<UUIDv7>` branch and isolated worktree, and persists the
logical workspace before execution. Git receives argument arrays through a
cleared, non-interactive environment.

The development-only executor is gated by
`AGENT_WORKSPACE_DEMO_EXECUTOR=true`. It creates a real isolated worktree,
persists one approval, and after approval appends a known proof line to
`KNOWN_FILE.txt`. It persists a single consolidated worker Message plus
significant file and lifecycle events; it never stores one row per token. This
executor is deterministic test scaffolding and is not accepted as the required
real Codex App Server run.

At startup, non-terminal runs enter `recovering`. M0 inspects their worktrees,
marks a provably dirty worktree as dirty, and preserves every worktree. Until
the retained Codex profile is authenticated and provider resume is proven, the
old run terminates as `interrupted` with an actionable recovery event instead
of guessing, resetting, or deleting filesystem state.

## Reader surface

The four-region React route loads the seeded Thread, renders durable Messages
and significant events, answers approvals or questions, interrupts an active
run, and reconnects to the WebSocket before refetching PostgreSQL-backed state.
Transient streamed Message deltas are coalesced in memory and replaced by the
durable consolidated Message. React text rendering prevents provider output
from becoming executable markup.

## Verification

- Rust unit tests cover configuration, redaction, state transitions, Git input
  sanitation, loopback WebSocket origins, and missing-worktree recovery.
- A real-PostgreSQL integration test covers migrations, idempotent seed,
  Actor foreign keys, sequence allocation, and the one-active-run constraint.
- Vitest covers timeline rendering, hostile output escaping, approval
  response/refetch, WebSocket refetch, and delta coalescing.
- The deterministic browser acceptance and restart run are performed on the
  Red Hat host before the real Codex acceptance run.
