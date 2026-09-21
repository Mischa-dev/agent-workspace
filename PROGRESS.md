# Implementation progress

Owner authorization: up to eight hours on September 21, 2026. Root is integration owner. Runtime choices are in ARCHITECTURE.md. No remote publication or other project edits.

- [x] Inspect planning documents and available environment.
- [x] Verify Node 24, pnpm, dedicated native PG availability, Codex ChatGPT login, GitHub read auth, browser capability and working local Git.
- [x] Record architecture and trust boundaries.
- [x] A: startup, schema, authentication, durable messages, real isolated Codex exchange.
- [x] A: groups, routing, burst budget, Resources, interruption, privacy tests.
- [x] B core: project planning, reviewed scope, worker results and GitHub read links connected. Remote publishing remains pending.
- [x] C core: two live worktrees, shared-lock tests, exact ancestry checks, interruption and recovery. AgentOS integration remains pending.
- [x] Integration: browser, restart/replay, backup/restore, final build and handoff.

## Preflight

Node 24.18.1 installed; pnpm 11.19.0; PG 18.6 native; Codex 0.148.0 authenticated through ChatGPT. Git works with task-local DEVELOPER_DIR=/Library/Developer/CommandLineTools. Browser control is available. GitHub read authentication is available; no remote is authorized for writes. No interaction required at preflight.

## Checkpoint: foundation and live runtime

- 25 protocol/domain/HTTP integration tests passed against a separate test PostgreSQL database.
- Real Codex tool catalog inspected; only checked workspace tools inside an isolated V8 wrapper. Ambient shell, file access, memory, skills, apps and delegation excluded.
- Live database-backed DM created and read a Resource. Two identities completed an independent group round on separate provider threads without private DM access.
- Live coding task created the expected file in a disposable worktree; original checkout stayed untouched.
- Live Stop received an observed interrupted terminal event. A real worker restart held a synthetic uncertain receipt without redispatch.
- Worker runs use finite time/turn/tool budgets; failure evidence remains distinct from acceptance.
- GitHub publishing disabled; shell/test-script execution is intentionally unavailable in the qualified tool-only runtime. Builtin patch/JSON validation is implemented.
- Frontend worker owns apps/web while root integrates backend and tests. No other project repositories edited.

Evidence: evidence/app-live.json, evidence/recovery-live.json, evidence/codex-qualification.md.

## Final integration checkpoint

- Browser created a space, project, real AI planning conversation, reviewed/applied two dependent research tasks, approved a 5-minute/one-worker scope, reviewed the first real output, and observed the dependent run consume its saved Resource.
- Browser created and revised a Resource, inspected old revisions, preserved a message draft after refresh, uploaded a text file, and verified/linked an existing real GitHub PR. No remote writes occurred.
- Real backup/restore retained every record count and attachment checksum in a new database/directory, preserved original/main configuration, cleared browser/runtime credentials, held a synthetic interrupted run, cancelled an old pending request, and restarted with dispatch disabled without replay.
- Added exact preview revision guards, post-claim stop/revocation checks, task tool idempotency, prerequisite result pins, output Resources, safe local checkpointing, shared/exclusive lock tests, SSE restored-cursor reconciliation, and bounded older-message retrieval.
- 45 integration/protocol tests passed before final connector startup hardening.
- A repeated live group test hit initialize timeouts before model dispatch. Earlier real DM/group/worktree qualification remains retained in `evidence/app-live-initial.json`; failed repetition is preserved separately. Connector startup serialization and a bounded setup deadline are being verified before final handoff.
- Read-only capacity probe returned unknown. Some in-run telemetry arrived with one missing window, correctly treated as unknown. Never infer reset capacity; finite bounds still apply.

The full planning roadmap remains open for AgentOS/Hermes, remote publishing, automatic invitations, model speaker selection, generated briefs/decision records, separate thread/channel views, GitHub event synchronization, arbitrary project tests, and large-dataset qualification. These are not claimed as completed by the usable local core.

## Verified local release

- [x] Discussion, Resources, projects and tasks connected in the same built interface.
- [x] Exact scope approval, bounded execution, local checkpoints, shared locks and prerequisite outputs.
- [x] Real GitHub reads and links; publication remains disabled.
- [x] Startup failure workaround verified by a fresh successful live DM/group/code run and two overlapping coding worktrees.
- [x] Real backup/restore and no-replay verification.
- [x] Actual browser interaction, conflict recovery, pinned versions, draft refresh and responsive layout checks.
- [x] 52 automated tests, TypeScript checks and production build.

Qualification details and remaining roadmap work are in ACCEPTANCE.md. Final startup uses the original clean owner database; browser/demo data remains isolated. There are no unattended demo agents.
