# Independent review of PR #211

Reviewed immutable head `9de9350ae4a95b2ee2a1c6258845ff463735f40c` against `dc695b5707ef6f74111f92b2e79ef2c884af1b75`. The open draft and AWD-107 were inspected read-only. No review thread, task or organizer state was changed. Local cherry-picks retain Claude's authorship as `beb4bc6` and `93c87bd`.

The context-only PR #207 at `8358d87b36f48d898c9fdc5540d1c6d7edec57bd` was read, never merged. All eight patch files were inspected. Their labels and prior compile result did not determine acceptance.

| Ledger IDs | Decision and evidence |
|---|---|
| AW-029 | Accept unique project-key resolution. `project_reference` requires exactly one match; the integration test checks neither project is archived on ambiguity. |
| AW-030 | Revise. PR #211 detects two current keys, but its `resolve_among` fallback could choose one current key after the shared resolver refused conflicting historical aliases. It also limits raw rows before deduplicating IDs. The local follow-up groups by task ID before limiting and distinguishes missing, unique and ambiguous results. The archived-original fallback remains when the moved target cannot be read, but cannot override ambiguity. A new Postgres regression covers current and historical keys across workspaces; the existing moved-original regressions protect the fallback. |
| AW-031 | Accept the enabled workspace-membership join for archived-project listing. The ordinary shared helper intentionally excludes archived projects, so calling it verbatim would erase this feature. The test removes workspace membership while retaining project membership. |
| AW-021 | Accept viewer-aware `timeline_requests` queries. Both reply rows and remaining-turn count check `conversation_visible_to` when read. No deterministic interleaving test was supplied. |
| AW-033 | Accept the executor-generic `record_visible` and in-transaction recheck after the task lock. This closes the earlier unchecked interval before entering the transaction. It does not claim global serializability of all membership changes. |
| AW-002 | Accept request-specific approval summaries with bounded redaction. Tests check command, working directory, network host, files, permission entries, redaction and output size. A summary is not the entire provider request. |
| AW-007 | Accept settling a provider turn before starting a new turn on a resumed thread. Unsupported inspection is allowed for adapters without reusable turns; other errors refuse the start. The demo-harness test verifies interruption and the no-open-turn case. |
| AW-009 | Accept periodic lease renewal for publish, PR and PR-sync workers. Tests cover long jobs, lost ownership and repeated renewal errors. Limitation: dropping an async future cannot retract a push already running in a blocking thread, as the implementation explicitly documents. This reduces ordinary lease-expiry duplicate work, without promising exactly-once external effects. |
| AW-010 | Accept clearing all publication fields and incrementing `publish_version` when a retained workspace is reused. The scheduler integration test exercises publication reset. |
| AW-016 | Accept retaining worktrees that contain ignored files in both removal paths. Publication/retry dirtiness intentionally keeps its narrower definition. The Git regression creates a real ignored file and checks it survives. |
| AW-036 | Accept a DNS lookup timeout before constructing the HTTP client. This is separate from multi-address fallback, which remains blocked as AW-117. No deterministic DNS-stall fixture was added. |
| AW-008 | Accept documentation of the schema-backed `thread/read` turns contract. This is a documentation fix, not new provider behavior. |
| AW-039 | Accept the revised mail transport selection: owned workspace, stored chosen default, or sole enabled workspace. This avoids the computed-earliest default in a multi-workspace account. The owner-only reference patch was rejected because it would remove mail from invited members; PR #211 includes the relevant reset-mail and self-hosting regressions. The sole-workspace and explicitly chosen-workspace trust rules remain policy assumptions. |

## Reference patch decisions

- Archived briefing, project keys, DNS timeout, ignored-file retention and pinned-contract documentation were reused through PR #211, with its tests.
- The task-key patch was revised further for historical aliases and distinct-ID counting.
- Retry publication was reused through PR #211 with its added `publish_version` update and scheduler test.
- The owner-only mail patch was rejected in favor of PR #211's tested routing rules.

## Deliberate exclusions

OAuth refresh grace is unchanged. Neither short-lived replacement refresh tokens nor access-only grace answers were introduced. A late answer can overwrite a shared client token store; the existing parallel-refresh behavior is intentional. The five earlier policy calls are also unchanged. See the ledger for these and the additional design decisions.

PR #205 owns the excluded things-resolver parameter mismatch. PR #208 owns newer task-move work. PRs #209 and #210 concern page blocks, and #201 is a held UI direction draft. None were incorporated into this integration branch.
