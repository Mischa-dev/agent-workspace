# Current frontend feature inventory

> **Historical.** This describes the frontend as it was in August 2026 and is
> no longer kept up to date. For what is planned, see
> [`docs/plans/roadmap.md`](../plans/roadmap.md); for what exists, read the
> code under `apps/web/src/`.

**Baseline:** `61bb577034d77dbcdb1d06f141036f51ede4bb7f` (PR #86)

**Captured:** August 19, 2026

**Frontend:** React 19, TypeScript 6, Vite 8, TanStack Query 5, React Router 7

## Baseline evidence

- Exact runtime: Node `24.18.1`, pnpm `11.18.0`.
- Lint, typecheck, and all 28 Vitest files passed (125 tests).
- Production output: one `455,684` byte JavaScript asset (`125,568` bytes
  gzip), one `36,908` byte CSS asset (`7,623` bytes gzip), and `450` byte HTML.
- Configured limits: 500 KiB raw JavaScript, 160 KiB gzip JavaScript, and
  60 KiB raw CSS. Baseline has no violations.
- Performance-tool contract tests passed (deterministic percentiles, loopback
  origin enforcement, cookie redaction, durable Thread selection, and complete
  emitted-asset measurement).

## Route inventory

| Route | Surface | Principal actions and states |
|---|---|---|
| `/sign-in` | Local-account authentication | Rate-limited error, focused error summary, password clearing, safe same-origin return route, submitting state. |
| `/join#token=…` | Invitation resolution and acceptance | Secret removed from URL before fetch; new/existing/signed-in Actor modes; invalid, expired, conflict, loading, and submitting states. |
| `/home` | Global Home and recent Projects | Project restore/switch, attention counts, create capability, list skeleton, empty/error/notice states. It intentionally does not aggregate Project conversations. |
| `/projects/new` | Project creation | Organization eligibility gate, validation, stable retry idempotency key, unavailable/forbidden states. |
| `/projects/:projectId` | Project overview | Role, attention, archived/read-only state, settings capability, remembered navigation, context drawer. |
| `/projects/:projectId/settings` | Project settings hub | Team/invitations, external agents, sponsored execution, personal Execution Profile account, repositories, GitHub integration/automation, exact-key archive confirmation, capability-aware read-only states. |
| `/projects/:projectId/work-items` | Work Item list and board | URL-backed search/type/status/priority/archive filters, pagination, creation dialog, list/board switch, drag projection plus keyboard move controls, loading/empty/error/conflict states. |
| `/projects/:projectId/work-items/:workItemId` | Work Item detail | Optimistic editor, conflict recovery, acceptance criteria, comments/activity, Thread/repository/PR links, archive, Work with Worker, Open in Codex, context details. |
| `/projects/:projectId/channels/:channelId` | Channel timeline | Paginated durable AST timeline, optimistic send/retry, semantic mentions and Work Item references, Thread creation, archive, mark-seen, realtime stream, archived-parent fallback. |
| `/projects/:projectId/conversations/archived` | Conversation archive | Channel/Thread archive discovery, filtering, loading/error/empty states, explicit history links. |
| `/projects/:projectId/threads/:threadId` | Thread and hosted execution | Durable/streaming timeline, run start/interrupt/retry, questions and approvals, publish, pull-request create/refresh, failure acknowledgement, Work Item links, recovery and repository context, non-disclosing route fallback. |
| `*` | Not found | Authenticated recovery to Global Home. |

## Shell and navigation

- `AppShell` owns the four-region grid, skip link, narrow Project rail, Project
  sidebar, primary main landmark, optional context drawer, and phone drawers.
- The Project rail provides Home, ordered pinned/recent/active Projects,
  attention counts, Project search, pinning, creation, and sign-out.
- Project switching preserves server-owned per-Actor locations. Viewer routes
  navigate without issuing a forbidden navigation write.
- The Project sidebar provides overview, Work Items, Channels, compact active
  Threads with icon plus text attention, archive, settings, and capability-gated
  Channel creation.
- At 980 px the context drawer is currently hidden. At 700 px the rail and
  sidebar become left drawers with focus containment, inert background, Escape,
  backdrop close, and trigger-focus restoration.

## Data and component dependencies

- `App` owns the Query client and all eager route imports.
- `SessionBoundary` owns session verification, Actor-cache clearing, the
  authenticated provider boundary, and one `RealtimeProvider`.
- `AppShell` depends on `ProjectRail` and `ProjectSidebar`; both use Project and
  conversation query hooks and server-returned capabilities.
- Project screens use `useProject`, remembered navigation, and the shell. The
  settings route composes Team, external-agent, sponsored-execution, execution,
  repository, and GitHub feature components.
- `ChannelPage` combines the conversation timeline query, realtime stream,
  archive/seen/send mutations, structured content, the composer, and Thread
  creation.
- `ThreadPage` combines wire normalization, query orchestration, run/approval/
  publication mutations, durable/streaming reconciliation, timeline rendering,
  composer controls, and execution context in one 1,428-line module.
- `RealtimeProvider` combines socket lifecycle, duplicate-event bounds,
  reconnect backoff, durable invalidation, animation-frame delta coalescing,
  and ephemeral stream cleanup.
- `WorkItemsPage` combines URL filters, list/board queries, board movement, and
  creation-dialog behavior. `WorkItemDetailPage` combines query orchestration,
  draft/conflict editing, execution entry points, criteria, links, and activity.
- `styles.css` owns tokens, reset, shell, primitives, every feature surface,
  responsive rules, forced colors, and reduced motion in one 2,624-line file.

## Security and authority boundaries to preserve

- Browser sessions use same-origin credentials; CSRF is added only to unsafe
  methods. Actor identity and capabilities are server-derived and fail closed.
- Query keys include Project boundaries. Cross-Project and inaccessible
  conversations recover without disclosing the resource.
- Credentials, invitation secrets, API keys, and bearer tokens are never stored
  in browser storage. One-time secrets are cleared and are never placed in URLs.
- Message AST and provider output are bounded, validated, and rendered inert.
- Mutation idempotency keys survive retries where a replay is valid; pull-request
  creation remains an explicit non-blindly-retried action.
- WebSocket data is invalidation and bounded ephemeral presentation, never the
  durable source of truth. Reconnect triggers authorized HTTP convergence.

## Existing accessibility and reliability coverage

- Skip link and main landmark; semantic navigation; native labelled forms.
- Visible focus, dialog/drawer focus containment, inert backgrounds, Escape and
  focus restoration, mobile 44 px targets, polite/assertive live regions.
- Thread attention and Work Item/run states use labels/icons in addition to
  color. Forced-colors and reduced-motion rules exist.
- Axe smoke coverage exists on representative Work Item output; semantic tests
  cover drawers, dialogs, hostile content, board controls, picker keyboards,
  conflicts, capability gates, and retry behavior.
- Missing baseline evidence: retained multi-viewport screenshots, a manual
  keyboard route matrix, Chrome/VoiceOver notes, and route-level automated axe
  coverage across all feature families.

## Incremental migration seams

1. Route imports can become lazy without changing URLs or contracts.
2. Theme, density, tokens, and primitives can wrap existing feature markup.
3. The shell can migrate independently while preserving `AppShell` props.
4. Realtime socket orchestration can move into a hook without changing context.
5. Thread timeline/context/run controls and Work Item list/board/detail sections
   are stable responsibility boundaries; no duplicate server model is needed.
