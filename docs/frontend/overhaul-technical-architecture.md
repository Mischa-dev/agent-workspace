# Frontend overhaul technical architecture

## Principles

1. Incrementally replace presentation around accepted behavior.
2. Keep HTTP/PostgreSQL state authoritative; realtime is invalidation plus
   bounded ephemeral presentation.
3. Keep feature ownership visible. Shared UI primitives own interaction
   contracts, not business policy.
4. Keep server capabilities fail-closed. Presentation preferences never grant
   authority.
5. Measure route chunks and total emitted assets on every production build.

## Module boundaries

- `app/`: Query client, providers, route table, route fallbacks.
- `auth/`: session normalization, authentication boundary, sign-in.
- `shell/`: global/Project navigation, responsive modal navigation, appearance.
- `theme/`: validated presentation preferences, system resolution, DOM tokens.
- `ui/`: accessible, dependency-light primitives and async state components.
- Feature folders (`projects`, `conversations`, `threads`, `work-items`,
  `repositories`, `github`, `team`, `execution`, `external-agents`) own their
  queries, contract adapters, screens, and feature components.
- `realtime/`: bounded wire parsing, socket lifecycle, durable invalidation, and
  ephemeral stream state.
- `styles/`: semantic tokens, base rules, primitives, shell, and feature styles.

## Route and chunk strategy

The provider shell and authentication boundary remain in the entry chunk.
Public authentication and each protected feature route load through
`React.lazy`/`Suspense`. Vite owns chunk graph generation; no manual vendor chunk
is introduced until measurement proves it useful. The asset report lists every
emitted file and aggregate JavaScript/CSS budgets, so splitting cannot hide total
growth.

## State and API contracts

- TanStack Query owns normalized server responses. Query keys start with the
  Project boundary for Project-scoped data.
- Local state owns open drawers/dialogs, drafts, URL filter staging, and
  appearance preferences. No fetched entity is copied into a global store.
- Wire adapters continue to bound text, identifiers, arrays, payload size, and
  known enum/capability values. A schema library is deferred because existing
  explicit adapters are small, tested, and add no runtime dependency.
- Unsafe methods use the central same-origin/CSRF client. Mutations retain an
  idempotency key for the lifecycle of one logical retryable command and rotate
  only after success. Non-replayable commands remain explicit.

## Realtime ownership

- `RealtimeProvider` publishes a small context only.
- `useRealtimeConnection` owns one authenticated socket, duplicate-ID bounds,
  reconnect backoff, close cleanup, and lifecycle state.
- Durable event invalidation is coalesced per Project. Open Project queries and
  the Project rail refetch after connect/reconnect.
- Provider deltas are bounded and coalesced once per animation frame. Durable
  message/run events clear only their correlated ephemeral streams.

## Error and loading strategy

Route `Suspense` uses an application-shell-shaped fallback. Screen queries keep
feature-specific skeletons because they accurately preserve layout. A root
error boundary catches rendering failures and offers a clean reload; resource
errors remain local so known navigation and durable context stay available.

## Responsive navigation state

`AppShell` owns one mutually exclusive modal region: Project rail, Project
sidebar, or context. It applies `inert` to every inactive shell region, contains
focus, closes on Escape/backdrop, and restores the originating trigger. CSS
breakpoints change structural presentation; route markup and semantics stay the
same.

## Theme and token resolution

- Primitive OKLCH ramps feed semantic surface, text, border, action, status, and
  identity roles.
- Theme preference is `system | dark | light`; density is
  `compact | comfortable | spacious`.
- A versioned custom-theme object accepts only bounded CSS color strings for a
  small semantic allowlist. Invalid fields are discarded and missing roles fall
  back to the selected theme.
- Only presentation preferences are stored under one namespaced local-storage
  key. The provider applies `data-theme`, `data-density`, `color-scheme`, and
  safe custom properties to the document root.

## Primitive ownership

Project-owned primitives cover buttons, form controls, dialog/drawer/popover,
tooltip/menu, tabs/segmented controls, identity/status, loading/progress,
empty/error, and announcements. They prefer native semantics and do not import a
component framework. Feature components continue using native elements during
incremental migration; shared primitives are adopted when they reduce duplicate
behavior, not by blind wrapper conversion.

## Stable decompositions

- `AppShell`: modal-region state/focus versus rail/sidebar feature content.
- `RealtimeProvider`: context publication versus socket/invalidation/stream hook.
- `ThreadPage`: wire model/query hook, page orchestration, timeline entries,
  approvals/run controls, and context details.
- `WorkItemsPage`: URL filter ownership, list projection, board projection, and
  creation dialog.
- `WorkItemDetailPage`: query boundary, draft/conflict editor, execution actions,
  criteria, links, collaboration, and context.
- Styles: split by layer (`tokens.css`, `base.css`, `components.css`) and by
  surface (`shell.css`, `conversation.css`, `pages.css`, `work.css`). The single
  legacy `styles.css` was removed in the redesign's second pass.

## Test pyramid and budgets

- Pure tests: normalizers, preference validation, query keys, stream models.
- Component tests: primitives, modal focus, capability-aware feature states,
  hostile rendering, conflicts, keyboard interactions.
- Route tests: authenticated boundaries and critical cross-feature flows.
- Browser/manual: desktop 1440×900, tablet 834×1112, mobile 390×844; keyboard,
  200% zoom, reduced motion, forced colors, dark/light.
- Existing hard budgets remain 500 KiB raw JS, 160 KiB gzip JS, 60 KiB raw CSS.
  Route-level files are additionally reported; a single route chunk above 120
  KiB raw requires review even when totals pass.

## Incremental completion rule

Old and new selectors may coexist only inside a named feature migration seam.
There is one router, one server-state model, and one shell. No duplicated
application tree, speculative task model, or alternate realtime authority is
permitted.
