# Local release acceptance

Verified on this Mac on September 21, 2026. The application is a usable private local core; the complete planning roadmap has additional milestones. No other project was edited, no remote branch or PR was published, and no paid infrastructure or billing change was made.

## Real agents and services

| Check                       | Result and evidence                                                                                                                                                                                                                                                                                      |
| --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Codex qualification         | Installed CLI 0.148.0, supported existing ChatGPT authentication, fresh isolated runtime sessions, emitted tool catalog and V8 host-access probe. `evidence/codex-qualification.md`, `codex-live.json`, `codex-tool-catalog.json`.                                                                       |
| DM and Resources            | A real agent created a PostgreSQL Resource, read it through the checked tool, and returned the stored content. `evidence/app-live.json`.                                                                                                                                                                 |
| Group privacy               | Two identities used different runtime threads for independent opinions; the private DM canary was excluded, peer answers stayed staged until completion, and the round published. `evidence/app-live.json`.                                                                                              |
| Planning and dependent work | Browser created Workshop / Reading garden, requested a real two-task proposal, applied it, approved a five-minute scope with one worker, reviewed the first saved Resource, accepted it, and observed the second task consume that output and save its own Resource. `evidence/browser-acceptance.json`. |
| Local code execution        | Real agent wrote the exact requested file in a disposable worktree, ran bounded checks, produced a local checkpoint and result Resource, and left the target checkout unchanged. `evidence/app-live.json`.                                                                                               |
| Two coding workers          | Two real task runs overlapped, wrote different JSON files in separate worktrees, and produced separate commits. Target checkout unchanged. `evidence/app-live.json` records times, paths, and commits.                                                                                                   |
| Stop                        | Real turn received an interruption request and observed terminal interruption. `evidence/recovery-live.json`.                                                                                                                                                                                            |
| GitHub read                 | Browser verified and linked an existing real PR, openai/codex #46959, to the test project. Read-only integration, no publication.                                                                                                                                                                        |
| PostgreSQL and restore      | Real dedicated PostgreSQL, Drizzle migrations, pg-boss worker, custom dump and restored bytes. New DB/directory preserved all record counts and one uploaded-file checksum. Original/main config preserved. `evidence/backup-restore.json`.                                                              |

A repeated concurrent group test exposed CLI initialize timeouts before model dispatch. The connector now serializes startup, permits model turns to overlap, and uses separate bounded queue/setup deadlines. The next full live run passed DM, group, one coding task, and two overlapping coding tasks. The failed attempt is preserved as `evidence/app-live-startup-failure.json`; earlier qualification is in `evidence/app-live-initial.json`. This is observed recovery from the reproduced failure, not a promise that an external runtime can never fail.

## Automated integration and protocol tests

**52 tests passed**, with TypeScript checks and the production build passing. `./workspace test` uses separate disposable PostgreSQL databases; it does not change the owner workspace.

Database, HTTP, storage, and Git operations in domain tests are real. The runtime responses in these tests are explicit fixtures. Tests cover:

- Command deduplication and conflicting command IDs; atomic claims and persistent Stop.
- Explicit/reply/focus routing, quoted mentions, acknowledgements, substantive burst renewal, and scoped Resource access.
- Independent-input revision freezing, staged outputs, Resource edit conflicts, and mismatched revision ownership rejection.
- Required preview revisions, stale project/task scope, dependency cycles, readiness, shared locks, disabled agents, revocation and project changes after claim.
- Task-output callback deduplication and pinned prerequisite output propagation.
- Owner login, Host/Origin/CSRF checks, authenticated uploads/downloads, checksum damage, orphan quarantine, and opaque storage-key validation.
- Work-hook authentication, duplicate delivery, changed-input rejection, SSE replay, overflow, and a cursor ahead of restored history.
- Real local Git worktree/checkpoint isolation, ancestry verification against a dedicated fixture branch, traversal/symlink rejection, tracked formatting errors, and host checkout-filter rejection.
- Protocol call/turn bindings, duplicate tools, cancellation versus terminal observation, uncertain transport loss, startup serialization/cancellation/timeouts without retries, and normalized quota events.
- Capacity normalization, stale/missing telemetry, known exhaustion, no inferred reset credit, and a worker-level hold without invoking a model.
- Bounded older-message retrieval, current context after pending admission, late-correction labels, and immutable initial packets.
- GitHub publication denial. Successful push/PR creation is not implemented or claimed as tested.

## Browser acceptance

Actual interactions in the Codex in-app browser verified sign-in, spaces/projects, live AI planning, scope approval, result review, dependent work, Resource editing/version history, file upload, real GitHub linking, composer draft persistence after refresh, keyboard clearing, and Stop of queued work. Two tabs edited the same Resource revision; one saved and the other retained its draft with a visible conflict. A message attached revision 3; after revision 4 existed, opening the attachment still showed revision 3's text and offered View latest.

Desktop layout was inspected visually at 1280 pixels. Phone-width inspection at 390 pixels showed the compact navigation, composer, and settings control with no horizontal document overflow. This is browser viewport testing, not qualification on a physical phone. No application console errors were observed in the final browser check. Browser data is isolated from the clean owner database.

## Recovery boundary

The live interruption check uses an actual model turn. The uncertain-receipt restart check deliberately seeds a synthetic in-flight receipt, then restarts the real worker and verifies no redispatch. The restore test similarly seeds pending/unknown fixtures and verifies cancellation/holding with dispatch disabled. These fixtures exercise otherwise difficult failure states without claiming a real machine crash occurred.

A queue claim never proves exactly-once provider execution. Uncertain results require inspection and deliberate resolution. The saved manifests distinguish initial input from later tool retrieval and corrections. Event-ring omissions are counted, and retrieval receipts are retained separately.

## Open qualification and product work

- AgentOS/Hermes connectivity is pending. Only local Codex is qualified.
- GitHub publishing, event synchronization, PR checks, and multi-branch candidate integration are pending. Existing PR reads and local commit ancestry are working.
- Project test scripts and arbitrary shell execution are unavailable in this scoped profile. Its checks cover patch formatting and JSON syntax. This release is not an unattended general-purpose build/test environment.
- Automatic agent invitations, model-assisted speaker selection, generated briefs, explicit decision records, separate channel/thread views, and sophisticated paste highlighting remain roadmap work.
- Roadmap scale, database pagination/full-text search, and multi-user operation are not qualified. The v0 implementation favors consistent local records over large-scale throughput.
- The standalone capacity probe reached its bounded deadline. Some live runs returned telemetry with one missing usage window. The UI therefore reports unknown where appropriate; time and worker limits still apply. No credit consumption, reset redemption, or billing change occurred.

These limits are also documented in README.md. The source plan remains intact so the next implementation can proceed from evidence rather than treating the broader roadmap as finished.
