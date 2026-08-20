# Frontend overhaul verification

Original verification was recorded on August 19, 2026 from
`codex/frontend-overhaul`, based on the merged PR #86 commit
`61bb577034d77dbcdb1d06f141036f51ede4bb7f`. Frontend merge-readiness checks
were rerun on August 20 after the route-loading and accessibility corrections.

## Automated checks

All commands used the repository-pinned Node 24.18.1 runtime.
Frontend, asset-budget, and diff checks were rerun on August 20. Rust and
performance-tool rows retain the August 19 branch evidence.

| Check | Result |
| --- | --- |
| `cargo fmt --check` | pass |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | pass |
| `cargo test --workspace` | pass |
| `cargo deny check` | pass; configured duplicate warnings only |
| `corepack pnpm lint` | pass |
| `corepack pnpm typecheck` | pass |
| `corepack pnpm test` | 31 files, 131 tests pass |
| `corepack pnpm build` | pass, lazy feature routes emitted |
| `corepack pnpm check:assets` | pass |
| `corepack pnpm check:performance-tools` | 5 tests pass |
| `git diff origin/main --check` | pass |

The component suite includes axe checks for the authenticated application,
invitation flow, Work Items list and creation dialog, external agents,
sponsored execution, and team settings. It also includes focus-containment and
focus-restoration tests for shell drawers and the Work Item creation dialog.
Automated results are evidence, not a WCAG conformance claim.

## Asset measurements

| Asset group | Baseline | Overhaul | Limit |
| --- | ---: | ---: | ---: |
| JavaScript raw | 455,684 B | 471,854 B | 512,000 B |
| JavaScript gzip | 125,568 B | 141,722 B | 163,840 B |
| CSS raw | 36,908 B | 61,289 B | 61,440 B |

The initial application chunk is 319,546 B raw / 97,509 B gzip. Feature routes
are lazy chunks; the largest is Project Settings at 38,431 B raw. Thread is
22,700 B and Work Item Detail is 25,326 B, both below the 120 KiB route-review
threshold. The CSS budget has intentionally narrow headroom and must not be
relaxed without a measured design-system consolidation.

## Browser checks

The production behavior was exercised against a disposable local PostgreSQL
database, seeded Project, Rust server, and authenticated browser session.

| Viewport | Evidence |
| --- | --- |
| Desktop 1440×900 | Four exact regions: 64 px rail, 248 px sidebar, 840 px primary workspace, 288 px context; no horizontal overflow. |
| Tablet 834×1112 | Persistent rail/sidebar, explicit modal Details sheet, close control receives focus, inactive regions inert, Escape restores Details; no overflow. |
| Mobile 390×844 | Rail, Project sidebar, and Details open independently; inactive regions inert; Escape restores each trigger; toolbar targets are 44 px; no overflow. |

Additional manual checks covered the Work Items list/board projection, empty
state, creation-dialog initial focus and Escape restoration, Thread empty/run
state, Project Settings hierarchy, System/Dark/Light appearance controls, and
the public sign-in surface. No records were created during browser QA.

## Retained visual captures

The August 19 desktop captures were taken before their lazy routes finished
loading. They are retained as diagnostics, not accepted final-state evidence,
and must be replaced after the branch is rebased and served from its combined
tree.

- [Stale desktop Project loading capture](screenshots/desktop-project-dark.png)
- [Stale desktop Work Items loading capture](screenshots/desktop-work-items-dark.png)
- [Tablet Details sheet](screenshots/tablet-details-dark.png)
- [Mobile Details drawer](screenshots/mobile-details-dark.png)

## Remaining acceptance boundaries

- Forced-colors, screen-reader traversal, and 200% zoom still require a human
  assistive-technology pass before any WCAG conformance claim.
- Fresh fully loaded desktop Project and Work Items captures, plus a 701–760 px
  navigation check, remain required after the upstream branches are combined.
- Final visual design sign-off remains a human acceptance gate.
- Offline mutation behavior remains intentionally unsupported; the shell reports
  reconnecting and rechecks durable state.
- Notification center and command palette remain deferred until server contracts
  exist.
