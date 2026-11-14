> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

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

At startup, non-terminal runs enter `recovering`; a run already left in
`recovering` by a second process interruption re-enters the same reconciliation
path. M0 inspects both their worktrees and every registered worktree, records
the observed dirty state, and preserves every worktree. It restarts the
retained profile's App Server, resumes the stored provider Thread, and reads
the latest provider turn. A provably completed turn may be reconciled to
completion; an interrupted, non-terminal, or ambiguous turn becomes an
application `interrupted` result with an actionable recovery event. Startup
also cancels stale pending approvals owned by terminal runs and returns an
`active` provider session to `idle` once it is linked only to terminal runs.
Recovery never guesses, resets, or deletes filesystem state.

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

## Red Hat live acceptance

On 2026-07-31 at 09:29Z, Chrome reached the Red Hat loopback service through
the approved SSH forward and visibly started hosted run
`019fb781-85fc-78b0-9805-08e0b6ba24fa`. Codex executed in an isolated
worktree and completed the bounded proof-file task. PostgreSQL retained these
ordered entries:

- sequence 25: command started
- sequence 26: command completed
- sequence 27: file change
- sequence 28: command started
- sequence 29: command completed
- sequence 30: consolidated worker Message
- sequence 31: run completed

The browser timeline rendered the submitted task, significant command and file
events, one consolidated worker Message, and terminal completion. This was the
real hosted Codex path, not the deterministic development executor. The
acceptance record deliberately excludes the prompt text, Message contents,
absolute worktree paths, and credentials.

A separate recovery run, `019fb783-f135-7be2-a2ad-64f3cccb50aa`, reached
`working` and persisted a command-started event at sequence 33 before the
systemd service was restarted. The service returned ready without database or
filesystem repair. Startup resumed the stored provider Thread and persisted
the sequence-34 recovery event with:

```text
provider_resumed=true
latest_turn=interrupted
result=interrupted
worktree_preserved=true
worktree_state=ready
dirty=false
```

After reload, Chrome visibly rendered the recovery result and the API reported
no active run. This proves the bounded M0 restart behavior: deterministic
provider reconciliation, terminal interruption when continuation is not
provable, and preservation of a clean isolated worktree.
