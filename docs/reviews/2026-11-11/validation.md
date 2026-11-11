# Validation record

Reviewed code head: `3ac71534fcaf939aa986b184c45821f3ba5de27b`.

The baseline was the recorded main commit, not the handoff's older `9227e49`. Baseline and review builds used separate checkouts. Rust tests use local PostgreSQL 17 and the existing official Rust 1.97.1 toolchain. The initial baseline web checks used the environment-provided Node 24.18.1, with an engine warning; review checks use the repository-pinned Node 24.22.1, installed from the checksum-verified official distribution. Both use pnpm 11.18.0. Chromium 153 was installed through Playwright for local browser tests.

## Baseline

- `cargo test --workspace --locked --tests`: 891 passed, 0 failed, 0 ignored, including all 302 PostgreSQL integration tests.
- Web lint and typecheck passed. All 584 web tests in 104 files passed.
- The first sandboxed Rust library run had 559 passed and 1 failed. `config::tests::a_new_keyring_stays_out_of_served_roots_and_git_work_trees` found the cloud sandbox's synthetic `/tmp/.git` marker, so it correctly refused that temporary path. The unchanged test and complete unchanged baseline passed using `TMPDIR=/var/tmp/aw-review-temp` through the approved executor route. The keyring check was not weakened, and the initial failure remains in the portable logs.

## Focused review tests

- Storage admission: 4 passed, including vanished descendants and a populated symlinked root.
- Canonical public origin: 1 passed, including normalized scheme spellings and default port removal.
- Reply expiry and completion-context idempotency: 1 PostgreSQL integration test passed. It checks final/interim sends, progress renewal, transactional rollback and exact versus changed-mode replay. The initial progress regression failed before the completed guard was rebuilt; the final rebuild passes.
- Untimed saved note: 1 PostgreSQL integration test passed. It reads back the note and checks no reminder notification is emitted.
- Reviewed resolver/archived-project sweep: 3 PostgreSQL integration tests passed, including the new historical-alias case.
- Mark-all-read regression: failed on the prior implementation, passed after preserving partial results and counting expansion failures.
- Head web tests: 585 passed in 105 files.

The first combined check failed typechecking because `Array.findLast` was outside the project's configured library target. The final code uses a copied reverse search instead. No TypeScript target or test expectation was loosened.

## Full check run

The first complete Rust run at code head `c7c5c17` ran 903 tests: 899 passed, 4 failed, 0 skipped. Removing the fallback had broken four existing cases involving archived moved originals: `mcp_admin::moved_tasks_carry_over_and_old_keys_follow_them`, `mcp_groundwork::archiving_releases_claims_and_moved_originals_stay_archived`, `mcp_task_batches::bulk_tools_check_access_for_every_task`, and `mcp_task_criteria::history_of_a_moved_task_points_back_only_where_the_reader_reaches`. The correction distinguishes missing, unique and ambiguous shared results, retaining the original fallback only when it does not override ambiguity. All four existing regressions and the new historical-alias case then passed together with retries disabled. No existing assertion was weakened. These are review-branch regressions, not baseline failures.

The final `tools/ci/checks.sh` completed successfully on code head `3ac71534fcaf939aa986b184c45821f3ba5de27b` in 1,203 seconds:

- CI/admission/pinned-Node script fixtures, formatting, self-hosted guide, Hermes fixtures and Agentos/Nyx fixture tests passed.
- Web lint and typecheck passed. Clippy passed with warnings denied.
- All 903 Rust tests passed, 0 skipped, with no retries. Nextest test runtime was 351.682 seconds.
- Doc tests passed. Cargo-deny 0.20.2 reported advisories, bans, licenses and sources all OK.
- All 585 web tests in 105 files passed.
- Production web build, asset budgets and performance-tool checks passed.
- All 97 Chromium browser tests passed in 3.7 minutes, including desktop/phone and light/dark screenshots. Vite emitted transient WebSocket ECONNRESET messages; no browser test failed or retried.

The final documentation-only commit does not change the tested runtime tree. Full console logs, including the earlier failures, are retained in the portable package. No checks were skipped to obtain the final green result.

## What these checks do not establish

The 43 blocked ledger entries lack their own completed independent validation or safe tested implementation. Passing the existing suite does not convert those entries into fixed or invalid findings.

No deterministic race-interleaving regression was added for PR #211's timeline metadata or task-link rechecks, and no deterministic DNS-stall fixture was added. The provider retry guard is tested with the demo harness, not a live authenticated Codex process. No live deployment, destructive restore drill, production email or remote credentialed push was exercised as a regression test.
