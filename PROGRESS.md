# Implementation progress

Owner authorization: up to eight hours on September 21, 2026. Root is integration owner. Runtime choices are in ARCHITECTURE.md. No remote publication or other project edits.

- [x] Inspect planning documents and available environment.
- [x] Verify Node 24, pnpm, dedicated native PG availability, Codex ChatGPT login, GitHub read auth, browser capability and working local Git.
- [x] Record architecture and trust boundaries.
- [x] A: startup, schema, authentication, durable messages, real isolated Codex exchange.
- [x] A: groups, routing, burst budget, Resources, interruption, privacy tests.
- [ ] B: backend implemented and tested; frontend integration in progress.
- [ ] C: real bounded worktree execution and cancellation passed; GitHub adapter and recovery implemented; final review remains.
- [ ] Integration: browser, restart/replay, backup/restore, final build and handoff.

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
