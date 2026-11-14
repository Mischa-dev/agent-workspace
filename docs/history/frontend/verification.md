> **History.** Kept as a record of how Agent Workspace was built; don't follow it. Where it is going is in [`docs/plans/master/`](/docs/plans/master/README.md).

# Frontend overhaul verification

Original verification was recorded on August 19, 2026 from
`codex/frontend-overhaul`, based on the merged PR #86 commit
`61bb577034d77dbcdb1d06f141036f51ede4bb7f`. Frontend merge-readiness checks
were rerun on August 20 after the route-loading, accessibility, upstream M10,
task-platform architecture, and responsive-layout corrections were combined.

## Automated checks

All frontend commands used the repository-pinned Node 24.18.1 runtime. The
table records the final combined-tree gate run on August 20.

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
| JavaScript gzip | 125,568 B | 141,702 B | 163,840 B |
| CSS raw | 36,908 B | 61,305 B | 61,440 B |

The initial application chunk is 319,546 B raw / 97,512 B gzip. Feature routes
are lazy chunks; the largest is Project Settings at 38,431 B raw. Thread is
22,700 B and Work Item Detail is 25,326 B, both below the 120 KiB route-review
threshold. The CSS budget has 135 B of headroom and must not be relaxed without
a measured design-system consolidation.

## Browser checks

The freshly built production bundle was served by the Rust host and exercised
against a disposable local PostgreSQL database, seeded Project, authenticated
administrator, Thread, and three representative Work Items.

| Viewport | Evidence |
| --- | --- |
| Desktop 1440×900 | Four exact regions: 72 px rail, 248 px sidebar, 840 px primary workspace, 280 px context; no horizontal overflow. |
| Tablet 834×1112 | Persistent rail/sidebar; Work Item filters and rows reflow without document overflow; the modal Details sheet focuses its close control, makes inactive regions inert, and restores Details on Escape. |
| Breakpoint 760×900 and 701×900 | Mobile shell activates consistently; Project search remains at layer 80 above layer-70 drawers, makes the application root inert, and restores Search Projects on Escape; no document overflow. |
| Mobile 390×844 | No document overflow; visible toolbar/action controls are 44 px high; Project search fits inside the viewport, remains above the open drawer, makes the root inert, and restores its trigger on Escape. |

Additional checks covered administrator sign-in; Work Item create, versioned
update, list, and board projections; creation-dialog initial focus and Escape
restoration; Channel and Thread empty/run states; Project Settings hierarchy;
Dark/Light appearance controls; and the final hashed production assets. QA
records existed only in the disposable database used for this pass.

## Retained visual captures

The accepted desktop captures were retaken from the fully loaded production
bundle on the final combined tree.

- [Desktop Project](screenshots/desktop-project-dark.png)
- [Desktop Work Items](screenshots/desktop-work-items-dark.png)
- [Tablet Details sheet](screenshots/tablet-details-dark.png)
- [Mobile Details drawer](screenshots/mobile-details-dark.png)

## Remaining acceptance boundaries

- Forced-colors, screen-reader traversal, and 200% zoom still require a human
  assistive-technology pass before any WCAG conformance claim.
- The responsive reflow was verified at the 701 px and 760 px CSS-pixel edges,
  but the browser harness did not expose actual page-zoom or forced-colors
  emulation.
- This verification is sufficient for merge readiness, not a formal WCAG
  conformance statement.
- Offline mutation behavior remains intentionally unsupported; the shell reports
  reconnecting and rechecks durable state.
- Notification center and command palette remain deferred until server contracts
  exist.
