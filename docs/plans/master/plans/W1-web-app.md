# W1 The web app: plan

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner for W1, 2026-11-14, against `main` at fe95a74. Read-only; nothing built or run. Paths
under `apps/web/src/` are written from `src/` (for example `realtime/useRealtimeConnection.ts:87`);
other paths are from the repository root.

## 1. Goal

When W1 is done, the app is one shell mounted once, with the workspace in every address
(`/w/acme/...`), so a sidebar never reloads and any link says which workspace it is in. Every
call the web app makes uses generated types and a generated client, a change by anyone (a
person in another tab, an agent through MCP) shows on screen by updating exactly the things it
names, and nothing refetches a whole project. Tasks, collections, pages lists, the inbox and
connected modules draw through the same list, table, board, calendar, timeline, gallery, chart,
detail and form pieces, which work on a phone as well as a laptop, and a message or edit made
on a train is sent when the connection comes back.

## 2. Where the code is today

### 2.1 Routes and the shell

- **Router.** `app/App.tsx:89` uses the declarative `<BrowserRouter>` with about 50 hand-written
  `<Route>`s (`App.tsx:93-381`) and 44 `lazy()` page imports (`App.tsx:24-68`). There is no
  route table, no route `handle`, and so no `useMatches()`; those need a data router
  (`createBrowserRouter`).
- **Signed-in layout already exists, but not the shell.** `auth/SessionBoundary.tsx:89-97` is
  a layout route: it mounts `RealtimeProvider`, `CommandCenter`, `DocumentTitle`, the lazy
  `AppearanceSync` and `Outbox` once, around `<Outlet/>`. The frame (rail, sidebar, main,
  context panel) is not there.
- **Each page renders its own shell.** `<AppShell` appears 73 times in 36 non-test files
  (counted). `AppShell` takes page-specific props: `context`, `contextKey`, `direct`, `home`,
  `mainClassName`, `onContextDismiss`, `project`, `selectedChannelId`, `thread`
  (`shell/AppShell.tsx:66-100`). Consequences visible in code:
  - `shell/SidebarParts.tsx:17-24`: "Each page renders its own shell, so the sidebar mounts
    afresh on many clicks"; scroll and folded state live in module-level `Map`s.
  - `shell/MobileNavigationProvider.tsx:7-8`: the phone drawer state is hoisted above the routes
    for the same reason.
  - `shell/AppShell.tsx:37-45` guesses the project from the URL so a loading page keeps its
    sidebar; `app/App.tsx:423-446` re-derives `home` and `direct` from the pathname (with a
    whiteboard exception) to draw a matching fallback; `shell/homeSpace.ts:6` keeps its own list
    of Home paths.
- **Why the persistent layout route was put on hold.** I found no written reason in the
  repository, the docs or the commit log (searched for "layout route", "ShellLayout",
  "persistent", and the AWD project search returned nothing). From the code, the blockers are:
  (1) the declarative router has no route `handle`, so the shell can't learn "which space and
  which layout" from the route without a router switch; (2) pages push five kinds of state into
  the shell through props (context panel, its open key, its dismiss callback, the selected
  channel and thread, and the main layout class), so a layout route needs a page-to-shell
  channel first; (3) 73 call sites in 36 files is a wide edit while many agents work in parallel
  (`AGENTS.md`, "changes must merge in either order"); (4) the fallback and the module gate sit
  inside each route element (`App.tsx:402-412`), so the loading frame and the page can disagree.
  This is inferred; the lead should confirm with the owner whether there was another reason
  (for example a visual regression found in a try that was never committed).
- **Workspace.** No route has a workspace segment (`App.tsx:141-380`). Home and workspace pages
  mean "the default workspace": `experience/layout.ts:233` `useHomeWorkspaceId()` returns the
  session's default organization or the first one, used in 10 places. Session already carries
  each organization's `slug` (`auth/session.ts:25,105-112`).
- **Task keys.** `/:taskKey` (`App.tsx:379`) goes through `app/TaskKeyRedirect.tsx:13-36`, which
  asks the person to pick when keys repeat across organizations.
- **The server answers any unknown path with `index.html`** (`apps/server/src/lib.rs:222-226`),
  so `/w/...` needs no server routing change. The service worker does: it only serves the
  offline shell for a fixed list of first segments (`apps/web/public/sw.js:23-24`).

### 2.2 Modules in the web app

- Manifests in `modules/*.json` are projected by Vite into `virtual:modules/slots/<kind>`
  (`apps/web/vite.config.ts` `moduleProjections`, about lines 47-98) and read by
  `modules/slots.ts`, `modules/ownership.ts` and `modules/pageOwners.ts`. They decide which
  module owns a route (for `ModuleGate`) and are checked by `modules/slots.test.ts`, but they do
  not build anything: rows are hand-written in `shell/ProjectSidebar.tsx:96-139` (Runs links to
  `/launches` at `:135`), routes in `App.tsx`, palette entries in `shell/CommandPalette.tsx`.
- `modules/ModuleGate.tsx:37` copies the whiteboard and run detail query keys by hand.
- The right pattern exists inside the app: `settings/registry.ts:53-95`, one line per settings
  page (group, id, label, icon, keywords, lazy import, visibility, layout part), read by nav,
  search and routing.
- Layout registry: `experience/registry.json` (74 `"id"` entries counting groups, parts and
  presets) and the hand mirror `experience/parts.ts`. `experience/registry.test.tsx:172-195`
  fails when a file uses `<SidebarLink` or `<PageTabs` and is not in its list. Pages ask
  `shows(part)`; the server resolves the hidden list including turned-off modules
  (`experience/layout.ts:30-45`).

### 2.3 Data layer

- **Library.** TanStack Query 5.101 (`apps/web/package.json`), one `QueryClient`
  (`app/App.tsx:70-82`: refetch on focus, one retry, no mutation retry). No other store.
- **Client.** One wrapper `api/client.ts:46-111` (`requestJson`): CSRF header, `idempotency-key`
  header, typed `ApiError` (`api/errors.ts`), an auth-expired window event. Keep it; the
  generated client should call through it.
- **Types.** Hand-written. 33 `api.ts`/`types.ts`/`wire.ts` files export 329 types (counted with
  `grep "^export type\|^export interface"`; most are wire types). 369 `requestJson<…>` calls in
  68 files, 120 of them `requestJson<unknown>` followed by hand normalisers; 20 local copies of
  `asRecord`/`record`/`asArray` helpers. Drift already present (each checked):
  `EffectiveSettings` at `settings/api.ts:46` and `launch/api.ts:171`; `ViewType` at
  `modules/contract.ts:28` (6 values) and `collections/api.ts:40` (4 values); `RunState` at
  `threads/threadTypes.ts:8` and `runs/api.ts:3`; `BoardColumn` at `work-items/types.ts:287` and
  `collections/collectionModel.ts:209`; two `FieldType` enums, `modules/contract.ts:12` (13
  values, nullable `FieldValue`) and `fields/fieldModel.ts:13` (9 values, non-nullable). The
  realtime parser accepts either casing (`realtime/realtimeModel.ts:35` `field(record, camel,
  snake)`) because the wire is not pinned.
- **Keys.** Three generations: the central `api/queryKeys.ts` (everything under
  `['project', id, …]`), about 20 feature-local factories, and inline literals in 42 files
  (`queryKey: [` count). 157 literal key arrays start with `'project'`, `'organization'`,
  `'things'`, `'whiteboards'`, `'inbox'` or `'direct'`.
- **Mutations.** 164 raw `useMutation` calls. `app/mutations.ts:38` `useInstantMutation` (cancel,
  optimistic patch, rollback, toast, invalidate) is the intended single way but has 3 users;
  tasks (`work-items/useTaskSave.ts`, `boardModel.createSaveQueue`) and collections
  (`collections/useRecordEdits.ts`) have their own optimistic machines. `patchQueries`
  (`app/mutations.ts:17`) snapshots and patches by key prefix.
- **Paging.** 7 files use `useInfiniteQuery`; responses use `nextCursor` and `hasMore` in most
  places, other spellings in a few.
- **Session scope.** `api/queryScope.ts` generation counter stops late patches after sign-out.

### 2.4 The realtime client

- One hook, `realtime/useRealtimeConnection.ts` (431 lines), one WebSocket to
  `/api/v1/realtime`. Dispatch for a durable event (`:286-352`): drop repeats (512 recent ids,
  `:76-86`), then:
  - reactions: refetch that conversation's timelines (`:125-133`, `:296-300`);
  - task events: patch lists, boards and detail in place through the lazily loaded
    `realtime/workItemPatches.ts` (`:301-318`), which knows which filters it can evaluate on a
    card and refetches only views using the others (`workItemPatches.ts:48-71`). This is the
    right shape and should be generalised;
  - page, discussion, whiteboard, sheet, collection events: `realtime/liveDocuments.ts:132-185`
    refetches by literal keys;
  - everything else (each message, edit, thread, run and reply-progress event) and every legacy
    event: `invalidateProject` (`:87-104`, called at `:351` and `:369`) invalidates the project
    rail, `['direct']`, `['inbox']` and every query whose key starts `['project', projectId]`.
    Since sheets, collections and pages keep their keys under that prefix, one chat message
    refetches an open sheet, collection, page list, members and settings in that project.
- On connect, close or reconnect, `refetchAuthenticatedState` (`:165-173`, called `:277`, `:377`)
  refetches every project query. The server has no resume: on lag it closes the socket to force
  a full resync (`apps/server/src/realtime.rs:387-393`), and the hub is one process
  (`realtime.rs:131-170`; K5's problem).
- Agent text streams update React state once per animation frame (`:189-201`) in the same context
  value as `connection` (`realtime/realtimeContext.ts:7-20`), so `shell/AppShell.tsx:135`,
  `shell/ProjectRail.tsx:117`, `whiteboards/BoardCanvas.tsx:44` and `memory/live/LivePage.tsx:230`
  re-render on every frame of any agent streaming anywhere.
- No presence or typing indicators exist in the client.

### 2.5 Shared pieces and their twins

| Piece | Generic today | Task or module twin(s) |
| --- | --- | --- |
| Table | kit `ui/Table.tsx:62` (sticky header, keyboard rows, windowing hooks, narrows by its own width `:34-50`); `collections/RecordTable.tsx` (272) on it | `work-items/WorkItemList.tsx` (547) on kit Table; piece table in `modules/pieces/Pieces.tsx:179`; embed tables `collections/CollectionEmbed.tsx`, `sheets/SheetEmbed.tsx`; raw tables in settings and runs |
| List | piece list `Pieces.tsx:157` (only used by `memory/MemoryPages.tsx:225` and page embeds) | inbox, My work, Later, whiteboard grid each draw a `<ul>` |
| Board | `collections/RecordBoard.tsx` (355) | `work-items/WorkItemBoard.tsx` (785), piece board `Pieces.tsx:209` |
| Calendar | `collections/RecordCalendar.tsx` (253) | none |
| Timeline | none generic | `views/TimelinePage.tsx` (434), tasks only |
| Gallery | `collections/RecordGallery.tsx` (100) | whiteboard grid |
| Detail | `collections/RecordPanel.tsx` (297); piece detail `Pieces.tsx:234` | `work-items/TaskDetail.tsx` (620) + `TaskPanel.tsx`; whiteboard panels |
| Form | `forms/Form.tsx` (211, the public form); piece form `Pieces.tsx:257` | task create dialog |
| Filters | `work-items/FieldFilters.tsx` (shared by both) | task `FilterBar.tsx` (648, URL query model) vs `collections/CollectionToolbar.tsx` (218, typed view config) |
| Field display/edit | `fields/FieldValueView.tsx` (56), `fields/FieldEditor.tsx` (352) | piece `fieldText` in `modules/pieces/fields.ts` on the other `FieldType` |
| Windowing, drag, pointer | `work-items/useWindowedRows.ts`, `useBoardDrag.ts`, `useCoarsePointer.ts`, reused by collections and sheets but living in a feature folder | |
| Keyboard lists | `ui/listKeys.ts` | separate j/k handlers in `WorkItemList.tsx:338-342`, `WorkItemBoard.tsx`, `inbox/InboxPage.tsx:370-376` |
| Side panel | none in the kit | Escape handlers drift: `work-items/TaskPanel.tsx:27` excludes `.aw-panel`, `collections/RecordPanel.tsx:113` does not |

Pieces are display only (no windowing, selection, drag or inline edit) and take static manifest
`FieldDecl`s, so they cannot draw a collection's runtime fields.

### 2.6 Bundle and budget

- `scripts/web-budget-lib.mjs:9-91` holds the initial load to 424 KB + 256 raw and
  138 KB − 2,270 gzip (`:48-49`) and initial CSS to 116 KB (`:56`); each lazy chunk to 56 KB raw
  / 18 KB gzip; editor and whiteboard chunks to their own limits. The measurement is careful
  (initial assets from `index.html`, per chunk, hashes normalised). The policy is zero headroom,
  with a changelog of byte negotiations in the comment.
- `main.tsx:10-18` loads every page stylesheet up front, including `conversation.css`,
  `pages.css` and `work.css` (about 74 KB of the 116 KB CSS limit per the health check;
  I did not rebuild to confirm).
- Workarounds for the budget: the Home sidebar loads after first paint (`AppShell.tsx:29`),
  task patches load on the first event (`useRealtimeConnection.ts:312`).

### 2.7 Phone, install, offline

- Phone layout is CSS at `(max-width: 860px)` (`shell/mobileNavigation.ts:4`): the rail and
  sidebar become one left drawer, the context panel a right drawer, swipe to open and close
  (`AppShell.tsx:258-266`), pull to refresh in the installed app.
- `public/sw.js` caches the app shell and hashed assets (never API responses, `:1-5`), shows
  push with Approve, Deny, Reply, Done and Snooze actions answered with a short-lived token
  (`:70-150`), and receives Share-to-Workspace posts (`:194-226`). `public/manifest.webmanifest`
  has shortcuts (New task, Inbox, Messages) and a share target.
- Sending survives a lost connection for messages and task comments only: drafts and an unsent
  queue in `localStorage` per signed-in account (`app/localDrafts.ts`, `app/outbox.ts:24-49`),
  flushed on reconnect by `app/Outbox.tsx` with the original idempotency key.
- No offline reading of content.

### 2.8 Design system

- `ui/index.ts` exports controls, overlays (Dialog traps focus and sets the root `inert`),
  feedback, page parts, identity chips, skeletons, toasts, time and text helpers; `ui/pickers.ts`
  is the lazy half (Popover, Combobox, DatePicker, Table). About 3,460 lines.
- `styles/tokens.css` (289 custom properties) with five built-in presets (`indigo`, `midnight`,
  `daylight`, `evergreen`, `graphite`, at `:5,72,122,140,178`) and a set of light and dark
  community themes loaded on demand (`theme/slots.ts:5-26`, `public/themes/classic.css`).
  `styles/designSystem.test.ts` fails on hand-typed colours, sizes, radii and timings.
- `axe-core` runs in about ten component tests (for example `modules/pieces/Pieces.test.tsx`)
  and in `e2e/pieces.spec.ts`. No contrast check over theme tokens.

### 2.9 Tests

- 119 web test files, about 17,250 lines (H report). Eight `app/App.*.test.tsx` files mount the
  whole app with stubbed `fetch`; the vitest timeout is 20 s because of them
  (`apps/web/vite.config.ts:181`). Fixtures are hand-written JSON, so a server change can leave
  them green.
- Playwright: 11 specs in `apps/web/e2e` (collections, fields, files, move, pages, pieces,
  screenshots, single sign-on, smoke, views, whiteboards) against a real server and database,
  one worker, `retries: 1` in CI (`apps/web/playwright.config.ts:16`), Desktop Chrome only.
  A separate tour config takes screenshots. `.claude/skills/check-in-browser/SKILL.md` explains
  writing a throwaway spec to look at a page.

### 2.10 Account-kind differences found in my area

- `team/TeamSettings.tsx:389`: the Admin role is not offered to a member whose `actorKind` is
  `external_agent`, and `:392` passes `agent` to `CustomRoleOptions`. This is a permission
  difference by account kind drawn in the web. Replacement: the server returns each member's
  `assignableRoles` (K1's role data); the select lists exactly those.
- Settings visibility uses role names (`settings/registry.ts:48-49` `manager`, `owner`), not
  account kind; it should read capabilities from the server (K1), listed here so K1 can pick it
  up.

### 2.11 What has the right shape and stays

`api/client.ts`; TanStack Query as the only store; `queryScope.ts`; the kit and tokens and their
test; `useInstantMutation`'s contract (instant, toast with undo, rollback, no "reload");
`workItemPatches.ts`' "patch what I can evaluate, refetch views I can't" rule; the settings
registry pattern; manifest projections through Vite; the budget's measurement; the outbox's
idempotent flush; the service worker's push actions and share target; `useWindowedRows`,
`useBoardDrag`, kit `Table`; the layout registry and `shows(part)`.

## 3. The target design

### 3.1 Router and shell

**One data router.** `app/routes.tsx` builds route objects; `App.tsx` becomes
`<RouterProvider router={createBrowserRouter(routes)} />`. Each route is
`{ path, lazy, handle }`, where `lazy` returns `{ Component }` (React Router's per-route code
splitting) and `handle` is:

```ts
export type RouteHandle = {
  /** Which sidebar the shell draws. */
  space: 'home' | 'direct' | 'space' | 'none'
  /** The module that owns the route (manifest id), for the gate and the nav highlight. */
  module?: string
  /** The layout part; the shell hides the route's nav entry and shows "turned off" when hidden. */
  part?: PartId
  /** How main is laid out: a page, a conversation (own header), or a full canvas. */
  layout: 'page' | 'conversation' | 'canvas'
  /** Static title for DocumentTitle, when the page has no better one. */
  title?: string
}
```

Alternative considered: keep `<BrowserRouter>` and pass handles through a context. Rejected,
because `useMatches()` and `route.lazy` give exactly the shell-from-route and per-route loading
we need, and React Router 7 is already installed.

**One shell.** Route tree:

```
/                      → redirect to /w/<current>/home
/sign-in, /forgot-password, /reset-password, /verify-email, /join, /forms/:token   (public)
SessionBoundary (exists)
  /settings/account/*            Account settings (the person is global to the server)
  /dm/:conversationId            resolves to /w/<last used>/dm/:conversationId (R20)
  /w/:ws                          WorkspaceBoundary: resolves slug, provides WorkspaceContext
    ShellLayout                   AppShell mounted once; reads useMatches() handles
      ...module routes (3.2)
  legacy routes                   redirects (3.3)
  /:taskKey                       TaskKeyRedirect, now into /w/... (kept)
  *                               NotFound inside a shell for the current workspace
```

`ShellLayout` renders `AppShell` once with `<Outlet/>` inside `main`. Pages talk to the shell
through two hooks instead of props:

```ts
/** Put something in the details panel while this component is mounted. */
function useShellPanel(panel: ReactNode | null, options?: { key?: string; onDismiss?: () => void }): void
/** Tell the shell what the current space is (the project once loaded) and what is selected. */
function useShellSpace(state: { spaceId?: string; selectedChannelId?: string; thread?: { id: string; name: string } | null }): void
```

Both write to a small store (`shell/shellStore.ts`, `useSyncExternalStore`), not React context
value churn, so a page update doesn't re-render the sidebar. The sidebar state workarounds
(`SidebarParts.tsx:17-24` maps, `MobileNavigationProvider`, `useAddressProject`, the fallback's
pathname guessing, `homeSpace.ts`'s path list) become unnecessary and are deleted in W1.12.

During migration `AppShell` checks a `ShellPresence` context: inside `ShellLayout` it renders
only its children and forwards `context` and friends to the two hooks. So a page that still
renders `<AppShell context={…}>` works unchanged inside the persistent shell, and pages move
off it one folder at a time.

`AuthenticatedRouteBoundary` (`App.tsx:402-412`) becomes the route's `errorElement` plus a
`Suspense` inside `ShellLayout`'s `main`, so the loading frame is the real shell.

**Static files (R24.1).** The web app is static files any CDN can serve: no server-rendered
page and no value written into `index.html` by the server. At start the app fetches its
configuration (`GET /client-config`: API origin, live origin, feature switches, sign-in
methods) and caches it with the shell. The server may still serve the files for a self-hoster.
The API origin may differ from the app's origin (CORS and cookie rules are H1's and K5's).

**Switcher (R19, R20).** The workspace switcher lists every workspace the account is in, each
with an unread mark from the live connection's counts for workspaces other than the current
one. It never lists the direct scope; DMs show in every workspace's sidebar.

### 3.2 Route table

`:ws` is the workspace slug. `:space` is a space reference: K4 decides the canonical form (I
assume the space key, such as `awd`, with the id accepted too and redirected to the key).
Module paths use the user-facing words; old words redirect.

| Address | Page | Handle (space, module, part, layout) |
| --- | --- | --- |
| `/w/:ws` | redirect to `home` | |
| `/w/:ws/home` | Home | home, workspace, -, page |
| `/w/:ws/welcome` | First run | none, workspace, -, page |
| `/w/:ws/inbox` | Inbox | home, inbox, -, page |
| `/w/:ws/later` | Later | home, inbox, workspace.later, page |
| `/w/:ws/my-work` | My work | home, tasks, workspace.my_work, page |
| `/w/:ws/runs`, `/w/:ws/runs/:runId` | Runs, run | home, runs, workspace.runs, page |
| `/w/:ws/pages`, `/w/:ws/pages/:pageId` | Workspace pages | home, pages, workspace.pages, page |
| `/w/:ws/whiteboards`, `/w/:ws/whiteboards/:id` | Boards, board | home, whiteboards, workspace.whiteboards, page / canvas |
| `/w/:ws/messages`, `/messages/agents`, `/messages/:conversationId` | DMs and groups | direct, messages, -, page / conversation |
| `/w/:ws/dm/:conversationId`, and `/dm/:conversationId` (outside `/w/`, resolves to the last-used workspace's `/w/<slug>/dm/<id>`) | A DM or group from the direct scope (R20), drawn inside the current workspace's shell; the same list in every workspace | direct, messages, -, conversation |
| `/w/:ws/search` | Search | none, workspace, -, page |
| `/w/:ws/spaces`, `/w/:ws/spaces/new` | All spaces, new space | home, projects, -, page |
| `/w/:ws/new`, `/w/:ws/share` | Quick capture, share in | none, -, -, page |
| `/w/:ws/settings/*` | Workspace and space settings | none, workspace, -, page |
| `/w/:ws/s/:space` | Space overview | space, projects, -, page |
| `/w/:ws/s/:space/chat/:channelId` | Channel | space, channels, area.chat, conversation |
| `/w/:ws/s/:space/threads/:threadId` | Thread | space, channels, area.chat, conversation |
| `/w/:ws/s/:space/chat/archived` | Archived conversations | space, channels, area.chat, page |
| `/w/:ws/s/:space/tasks` (`?view=`) | Tasks list and board | space, tasks, area.tasks, page |
| `/w/:ws/s/:space/tasks/:taskRef` | Task (key such as `AWD-12`, or id) | space, tasks, area.tasks, page |
| `/w/:ws/s/:space/timeline`, `/milestones` | Timeline, milestones | space, tasks, area.timeline / area.milestones, page |
| `/w/:ws/s/:space/planning/:tab` | Next, suggestions, graph, import | space, suggestions, area.planning, page |
| `/w/:ws/s/:space/runs` | The space's runs | space, runs, area.runs, page |
| `/w/:ws/s/:space/pages`, `/pages/:pageId`, `/decisions` | Pages, decisions | space, pages, area.pages / area.decisions, page |
| `/w/:ws/s/:space/collections/*` | Collections | space, collections, area.collections, page |
| `/w/:ws/s/:space/sheets/*` | Sheets | space, sheets, area.sheets, page |
| `/w/:ws/s/:space/whiteboards/:id` | A board in the space | space, whiteboards, area.whiteboards, canvas |
| `/w/:ws/s/:space/m/:module/*` | A connected or built module's panels (M1) | space, from data, from data, page |
| `/w/:ws/m/:module/*` | Workspace-level panels of such a module | home, from data, from data, page |
| `/w/:ws/t/:kind/:id` | Any thing by kind and id (K3): resolves to its canonical address and replaces | none |
| `/design-system`, `/design-system/pieces` | Kit and piece gallery | none |

Workspace-level words (`home`, `inbox`, `s`, `m`, `t`, `settings`, …) are reserved; K4 refuses
them as space keys.

**One address scheme for things.** Every thing has a canonical href from its kind's
registration (K3 returns `href` on the thing envelope; the web module registration has the same
template for building links without a round trip). `/w/:ws/t/:kind/:id` is the permanent form
for links that must survive a rename or a move between spaces; it asks K3's `thing_get` (or its
`resolve` operation) and replaces the address. Inside the app, links are built by one helper:

```ts
// apps/web/src/app/href.ts
export function hrefFor(ws: string, thing: { kind: string; id: string; spaceRef?: string; key?: string }): string
export function spaceHref(ws: string, spaceRef: string, path?: string): string
export function workspaceHref(ws: string, path?: string): string
```

**Lazy loading per module.** Each module's routes are `lazy`; the registration file (3.4) holds
only paths, handles, labels and `() => import(...)` functions, so the initial load carries a few
hundred bytes per module. Feature stylesheets move from `main.tsx:14-16` into the routes that use
them.

### 3.3 Old addresses

A `legacyRoutes` list maps every current path to its new one, with the workspace resolved as:
the workspace of the project in the path (from the projects list, which already carries
`organizationId`), else the last workspace used in this browser (`localStorage`), else the
session's default (`useHomeWorkspaceId` logic). Mapping:

| Old | New |
| --- | --- |
| `/home`, `/inbox`, `/later`, `/my-work`, `/runs[/:id]`, `/whiteboards[/:id]`, `/pages`, `/messages/...`, `/search`, `/welcome` | `/w/<ws>/<same>` |
| `/projects` , `/projects/new` | `/w/<ws>/spaces`, `/w/<ws>/spaces/new` |
| `/projects/:id` | `/w/<ws>/s/<ref>` |
| `/projects/:id/work-items[/:wid]` | `.../s/<ref>/tasks[/<key or id>]` |
| `/projects/:id/launches` | `.../s/<ref>/runs` |
| `/projects/:id/channels/:cid` | `.../s/<ref>/chat/:cid` |
| `/projects/:id/conversations/archived` | `.../s/<ref>/chat/archived` |
| `/projects/:id/{threads,pages,decisions,collections,sheets,timeline,milestones}/...` | same under `.../s/<ref>/` |
| `/projects/:id/{next,suggestions,graph,plan-import}` | `.../s/<ref>/planning/{next,suggestions,graph,import}` |
| `/projects/:id/settings` | `/w/<ws>/settings/spaces/<ref>` (already a redirect today, `App.tsx:448-453`) |
| `/settings/workspace/...`, `/settings/projects/...` | `/w/<ws>/settings/...` |
| `/new-task`, `/share` | `/w/<ws>/new`, `/w/<ws>/share` (query string kept) |
| `/:taskKey` | kept, resolves into `/w/...` |

Redirects stay for at least one release after the server and push links move (K4), and the
redirect table has a test that every old route in today's `App.tsx` has an entry. Links already
in people's messages and pages keep working because the redirect is client side; K3/K4's
mention regex has to accept both forms (touch point).

### 3.4 Shell drawn from module registrations

Each module gets one web registration file. M1 owns the registration contract and its
collection (M1.4, M1.5): `apps/web/src/<folder>/module.ts` exporting `defineModule({...})` from
`modules/define.ts`, collected by `modules/registry.ts`. W1 does not define a second one; the
type below is the set of fields the shell needs, which M1's `defineModule` carries (M1 section
3.4 lists them: route handles with space and layout, `phone` and `badge` on navigation,
`things`, `capture`, `live` handlers by event kind). No shared list to edit, so adding a module
touches no core file.

```ts
// fields W1's shell reads from M1's defineModule (apps/web/src/modules/define.ts, M1.4)
export type WebModule = {
  id: string                                   // manifest id; checked against the manifest
  routes: Array<{ path: string; scope: 'workspace' | 'space'; lazy: () => Promise<{ Component: ComponentType }>; handle: Omit<RouteHandle, 'module'> }>
  nav?: Array<{
    scope: 'home' | 'space'
    part?: PartId
    label: string | ((words: Words) => string)  // layouts rename nouns
    icon: IconName
    to: string                                    // relative to the workspace or space
    order: number
    badge?: () => Promise<{ default: () => number | null }>   // lazy hook, for unread counts
    phone?: 'tab' | 'drawer'                     // where it goes on a phone (3.9)
  }>
  palette?: Array<{ id: string; label: string; keywords: string[]; to?: string; run?: () => Promise<void> }>
  shortcuts?: Array<{ keys: string; to: string }>
  settings?: SettingsPage[]                       // same shape as settings/registry.ts
  live?: () => Promise<{ default: LiveHandlers }> // event handlers (3.6), loaded on first event of the module's kinds
  liveKinds?: string[]                            // event kinds the handlers above take
  things?: Record<string, { href: string; detail?: () => Promise<{ default: ComponentType<ThingDetailProps> }>; chip?: ChipSpec }>
  embeds?: Array<{ id: string; match: string; piece?: PieceSpec; component?: () => Promise<{ default: ComponentType<EmbedProps> }> }>
  capture?: Array<{ id: string; label: string; form: () => Promise<{ default: ComponentType<CaptureProps> }> }>
}
// no defineWebModule: modules call M1's defineModule
```

The manifest stays the source of truth for what a module claims (slots); the web registration
supplies how it draws. `slots.test.ts` keeps checking every route, row and palette entry is
claimed, now by reading registrations instead of `App.tsx`. M1 owns the manifest and the module
registry on the server; for connected and built modules (no code), the shell reads the same
shape from M1's `module_list` result at runtime: nav entries and routes under `m/:module/*`
whose pages are drawn by the panel block renderer (3.7). Turned-off modules are dropped by the
same `hidden` list the layouts use.

The shell's sidebar, palette, shortcut sheet, settings nav and route table are then built from
the registrations; `shell/ProjectSidebar.tsx`, `HomeSidebar.tsx` and `CommandPalette.tsx` lose
their hand-written rows. `experience/parts.ts` is generated from `registry.json` by the same
Vite plugin (a `virtual:experience/parts` module with `export type PartId = …`), so its mirror
and its test go.

### 3.5 Data layer

**Generated client (K2).** K2 generates `apps/web/src/api/generated/` (types for every
operation's input and result, the error list, the event payloads, and a client). I assume each
operation is exported as a descriptor:

```ts
// generated (K2)
export const task_update: Op<'task_update', TaskUpdateInput, TaskUpdateResult> = {
  name: 'task_update', kind: 'change', method: 'PATCH', path: '/api/v2/w/{workspace}/tasks/{id}',
  thing: 'task', queueable: true,
}
```

and an `Envelope<T>` result (`{ outcome: 'ok', value: T } | { outcome: 'error', error: ApiErrorBody }`)
plus a page shape for lists (`{ items: T[], nextCursor: string | null }`). W1 does not generate;
it wraps:

```ts
// apps/web/src/api/ops.ts (W1)
export function callOp<I, R>(op: Op<string, I, R>, input: I, options?: { requestId?: string; expectedVersion?: number; signal?: AbortSignal }): Promise<R>
export function opKey<I>(op: Op<string, I, unknown>, input: I): QueryKey   // ['op', workspaceId, op.name, stableInput]
export function opQuery<I, R>(op: Op<string, I, R>, input: I): UseQueryOptions<R>   // queryOptions() with opKey and normalisation
export function opList<I, R>(op: Op<string, I, Page<R>>, input: I): UseInfiniteQueryOptions<Page<R>>
export function useOp<I, R>(op: Op<string, I, R>, options: OpMutationOptions<I, R>): OpMutation<I, R>
```

`callOp` goes through `requestJson` (CSRF, idempotency key, `ApiError`). The request id is a
UUIDv7 made by the caller once per user intent and reused on every retry (it is the existing
`idempotency-key` header until K2 names it). `expectedVersion` is read from the thing store when
the caller doesn't pass it. Errors map from K2's error list to `ApiError.code`, and
`ui/errorText.ts` `describeError` gets one sentence per code, generated alongside the list, so
the polish checklist's "say the real cause" is automatic.

**Keys.** Every op query key is `['op', workspaceId, opName, input]`. No feature writes keys.
The workspace id is in every key, so two workspaces can be cached side by side and switching
doesn't show the wrong one's data.

**The thing store (normalised cache).** Choice: keep TanStack Query for fetching, paging,
retries and suspense, and add a small entity store beside it rather than adopt a sync engine.
A sync engine (local-first database in the browser) was the alternative; I rejected it for now
because it would replace the whole data layer at once, add a large dependency to a full budget,
and duplicate K3/K5's log-and-cursor design on the client. The store:

```ts
// apps/web/src/data/things.ts (W1)
export type ThingRef = { kind: string; id: string }
export type Stored<T> = { base: T & { version: number }; overlays: Array<{ requestId: string; patch: Partial<T> }> }

export const thingStore: {
  get<T>(ref: ThingRef): T | undefined                       // base with overlays applied in order
  put<T extends { kind: string; id: string; version: number }>(thing: T): void   // ignores an older version
  patch(ref: ThingRef, version: number, fields: Record<string, unknown>): 'applied' | 'stale' | 'unknown'
  overlay(ref: ThingRef, requestId: string, patch: object): () => void          // returns rollback
  settle(requestId: string): void                             // drops the overlay once the server's answer is in base
  remove(ref: ThingRef): void
  subscribe(ref: ThingRef, listener: () => void): () => void
}
export function useThing<T>(ref: ThingRef | null): T | undefined           // useSyncExternalStore
```

Op query functions normalise: every value in a result that has `kind`, `id` and `version`
(K3's thing envelope) is `put` into the store, and the cached query data keeps refs where the
op is declared as returning things. List components read rows with `useThing`, so a row
re-renders alone when its thing changes and a thing shown in a list, a board, a detail panel and
an embed is stored once.

**Lists with paging.** One pattern: `opList` + `useThingList(op, input)` returning
`{ refs, fetchNextPage, hasNextPage, total?, status }`. Each list registers its *membership
rule* with the store: a function `(thing) => 'in' | 'out' | 'unknown'` derived from the view's
filter where the client can evaluate it (the `workItemPatches.ts:48-71` rule generalised over
field clauses from T1's filter grammar), plus a sort comparator. When a thing changes, lists
whose rule says `in` or `out` insert, move or drop the ref in place; lists that say `unknown`
for that change are marked stale and refetched (only those lists, only the first page, then the
pages the person has scrolled to).

**Optimistic writes with rollback.**

```ts
const update = useOp(ops.task_update, {
  optimistic: (input) => ({ ref: { kind: 'task', id: input.id }, patch: input.changes }),
  success: (result, input) => ({ title: 'Task updated', onUndo: ... }),
  errorTitle: 'Couldn’t update the task',
})
update.mutate({ id, changes: { status: 'done' } })   // request id and expected version supplied
```

On `mutate`: make the request id, read the thing's version, add an overlay keyed by request id
(the screen changes at once), send. On success: `put` the returned thing (newer version), then
`settle(requestId)`. On error: drop the overlay (rollback) and toast `describeError`. On
a version conflict (K2's outcome `conflict` with `code: "version"`): drop the overlay, refetch that one thing, and say who
changed it if the error carries the actor; offer "Apply my change again" which retries with the
new version and a new request id. When the server's own event for this write arrives (it carries
the `request_id`, K3), the store sees it is already applied and does nothing, so there is no
flicker. Overlays survive incoming events for the same thing because they sit above base, which
fixes today's race where an invalidation overwrites an optimistic patch.

`useInstantMutation` keeps its signature and becomes a thin wrapper over `useOp` for ops; for
legacy endpoints it keeps today's behaviour.

**Old and new side by side.** The old `requestJson` + hand types + keys keep working. A module
moves by: replacing its `api.ts` functions with op calls, deleting its hand types, its key
factory and its normalisers, and adding its live handlers. The order is in W1.9. During the
move, legacy keys that still need refreshing after an op write are named in the op mutation's
`alsoInvalidate`, so mixed screens stay right.

### 3.6 Precise live updates

**Wire (K5's protocol 2, `GET /api/v1/realtime?v=2`, K5.9 and K5.10).** The browser keeps one
WebSocket (R15; agents use server-sent events and polling from the same cursor) to the live
role (R24.5; the frame host page `/panel-frame/{launch}` is served by `api`, not the static app, R31): it first asks `api` for a short-lived signed ticket naming the audiences it may
hear (the current workspace, the direct scope for its DMs, and unread counts for its other
workspaces, R20), connects to the live origin from the client config with that ticket, and on
reconnect gets a fresh ticket and resumes from its cursor. The server, not the client, decides
the audiences from the account's memberships (K3's envelope); the client does not subscribe to
scopes. It resumes from one cursor and tells the
server only which documents it has open and when the person is typing.

```jsonc
// client → server
{ "type": "hello", "resumeFrom": 18234 }                       // one cursor for the connection
{ "type": "watch", "document": { "kind": "page", "id": "…" } }   // open editor: document updates and awareness
{ "type": "unwatch", "document": { "kind": "page", "id": "…" } }
{ "type": "typing", "conversationId": "…" }
{ "type": "awareness", "document": {…}, "state": {…} }          // cursors and selection in an open editor (C1)

// server → client
{ "type": "ready", "resumed": true }
{ "type": "event", "position": 18240,
  "event": { "id": "…", "kind": "task.updated", "thing": { "kind": "task", "id": "…", "version": 14 },
             "spaceId": "…", "conversationId": null, "actorId": "…", "requestId": "…",
             "occurredAt": "2026-11-14T09:12:00Z",
             "changes": { "status": "done", "assigneeId": "…" } } }
{ "type": "access" }                                            // memberships changed; the server recomputed the audience
{ "type": "resync" }                                            // cannot replay from that cursor; refetch active queries
{ "type": "typing", "conversationId": "…", "accountId": "…" }   // ephemeral, not in the log
{ "type": "presence", "online": ["…"] }                          // ephemeral; K5's realtime_connections
{ "type": "stream.delta", "conversationId": "…", "messageId": "…", "runId": "…", "delta": "…" }   // ephemeral
{ "type": "awareness", "document": {…}, "accountId": "…", "state": {…} }
{ "type": "document.update", "document": {…}, "update": "…" }
{ "type": "reconnect" }                                         // server draining; reconnect with the cursor
```

**Who listens.** `ShellLayout` owns the one connection and the cursor. Pages don't subscribe;
they ask for the events they care about with `useLiveKinds(kinds, handler)`, and an open editor
sends `watch` for its document (reference counted, `unwatch` a few seconds after the last user
unmounts, so moving between pages doesn't churn).

**Applying an event.** A dispatcher (`realtime/live/dispatch.ts`) runs, in order: drop repeats
(by event id, as today); record the connection's cursor (`position`); if `requestId` matches a settled
local write, stop; call the module's handler for `event.kind` if registered (loaded lazily from
the registration's `live`), else the default handler:

1. if `thing` is in the store and `changes` is present: `thingStore.patch(ref, version,
   changes)`; if it returns `stale` (a version was skipped), fetch that one thing with
   `thing_get`;
2. if `thing` is in the store and `changes` is absent: fetch that one thing;
3. run list membership (3.5) for lists holding that kind;
4. invalidate only queries the module declared as derived from that thing (for example a
   task's history when a task changes), by op name and the thing's id.

Module handlers cover what the default can't: a new message appends to the conversation's
timeline pages and bumps that conversation's unread count and the sidebar row; a removed message
scrubs every copy synchronously before anything else (today's `dropRemovedMessage` rule, which
must be kept); a page or board event tells the open editor (the existing listeners in
`liveDocuments.ts`). Nothing ever invalidates by a space or project prefix.

**Gaps and reconnects.** On reconnect the client sends `hello` with its last cursor. If the
server replays (`ready{resumed:true}`), events apply as normal. If it answers `resync` (cursor
older than retention, or a lagged connection), the client refetches only the active queries:
the mounted lists and things, and each open conversation's first timeline page. Older timeline
pages kept outside the cache refetch through the existing `resyncGeneration` signal. On
`access`, lists whose membership can't be told from an event refetch themselves (the accepted
list-refetch exception), never a whole space. The connection banner (`AppShell.tsx:337-341`) stays.

**Streams, presence and typing.** Ephemeral frames go to a separate store
(`realtime/live/ephemeral.ts`) read with `useSyncExternalStore` per conversation:
`useStream(conversationId)`, `usePresence(scope)`, `useTyping(conversationId)`. The realtime
context keeps only `connection` and is stable, so the shell and editors no longer re-render on
stream frames. Typing is sent at most every 3 s while typing and cleared on send or blur;
presence (who is online, from K5's `presence` frame; who is in an open document, from K5's
`awareness` frame) is shown as avatars by C2 and C1.
Agents show typing the same way when their tools report it (A1/A2's "working" line).

**Test harness.** `realtime/live/replay.ts` replays a recorded stream into a dispatcher wired to
a real `QueryClient` and thing store with a stub transport and a counting `fetch`:

```ts
const run = await replay('fixtures/chat-burst.jsonl', { seed: 'fixtures/chat-burst.seed.json' })
expect(run.fetches).toEqual([])                         // a message in a busy space fetched nothing
expect(run.thing({ kind: 'task', id: T1 }).status).toBe('done')
expect(run.invalidated).not.toContainPrefix(['op', WS])  // nothing broad
```

Fixtures are JSON Lines of server frames, type-checked against K3's generated event types. A
Playwright helper (`e2e/support/recordFrames.ts`) records frames from the real server during a
spec, so fixtures are real wire data, re-recorded with an environment flag like the existing
parity fixtures (`UPDATE_DOC_PARITY=1`).

Until K5 ships protocol 2, W1.3 applies the same "only what the event names" rule to today's
envelope (it has `projectId`, `conversationId`, `type` and a payload with ids), which already
removes the whole-project refetch.

### 3.7 Shared pieces

**Split with T1 (R17).** T1 owns everything that knows fields and views: view config (type,
filters, sort, group, columns, card fields, date fields, chart config), the filter grammar and
its filter editor, field definitions, every field type's value shape and the field registry
(renderers and editors by field type), saved and personal views, the timeline and chart
renderers, and the adapters that turn a view config into a piece's props. W1 owns the generic
React components (list, table, board, calendar, gallery, panel, form, detail, chips, the
discussion panel shell, the history panel, the request card, the run view, the block renderer),
in `apps/web/src/pieces/` (moved from `modules/pieces/`), their interaction (selection,
keyboard, drag, inline edit, windowing) and their narrow behaviour. A W1 piece knows columns,
groups and dates as callbacks, never fields. A
piece never fetches by itself; it takes a list source and calls back with intents. A module
supplies extras through slots, never by forking a piece. C1 owns the discussion panel's
contents and editors; A1 owns run and request data; M1 owns the block format.

Common props:

```ts
export type ListSource = ReturnType<typeof useThingList>            // refs, paging, status
export type PieceCommon = {
  source: ListSource
  columns: PieceColumn[]              // built by T1's view adapter from fields and the view config
  label: string                       // accessible name
  selection?: SelectionModel          // shared: single, multi, range, select all matching
  onOpen?: (ref: ThingRef) => void
  onChange?: (ref: ThingRef, columnId: string, value: unknown) => void   // caller wires useOp
  onLayoutChange?: (next: PieceLayout) => void                    // column widths, sort, group; T1 maps back to ViewConfig
  capabilities?: { edit?: boolean; create?: boolean; reorder?: boolean }
  slots?: PieceSlots                  // rowActions, bulkBar, groupHeader, cardFooter, empty, createRow
}
export type PieceColumn = {
  id: string; label: string; width?: number
  render: (ref: ThingRef) => ReactNode                       // T1's field renderer
  editor?: (ref: ThingRef, done: () => void) => ReactNode     // T1's field editor
  sortable?: boolean
}
```

Board takes `groups: PieceGroup[]` and `groupOf(ref)`; calendar takes `dateOf(ref)` (and
`endOf?`); the adapters that compute them from a view config live in T1 (T1.13), so a piece
never imports `FieldDefinition` or `ViewConfig`.

| Piece | Today, grown from | What to generalise | Own props | Wide | Narrow (under 600 px of its own width) |
| --- | --- | --- | --- | --- | --- |
| Table | kit `Table` + `RecordTable` + `WorkItemList` | columns from `FieldDefinition`s, inline edit cells by field type, column resize and hide, sticky first column, group rows, row selection, windowing to 10,000+ rows, keyboard grid | `columns?: string[]`, `rowHeight: 'compact' \| 'default' \| 'roomy'` | grid with sticky header | a card per row with labelled values; selection by long press |
| List | piece list + `WorkItemList` rows + inbox, My work, Later | grouping, done tucking as a slot (`groupFooter`), j/k from `ui/listKeys.ts`, windowing | `primary`, `secondary`, `meta` field ids | one line per thing | wraps to two lines |
| Board | `RecordBoard` + `useBoardDrag` | columns from any single-choice or status field, per-column paging, drag between and within columns calling `onMove`, column composer slot, card fields | `columnsBy: string`, `onMove(ref, to, before)` | columns side by side | one column at a time with a column switcher, swipe between, drag by long press |
| Calendar | `RecordCalendar` | date or date-range field, month and week, drag to reschedule | `dateField`, `endField?` | month grid | agenda list by day |
| Timeline (T1's, R17: T1.14) | `views/TimelinePage` | start and end fields, dependency lines as a slot, zoom; built by T1 on W1's grid and drag primitives | `startField`, `endField`, `dependencies?` | bars over weeks | agenda list with date ranges |
| Gallery | `RecordGallery`, whiteboard grid | cover image or preview slot, card fields | `cover?` | cards in a grid | one column of cards |
| Chart (T1's, R17: T1.31 `ChartView`) | none (T1's Dashboards) | bar, line, area, pie, number, from T1's aggregate op result; drawn in SVG with kit tokens; no chart library in the initial load; W1 holds it to the budget and the narrow rules | `chart: ChartConfig`, `data: AggregateResult` | as configured | full width, legend below, number tiles stack |
| Detail | `RecordPanel` + `TaskDetail` | title, status, fields, body slot, tabs for discussion, history, links, files, runs; works as a page or a side panel | `ref`, `sections: DetailSection[]` | side panel or page | full screen sheet with a back button |
| Form | `forms/Form.tsx` + piece form | fields from `FieldDefinition`s, required and hints, validation from T1's rules, file fields, public and in-app use | `form: FormConfig`, `onSubmit` | one column, max reading width | the same |
| Filter builder (T1's, R17: T1.13 `FilterEditor`) | `FieldFilters` + `FilterBar` + `CollectionToolbar` | one builder over T1's grammar with chips; URL sync adapter for shareable views | `value: FilterClause[]`, `onChange` | toolbar chips | a sheet |
| Field renderers and editors (T1's, R17: T1.13a) | `fields/FieldValueView`, `FieldEditor`, piece `fieldText` | one registry keyed by field type: renderer, editor, filter operators, sort, cell width; an unknown type falls back to the server's formatted text | `registerFieldType(type, spec)` | inline | inline; editors open as sheets |
| Thing and record chips | `ui/identity` chips, `ReferencedIn` | one `ThingChip` (icon by kind, title, key, status) from the thing store, hover card on wide | `ref` | chip with hover card | chip; tap opens |
| Discussion panel | `TaskActivity` (composer with mentions), `PageDiscussion` (plain textarea) | a `DiscussionPanel` shell (tab, count, unread dot, narrow sheet) hosting C1's `ThingComments({thing, anchor?, threadId?})`, so every detail view gets mentions and files | `thing: ThingRef`, `anchor?`, `threadId?` | tab in detail | tab in detail sheet |
| History panel | task timeline renderer, `mcp/history` equivalents | one panel over K3's history op: who, what, through which app, requested by; restore where the kind supports it | `thing: ThingRef` | tab in detail | the same |
| Request card | inbox approval rows, push actions | one card for approvals, questions and proposal acceptance (A1's request record) with answer buttons and a reply field; answers through `request_answer {requestId, optionId?, text?, expectedVersion}` | `card: RequestCard` (A1's `requests::card` result) | card | full width, buttons 44 px |
| Run view | `runs/RunPage` (A1.5), thread run cards | header (who, host, state, cost), activities list (AG-UI names), requests inline, results | `run: RunRef`, `compact?` | page or panel | page |
| Panel blocks | none | renders M1's block JSON (M1's set: text, fields, form, list, table, buttons, status, chart, section; chart drawn by T1's `ChartView`; further kinds only when M1 adds them to its format) with kit parts; buttons call the module's operations through the op client | `blocks: Block[]`, `onAction` | as blocks | each block narrows itself |

**Replacing the task twins without losing anything.** Before a task screen moves, its slice
writes the parity list of what the twin does and the piece must do through slots, checked by an
e2e. For tasks: keys and type glyphs, done tucking and "show done", bulk bar (status, assignee,
labels, milestone, move, archive, launch), claim chips, blocked and ready markers, subtasks
count, quick create row, j/k and the task shortcuts (`work-items/shortcutKeys.ts`), board per-
column paging, workflow transition checks on drag (server refuses; card snaps back with the
reason), column composer, saved views and pins, `?view=` URLs, the task panel beside the list,
launch to an agent, open in a local app (the deep link moves to data, see 3.11). The task list
and board move only after T1 lands tasks as records with typed view configs; until then the
pieces serve collections, embeds and other lists.

### 3.8 Design system additions

`ui/` gains, in the lazy half unless the shell needs them:

- `SidePanel` (title, close, Escape handling with one shared "focus belongs elsewhere" rule,
  resize edge, becomes a sheet on narrow) replacing three hand-built panels.
- `Sheet` (bottom sheet for phones, with drag handle and focus trap).
- Grid primitives for dense data: roving tab index per the WAI-ARIA grid pattern, cell focus
  ring tokens, selection checkbox column, drag handle, column resize handle, sticky column
  shadow, density tokens (`--row-compact: 28px`, `--row-default: 36px`, `--row-roomy: 44px`;
  roomy is the floor on a coarse pointer).
- `HoverCard`, `Chip` variants for thing kinds, `KeyHint`.
- Block primitives for panel blocks: `Progress`, `Stat`, `CodeBlock`, `ImageBlock` (with
  alt text required).
- Moving `useWindowedRows`, `useBoardDrag`, `useCoarsePointer` and `listKeys` into `ui/`.

**Theming.** Keep tokens and presets. Every preset and community theme declares
`color-scheme: light | dark`; the app follows the system by default and the person's choice
otherwise (exists in `theme/`). A contrast test computes WCAG ratios for each theme's
text-on-surface token pairs (body text, muted text, chip text, focus ring against background)
and fails under 4.5:1 for text and 3:1 for focus rings and borders that carry meaning.

**Accessibility as a gate.** For every piece: axe in its component test (exists for pieces),
keyboard path tested (Tab into the grid, arrows, Enter opens, Escape closes, focus returns),
names on every icon button (already linted by the polish list, now tested by axe), live regions
for counts and toasts. For routes: an e2e that opens each registered route at 1440 and 390 px in
one light and one dark theme and fails on axe violations of serious or critical impact.

### 3.9 Phone and install

- **Narrow is a first-class target.** Every piece has the narrow behaviour in the table above,
  decided by its own width (container queries and the kit `Table`'s `ResizeObserver` rule), so a
  piece in a side panel on a laptop narrows like on a phone.
- **Navigation on a phone.** A bottom bar with up to five entries drawn from registrations with
  `phone: 'tab'` (default: Home, Inbox, Messages, Search, You; the current space is reached from
  Home and the drawer), the drawer kept for switching spaces and workspaces. Entries of modules
  that are off drop out; with only Chat on (R22), Home is the conversation list (C2). The detail view opens as a full-screen sheet with a back
  button that is real history (so the hardware back button works). C2 owns what chat and inbox
  look like on a phone; W1 owns the frame.
- **Installable app.** Keep `sw.js` and the manifest. `start_url` becomes `/` (which redirects to
  the current workspace); shortcuts point at `/w/<current>/...` through a small redirect route;
  `APP_ROUTE` adds `w` and `t`.
- **Push.** Keep the service worker's actions. Push `url`s become `/w/...` (server side, K4/C2);
  the worker accepts both. A1's request card answers from the lock screen through the same
  token flow (`sw.js:70-150`), for every request kind.
- **Outbound queue.** Generalise `app/outbox.ts` to any op K2 marks `queueable`: an IndexedDB
  queue per account (`aw-outbox-<accountId>`), each entry `{ requestId, op, input,
  expectedVersion, thing, createdAt, attempts, state }`, sent in order per thing, oldest first,
  on reconnect, on `online`, and on app start. The overlay for a queued write stays on screen,
  marked "Waiting to send" by a small `PendingMark`; a refusal (not a connection error) keeps the
  entry as "Not sent" with the reason and actions (retry with current version, discard, copy).
  The existing message and comment queue moves into it without changing what a person sees.
  Sign-out clears it for that account (today's rule).
- **Offline reading.** The thing store and the first page of recently opened lists and
  conversations are persisted to IndexedDB per account (cap: 500 things, 30 conversations with
  their last 100 messages, 20 lists), restored at start before the network answers, cleared at
  sign-out and on an `authentication_required` response. The service worker still never caches
  API responses. This changes a promise written in `sw.js:1-5`; the accepted end-state change
  settles it: an opt-in per person, which a workspace can switch off, cleared at sign-out
  (W1.31).
- **Share in and quick capture.** `/w/:ws/share` and `/w/:ws/new` show the registrations'
  `capture` entries (a task, a page, a message to a conversation, a record in a collection that
  has a form), not just a task.
- **Targets on a mid-range phone** (Chrome DevTools "mid-tier mobile": 4x CPU slowdown, Fast
  4G, 390×844):
  - cold start, nothing cached, to the shell with skeletons: under 2.5 s; to Inbox content:
    under 3.5 s;
  - warm start (service worker cache, offline store) to content: under 1.2 s;
  - route change inside the shell to skeleton: under 100 ms; to content: under 800 ms;
  - an incoming event applied to the screen: under 200 ms after it arrives;
  - typing in the composer: no input delay over 50 ms (Interaction to Next Paint under
    200 ms at the 75th percentile);
  - scrolling a 10,000-row table: no long task over 50 ms after the first frame.

### 3.10 Speed

- Route-level splitting through `route.lazy`; registrations hold no component code (guard
  10.6).
- Initial load diet: feature CSS out of `main.tsx` into routes; `conversations/liveMessages.ts`
  out of the initial chunk (it becomes the chat module's lazy live handler); `App.tsx`'s repeated
  JSX replaced by data. That frees room the shell needs for the data router and the thing store.
- Budget policy: keep the measurement; set each limit 10% above the measured size at the time of
  the change and have CI print the delta of every limit on every pull request
  (`scripts/check-web-budgets.mjs` already measures; add a "base vs head" table). Fail only when
  a limit is crossed. Move the changelog out of the limits comment into the git log.
- Virtualisation: the table, list, board column and gallery window their rows with
  `useWindowedRows` (fixed heights per density, measured heights for wrapped narrow rows).
- Perceived speed: skeletons from the shell immediately; data from the thing store before the
  network; prefetch a route's chunk and first query on hover and focus of nav entries and links
  (`route.lazy` is idempotent); optimistic writes everywhere through `useOp`.
- Measured in CI (3.12, `perf.spec.ts`), plus the app emits `performance.mark('aw:route-ready')`
  when a route's first query has data, which both the e2e and telemetry read.

### 3.11 Brand hard-coding in the web app

`work-items/codexDeepLink.ts` and its use at `work-items/TaskDetail.tsx:22,265` build one
vendor's deep link on every task. Under the pieces move, "Open in …" becomes a list of
deep-link templates the server supplies from connected apps or hosts (A1/A2), drawn in the task
detail's actions slot. The runner presets naming vendors (`external-agents/runnerPresets.ts`) go
the same way with A2. W1.27c does the web side.

### 3.12 Testing

- **Fewer full-App tests.** Shell and routing behaviour (which sidebar, which module is off,
  redirects, workspace switching) is tested against the route table and registrations with a
  memory router and a stub op client. Keep about five full-App smoke tests (sign-in, open a
  channel, open a task, a turned-off module, a workspace switch). The eight `App.*.test.tsx`
  files shrink accordingly.
- **Component test pattern for pieces.** `pieces/testing.tsx`:

  ```ts
  renderPiece(<TablePiece {...props} />, { width: 390 | 1280, things: [...], fields: [...] })
  ```

  sets the container width (a fake `ResizeObserver`), seeds the thing store with generated-type
  fixtures (`satisfies Task` so a server change breaks the fixture's type check), and gives
  helpers: `expectAxeClean()`, `pressKeys('j j Enter')`, `expectOp('task_update', { requestId:
  expect.any(String), expectedVersion: 3 })`. Each piece has one file asserting behaviour at both
  widths; no snapshots.
- **Playwright for the test for "done"** (`END-STATE.md` section 1). W1 adds two projects,
  `desktop` (1440×900) and `phone` (Pixel 7 emulation), and `e2e/done/` with one spec per item.
  Each spec is written by the workstream that builds the feature; W1 writes the harness, the
  helpers and the specs for items whose parts exist, and marks the rest `test.fixme` with the
  owning workstream named:

  | Item | Spec | Owner of the feature | W1 helper |
  | --- | --- | --- | --- |
  | 1 sign up, name a workspace, invite | `done/01-start.spec.ts` | H1, K4 | `newWorkspace()`, mail stub |
  | 2 add an agent three ways | `done/02-agents.spec.ts` | A2 | `connectTestAgent()` (MCP client with a token) |
  | 3 chat to task, decision, page, board | `done/03-chat.spec.ts` | C2, C1, T1 | composer and message menu helpers |
  | 4 phone: agent edits a sheet live, undo | `done/04-phone-sheet.spec.ts` (phone project) | C1, A2 | `asAgent(token).call(op)`, live wait |
  | 5 give an agent a role | `done/05-roles.spec.ts` | K1 | |
  | 6 a tracker with fields, board, public form, agent triage | `done/06-tracker.spec.ts` | T1, M1 | piece helpers |
  | 7 what every agent is doing | `done/07-runs.spec.ts` | A1 | run view helpers |
  | 8 everything off but chat, or but pages | `done/08-modules.spec.ts` | M1 | `setModules()` |
  | 9 export and import | `done/09-move.spec.ts` | H1 | |

- **Checking a slice in a real browser.** The `check-in-browser` skill gains a section: open the
  changed route at 1440 and 390 px, light and dark, with `e2e/support/look.ts`
  (`look(page, path, name)` saves four screenshots); and for live features, a second browser
  context as another person plus an agent through MCP to watch the change arrive.

## 4. Migration path

The app is live and many sessions edit `apps/web` at once. Every step below keeps old and new
working together.

1. **Stop the bleeding with today's wire** (W1.1 to W1.3): split the realtime context, move all
   keys into factories that realtime imports, and replace the project-wide fallthrough with
   per-event handlers that name keys. No server change. The broad refresh stays only for
   reconnect until W1.6.
2. **Generated client beside the old one** (W1.4, W1.5): `api/ops.ts` and the thing store land
   with one small module moved as proof. Old `requestJson` code keeps working. A ratchet test
   stops new hand-written calls (10.1). Each later module move deletes its hand types and keys in
   the same pull request.
3. **Precise live updates** (W1.6, W1.7): the new transport negotiates with the server; if the
   server doesn't offer subscriptions (K5 not yet deployed), the client falls back to today's
   path from step 1. Both dispatchers feed the same handlers, so modules write handlers once.
   The old dispatcher is deleted in W1.6b once the server ships subscriptions and one release
   has passed.
4. **Router and shell** (W1.10 to W1.12): switch to the data router with identical URLs
   (no visible change), then mount the shell once with `AppShell` turned into a pass-through
   inside it, then move pages off `AppShell` folder by folder. The pass-through goes when the
   last call site does (W1.12c).
5. **Registrations** (W1.13): one module at a time; the shell draws registered entries and still
   draws the old hand rows for modules not yet registered (a `legacyRows` list that only
   shrinks).
6. **Workspace in the URL** (W1.14): new routes and the redirect table land together, so every
   old link works the day new links appear. Internal links switch to `hrefFor` in the same
   slices. The server's own links (notifications, push, mail, MCP `href`s, manifest href
   templates) move in K4's slice; until then they hit the redirects. Redirects are deleted no
   earlier than one release after K4's link slice, in W1.14d.
7. **Pieces** (W1.16 onwards): collections and embeds move first (already generic), then other
   lists, then tasks after T1. Each move deletes the twin in the same slice once its parity e2e
   passes, never before.
8. **Phone and offline** (W1.29 onwards): the outbound queue reads the old `localStorage` queue
   on first start and moves its entries over (idempotency keys preserved), so nothing written
   offline before the update is lost.

No database change is owned by W1. Nothing is renamed on the server by W1.

## 5. Slices

Sizes: S about a day, M a few days, L about a week. "Beside" means it can be built in parallel.

### Phase A: precise updates on today's wire, and the generated client

**W1.1 Realtime context split.** Split `realtime/realtimeContext.ts` into a stable context
(`connection`, `resyncGeneration`) and an ephemeral store for streams, message and reaction
updates read with `useSyncExternalStore` (`useStream(conversationId)`). Update the four
consumers named in 2.4.
- Changes: `realtime/realtimeContext.ts`, `useRealtimeConnection.ts`, `RealtimeProvider.tsx`,
  conversation timeline consumers, `shell/AppShell.tsx`, `shell/ProjectRail.tsx`,
  `whiteboards/BoardCanvas.tsx`, `memory/live/LivePage.tsx`.
- Depends on: nothing.
- Acceptance: `realtime/RealtimeProvider.test.tsx` gains a test that a component reading only
  `connection` renders once while 100 stream frames arrive; streaming text still shows (existing
  `App.streaming.test.ts`).
- Size: S. Beside: W1.2, W1.10, W1.15.

**W1.2 Every key from a factory.** Each feature's `api.ts` exports its key factory; inline
`queryKey: [` literals in the 42 files move into them; `realtime/liveDocuments.ts`,
`useRealtimeConnection.ts`, `modules/ModuleGate.tsx:37` import factories. Adds the ratchet lint
(10.5).
- Changes: feature `api.ts` files, realtime files, `ModuleGate.tsx`, `eslint.config.js`,
  `api/queryKeys.test.ts`.
- Depends on: nothing.
- Acceptance: the lint ratchet test passes with the count at zero for realtime and shell; a
  test renames a key factory and sees the live document test fail (proves realtime reads the
  factory).
- Size: M (mechanical, touches many folders; split into two PRs by folder if conflicts).
  Beside: W1.1, W1.10.

**W1.3 Per-event handlers, no project-wide refetch.** Replace `invalidateProject` as the
fallthrough with a handler table by event type (message created, edited, removed; thread
created, status; run started, progressed, finished; reply progress; member and channel
changes), each naming the keys it touches (the conversation's timeline and its sidebar row, the
run's detail and list, the thread's card). Keep the broad refresh only for (re)connect. Legacy
events get the same table by type.
- Changes: `realtime/useRealtimeConnection.ts`, new `realtime/handlers/*.ts` per module,
  `conversations/liveMessages.ts`.
- Depends on: W1.2.
- Acceptance: new `realtime/handlers.test.ts` replays one event of each type into a seeded
  `QueryClient` and asserts the invalidated keys; a message event in a project with an open
  sheet, collection and page list leaves those queries untouched. Existing realtime tests pass.
- Size: M. Beside: W1.4, W1.10.

**W1.4 Op client and error sentences.** `api/ops.ts` (`callOp`, `opKey`, `opQuery`, `opList`,
`useOp`) over K2's generated descriptors, request ids (UUIDv7, reused on retry), expected
versions, error mapping to `ApiError` and generated `describeError` sentences. Move one small
module as proof (labels: list, create, rename, archive). Add the hand-written-call ratchet
(10.1).
- Changes: `api/ops.ts`, `api/ops.test.ts`, `ui/errorText.ts`, `work-items` labels calls,
  `api/handwritten.test.ts`, `api/legacy-allowlist.json`.
- Depends on: K2's first generated client slice.
- Acceptance: `api/ops.test.ts` asserts the same request id on a retried call, the
  `expectedVersion` sent, each K2 error code turning into its sentence; labels work end to end in
  `e2e/smoke.spec.ts` (create a label).
- Size: M. Beside: W1.3, W1.10, W1.11.

**W1.5 The thing store.** `data/things.ts` with versions, overlays, subscriptions;
normalisation in `opQuery`; `useThing`, `useThingList` with membership rules and sorted
insertion; `useOp` optimistic overlays and conflict handling. Move tasks' patch logic
(`realtime/workItemPatches.ts`) onto membership rules only when tasks move (W1.9a); until then
both live.
- Changes: `data/things.ts`, `data/lists.ts`, `api/ops.ts`, tests.
- Depends on: W1.4; K3's thing envelope (`kind`, `id`, `version`) in op results.
- Acceptance: `data/things.test.ts`: an older version is ignored; an overlay survives a base
  update and is dropped by `settle`; rollback restores exactly; a list with a rule inserts a
  matching thing in sort order and drops a non-matching one without fetching; an `unknown` rule
  marks only that list stale. `useOp` test: a `conflict` with `code: "version"` drops the overlay, refetches one
  thing, toasts.
- Size: M. Beside: W1.10 to W1.12.

**W1.6 Live client on protocol 2.** `realtime/live/`: WebSocket transport on K5's protocol 2
(`?v=2`, `hello{resumeFrom}`), one cursor per connection, reference-counted `watch` of open
documents, `resync` and `access` handling, the dispatcher (default handler plus registered
handlers), typing sends and the ephemeral stores (typing, presence, stream deltas, awareness).
Falls back to the W1.3 path when the server doesn't offer protocol 2.
- Changes: `realtime/live/*`, `realtime/RealtimeProvider.tsx`, `shell/ShellLayout.tsx` (owns
  the connection), editors (`watch`).
- Depends on: W1.3, W1.5; K5.9 (protocol 2) and K5.10 (presence and typing); K3's event shape
  with `thing`, `version`, `changes`, `requestId`.
- Acceptance: replay tests (W1.7) for: a task change in a 3-list space fetches nothing; a skipped
  version fetches one thing; a `resync` refetches only active queries and each open
  conversation's first page; an echo of a local write changes nothing; a reconnect sends
  `hello` with the last cursor. `m1_realtime_websocket`-style
  server test is K5's.
- Size: L. Beside: W1.12, W1.13.

**W1.6b Remove the old dispatcher.** Delete the envelope-v1 path and `invalidateProject`, one
release after the server ships protocol 2. K5.15 removes the server's old paths after this.
Proof that nothing is lost: the W1.7 fixtures recorded on v1 replay with the same results on
protocol 2, and `e2e/realtime*.spec.ts` passes.
- Depends on: W1.6, K5.9 deployed. Size: S.

**W1.7 Recorded event streams.** `realtime/live/replay.ts`, fixtures under
`realtime/live/fixtures/`, the Playwright frame recorder, fixtures for chat burst, task board
moves, page edits, reconnect with `resync`.
- Changes: as named; `e2e/support/recordFrames.ts`.
- Depends on: W1.3 (v1 fixtures); extended by W1.6.
- Acceptance: `replay.test.ts` runs every fixture with its expectations file; fixtures
  type-check against generated event types.
- Size: S. Beside: W1.4, W1.5.

**W1.9 Move modules to the op client.** One pull request per group, each deleting the group's
hand types, key factory, normalisers and its legacy-allowlist entries, and adding its live
handlers:
- W1.9a tasks (with T1; after T1's tasks-as-records slices), M;
- W1.9b conversations, threads, DMs, inbox, later (coordinate with C2), M;
- W1.9c pages, sheets, whiteboards (coordinate with C1), M;
- W1.9d collections, fields, views, forms (with T1), M;
- W1.9e runs, launch, agents, external agents (with A1/A2), M;
- W1.9f settings, team, workspace, invitations, projects/spaces, onboarding, M.
- Depends on: W1.5 and K2 covering the group's operations.
- Acceptance for each: the allowlist count for the group's files is zero; `pnpm typecheck`
  passes with the hand types deleted; the group's e2e spec passes.
- Beside each other, as K2 coverage allows. A neighbour workstream that rewrites a module's web
  code does that module's move instead of W1.

### Phase B: the shell and the address

**W1.10 Data router, same URLs.** `app/routes.tsx` as data; `createBrowserRouter` with
`route.lazy` and handles; `RouterProvider` in `App.tsx`; `AuthenticatedRouteBoundary` becomes
`errorElement` plus `Suspense`. No visible change.
- Changes: `app/App.tsx`, `app/routes.tsx`, `modules/slots.test.ts` (reads the route table),
  `app/App.*.test.tsx` harness.
- Depends on: nothing.
- Acceptance: all existing App tests and e2e pass unchanged; `routes.test.ts` asserts every
  route has a handle with `space` and `layout`.
- Size: M. Beside: Phase A.

**W1.11 Persistent shell.** `shell/ShellLayout.tsx` mounting `AppShell` once around
`<Outlet/>`; `shell/shellStore.ts` with `useShellPanel` and `useShellSpace`; `ShellPresence`
pass-through in `AppShell`; route fallback inside the shell.
- Changes: `shell/*`, `app/routes.tsx`.
- Depends on: W1.10.
- Acceptance: `shell/ShellLayout.test.tsx`: navigating channel → thread → page keeps the same
  sidebar DOM node (identity check) and its scroll position; the context panel follows the
  page's `useShellPanel`; the phone drawer stays open across a space change (today's
  `keepOpenOnNextNavigation` behaviour). `App.shell.test.tsx` passes.
- Size: M. Beside: W1.4 to W1.7.

**W1.12 Pages off `AppShell`.** Codemod plus hand fixes, three PRs: (a) conversations, direct,
threads, inbox, later; (b) tasks, views, ai, runs, projects, memory; (c) collections, sheets,
whiteboards, search, settings, app fallbacks; (c) also deletes the pass-through, the
`SidebarParts` maps, `MobileNavigationProvider`, `useAddressProject`, the fallback's pathname
guessing and `homeSpace.ts`'s path list.
- Depends on: W1.11.
- Acceptance: `grep "<AppShell" apps/web/src` outside `shell/` is empty (guard 10.3);
  e2e smoke passes; `check-in-browser` screenshots of each folder's main page at 1440 and 390.
- Size: M each. Beside: W1.13 (different files mostly).

**W1.13 Shell drawn from module registrations.** M1.4 builds `modules/define.ts`,
`modules/registry.ts`, one `module.ts` per core module and the generated `parts.ts`; this slice
draws the persistent shell from them: the sidebar, home rows, palette, shortcut sheet and route
table, plus the shell-only fields (things, capture, `phone`, `badge`) added to M1's
`defineModule`.
- Changes: `shell/ProjectSidebar.tsx`, `HomeSidebar.tsx`, `CommandPalette.tsx`,
  `shortcutModules.ts`, `app/routes.tsx`, `experience/registry.test.tsx`, `modules/slots.test.ts`.
- Depends on: W1.11, M1.4. Works on today's manifests; M1 later loads the same shape from data.
- Acceptance: a test adds a fake registration (test-only virtual module) and sees its route,
  sidebar row, palette entry and shortcut appear with no other file changed; registry tests:
  every nav entry has a registered part or is on an "always shown" list; turning a module off
  hides its rows (existing `App.moduleScopes.test.tsx` moved to registry level).
- Size: L. Beside: W1.12, W1.14a.

**W1.14 Workspace and spaces in the address.**
- **W1.14a `/w/:ws` routes, workspace context, switcher, redirects.** `WorkspaceBoundary`
  resolves the slug from the session (unknown slug: a "you're not in this workspace" page with
  the workspaces you are in); `WorkspaceContext` replaces `useHomeWorkspaceId` in its 10 users;
  op keys include the workspace; switcher in the rail listing every workspace the account is in
  with unread marks (R19, R20; counts from the live connection after W1.6, from a poll of
  `workspace_list` before); `/w/:ws/dm/:id` and `/dm/:id` routes (R20); legacy redirects for workspace-level
  routes; `sw.js` `APP_ROUTE`; manifest `start_url` and shortcuts. M. Depends on W1.10, W1.11;
  K4 for the server-side op paths (the client works on today's endpoints meanwhile, since
  workspace is known from the slug). Acceptance: `app/redirects.test.ts` covers every old route
  in today's `App.tsx`; `serviceWorker.test.ts` asserts `APP_ROUTE` matches every first segment
  of the route table; e2e: a person in two workspaces opens `/w/b/home` and sees B's layout and
  modules.
- **W1.14b Space routes and thing addresses.** `/w/:ws/s/:space/...` with the new module words,
  `/w/:ws/t/:kind/:id`, task keys under the space, `href.ts` helpers. M. Depends on W1.14a,
  W1.13 (registrations carry the paths), K4's space reference, K3's thing resolve. Acceptance:
  redirects test for every `/projects/...` form; `href.test.ts` round-trips each kind's href
  through the router; e2e opens a task by `/AWD-1`, by old URL and by `/w/.../t/task/<id>`.
- **W1.14c Internal links through `href.ts`.** Replace literal paths in components; guard 10.3.
  S. Depends on W1.14b.
- **W1.14d Delete redirects.** One release after K4's link slice. S.

**W1.15 Initial-load diet and budget policy.** Feature CSS into route chunks; `liveMessages`
out of the entry; budget limits at measured size plus 10%; CI prints a base-versus-head delta
table.
- Changes: `main.tsx`, route modules importing their CSS, `scripts/web-budget-lib.mjs`,
  `scripts/check-web-budgets.mjs`, `.github/workflows/ci.yml` (report step).
- Depends on: nothing (W1.10 makes it easier).
- Acceptance: `web-budget-lib.test.mjs` tests the delta table; initial CSS drops by the moved
  sheets (measured in the PR); a test that `main.tsx` imports only `styles/{tokens,base,components,shell,kit}.css`.
- Size: S. Beside: anything.

### Phase C: shared pieces

**W1.16 One field contract and the field registry. Moved to T1 (T1.13a) by R17**: T1 owns
everything that knows fields. Kept here for reference; T1.13a carries this text and acceptance.
W1's share is only the kit primitives the editors use (W1.17). Original text:
`pieces/fields/registry.ts` keyed by T1's
field types (renderer, editor, filter operators, sort, width); manifest `FieldDecl` becomes a
declared subset mapped to `FieldDefinition`; the second `FieldType` enum in `modules/contract.ts`
is replaced by T1's generated one.
- Depends on: T1's generated field types.
- Acceptance: `pieces/fields/registry.test.tsx` renders and edits every field type at both
  widths with axe; a field type missing from the registry fails the test (iterates T1's type
  list).
- Size: M. Beside: W1.17.

**W1.17 Pieces foundation.** Move pieces to `apps/web/src/pieces/`; move `useWindowedRows`,
`useBoardDrag`, `useCoarsePointer`, `listKeys` into `ui/`; add `SidePanel`, `Sheet`, selection
model, grid keyboard primitives, density tokens; `pieces/testing.tsx`.
- Depends on: nothing.
- Acceptance: kit tests for `SidePanel` (Escape rule, focus return), grid keyboard; existing
  tests pass after moves.
- Size: M. Beside: Phase A and B.

**W1.18 Table and list pieces; collections onto them.** Built from kit `Table`, `RecordTable`
and piece list; inline edit through `onChange`; windowing; group rows. Collections' table view
and the project Pages list use them; `RecordTable.tsx` deleted.
- Depends on: W1.17, W1.5 (and W1.9d for collections data). The piece takes `columns` (3.7);
  whichever of this slice and T1.13 lands second swaps the collection components onto the
  piece through T1's adapters (stopgap: a local adapter over today's `RecordTable` props).
- Acceptance: `pieces/TablePiece.test.tsx`, `ListPiece.test.tsx` (keyboard, selection, edit
  dispatches op with request id and version, narrow cards, axe); `e2e/collections.spec.ts`
  passes; a 10,000-row fixture renders under 60 DOM rows.
- Size: L. Beside: W1.19.

**W1.19 Board piece; collections onto it.** From `RecordBoard` + `useBoardDrag`; per-column
paging; narrow one-column mode; `RecordBoard.tsx` deleted.
- Depends on: W1.17 (same swap rule with T1.13 as W1.18).
- Acceptance: `BoardPiece.test.tsx` (drag by keyboard and pointer calls `onMove`, refused move
  snaps back with reason, column paging, narrow switcher); `e2e/views.spec.ts` passes.
- Size: M. Beside: W1.18.

**W1.20 Calendar and gallery pieces.** From `RecordCalendar` and `RecordGallery`;
collections onto calendar and gallery. The timeline renderer moved to T1 (T1.14, R17); W1
provides the grid, drag and narrow primitives it is built on (W1.17).
- Depends on: W1.17. Acceptance: one component test each at both widths; collections e2e.
- Size: M. Beside: W1.18, W1.19.

**W1.21 Detail view, history panel, chips.** `ThingDetail` from `RecordPanel` with sections and
tabs; `HistoryPanel` over K3's history op; `ThingChip` with hover card; discussion slot hosting
C1's component; collections' record panel onto it.
- Depends on: W1.17; K3 history op; C1's discussion component for the discussion tab (tab hidden
  until then).
- Acceptance: `ThingDetail.test.tsx` (as panel and as page, narrow sheet with back), history
  renders "requested by"; collections e2e.
- Size: M. Beside: W1.22.

**W1.22 Form piece.** One `FormPiece` used by the public form route and in-app create, taking
its inputs from T1's field registry. The filter builder moved to T1 (T1.13 `FilterEditor`,
R17), with its URL sync and the collections toolbar.
- Depends on: W1.17; T1.13a (field editors) and T1.28 (form config); stopgap until T1.13a:
  today's `FieldEditor`.
- Acceptance: `FormPiece.test.tsx` (required, hints, file field, submit op);
  `e2e/collections.spec.ts` public form.
- Size: M. Beside: W1.21.

**W1.23 Chart piece. Moved to T1 (T1.31 `ChartView`) by R17.** T1 builds the chart renderer
with the view kind; W1 only holds it to the lazy-chunk budget (W1.15) and the accessibility
gate (W1.36). Kept for reference. Original text: SVG charts from T1's aggregate results;
number tiles; lazy chunk.
- Depends on: T1's Dashboards groundwork (chart config and aggregate op).
- Acceptance: `ChartPiece.test.tsx` (each chart type, empty data, narrow legend, axe with
  accessible table fallback).
- Size: M. Beside: W1.21, W1.22.

**W1.24 Request card and run view.** R28: extracted from A1's work, not rewritten.
`RequestCard({card: RequestCard})` extracted from A1.8's `requests::card` result and answering through `request_answer`, and `RunView` grown from A1.5's
`runs/RunPage.tsx` (A1.5 builds the page and its data; this slice makes it the shared piece),
used by the inbox (C2.12), run page, thread cards and task detail's runs tab.
- Depends on: A1.5 (runs page and run operations), A1.8 (requests and `request_answer`).
- Acceptance: component tests for each request kind's answer flow (op called with request id),
  run view with gaps in activities shown honestly; `App.runs.test.tsx` cases moved here.
- Size: M. Beside: Phase C.

**W1.25 Panel block renderer.** `pieces/blocks/` exporting `PanelBlocks({blocks, onAction})`,
drawing M1's block JSON; actions call operations through the op client; routes `m/:module/*`
for connected and built modules. M1.14's `PanelHost` (fetch, signing, time-outs, slots) renders
through it.
- Depends on: M1's block format (M1.12) and `module_list` shape; T1.31 for the chart block
  (placeholder until then).
- Acceptance: `Blocks.test.tsx` renders every block type at both widths with axe and refuses
  unknown block types with a visible placeholder; an e2e with a stub connected module.
- Size: M. Beside: Phase C.

**W1.26 Embeds through registrations and pieces.** `PageEmbed` dispatches on the `embeds`
that M1.5 registers (match pattern to piece or component); collection and sheet embeds onto
pieces; the `OPENS` set and task regex in `memory/embeds/moduleEmbed.ts` go.
- Depends on: M1.5, W1.13, W1.18; coordinate with C1 (page editor) and K3 (`Thing.fields`).
- Acceptance: `memory/embeds/*.test.tsx` updated: each module's declared embed renders;
  `e2e/pages.spec.ts` embeds pass.
- Size: M.

**W1.27 Tasks onto pieces.** After T1 ships tasks as records with typed view configs. Each
slice starts with its parity list (3.7) as an e2e that passes on the old screen, then swaps,
then deletes the twin.
- W1.27a task list. **Merged into T1.15** (the Tasks page on the views engine: list), which
  builds it on W1.18's pieces with these task slots (done tucking, bulk bar, quick create,
  claim chips) and the parity e2e below; `WorkItemList.tsx` is deleted by T1.39. Not a W1 slice.
- W1.27b task board. **Merged into T1.16** (board and timeline), on W1.19's piece with workflow
  move checks and the column composer; `WorkItemBoard.tsx` and `boardModel` deleted by T1.39.
  Not a W1 slice.
- W1.27c task detail → `ThingDetail` with task sections (criteria, links, files, runs, activity
  through C1's discussion; the Fields section is T1's), "Open in" deep links from data; deletes
  `TaskDetail.tsx`, `codexDeepLink.ts`. M.
- W1.27d task filters. **Merged into T1.15 and T1.39** (T1.13's `FilterEditor` with the URL
  adapter keeping `?view=` and old query links working; `FilterBar.tsx` deleted by T1.39). Not
  a W1 slice.
- Depends on (W1.27c): W1.21, W1.9a, T1.15, C1.3.
- Acceptance: the parity e2e (`e2e/tasks-parity.spec.ts`, written by W1 against today's
  screens and handed to T1.15) passes before and after; T1.39 may delete the old paths only
  once it passes on the new ones; `WorkItemsPage.test.tsx` cases moved to piece tests.

**W1.28 Other lists onto the list piece.** My work, Later, whiteboard grid (gallery), decisions;
inbox rows with C2.
- Depends on: W1.18, W1.20. Size: M. Beside: W1.27.

### Phase D: phone, install, offline, speed and tests

**W1.29 Phone navigation.** Bottom bar from registrations' `phone: 'tab'`; detail as full-screen
sheet with history-backed back; narrow pass on every piece in e2e.
- Depends on: W1.13, W1.21.
- Acceptance: `e2e/phone.spec.ts` (phone project): every bottom-bar destination opens, back
  button closes a detail sheet, no page scrolls sideways at 390 px on any registered route.
- Size: M.

**W1.30 Outbound queue for any queueable operation.** IndexedDB queue, per-thing ordering,
pending marks, "Not sent" handling, migration of the old `localStorage` queue.
- Depends on: W1.4, W1.5; K2's `queueable` flag.
- Acceptance: `app/outbox.test.ts` rewritten: offline task edit and message are kept, sent in
  order on reconnect with the same request ids, a refusal stays with its reason; old-format
  entries move over; sign-out clears. e2e: Playwright `context.setOffline(true)`, edit a task
  and send a message, go online, both arrive once.
- Size: M.

**W1.31 Offline reading.** The one offline mechanism (accepted end-state change): an opt-in
per person in account settings, which a workspace setting can switch off. When on, persist the
thing store and recent first pages to IndexedDB per account with caps; restore before network;
clear at sign-out, on `authentication_required` and when the person or the workspace switches
it off. C2.33 (offline chat) uses this, not a second store.
- Depends on: W1.5, W1.30.
- Acceptance: `data/persist.test.ts` (off by default, caps, per account, clear on sign-out and
  on the workspace switch); e2e: open a page and a channel, go offline, reload in the
  installed-app emulation, both read.
- Size: M.

**W1.32 Install, push and capture.** Manifest and shortcuts for `/w/...`; capture registrations
on `/new` and `/share`; push actions for every A1 request kind.
- Depends on: W1.14a, W1.13; A1 requests; C2 for push content.
- Acceptance: `serviceWorker.test.ts` covers request actions and new URLs; e2e share-in with a
  file to a collection form.
- Size: S.

**W1.33 Speed in CI.** `e2e/perf.spec.ts` with CPU throttling and network shaping through the
browser's DevTools protocol, a 10,000-row seeded collection and a busy chat, reading
`aw:route-ready` marks and long tasks; results compared with a stored baseline; report always,
fail only beyond 25% over the target in 3.9. Runs on the release host only (stable hardware).
- Depends on: W1.18, W1.11.
- Size: M. Beside: Phase C.

**W1.34 Test diet.** Shell, routing and module-gating tests moved from full-App tests to route
and registration tests; keep about five full-App smokes; vitest timeout back toward default.
- Depends on: W1.12, W1.13. Size: M.

**W1.35 The test-for-done harness.** Desktop and phone Playwright projects; `e2e/done/` specs
with helpers (`connectTestAgent`, `asAgent`, `look`, `newWorkspace`, `setModules`); specs for
items whose parts exist, `test.fixme` naming the owner for the rest; `check-in-browser` skill
section.
- Depends on: nothing hard; better after W1.14a.
- Acceptance: the suite runs in CI's e2e job; fixme specs are listed in the report with owners.
- Size: M. Beside: anything.

**W1.36 Accessibility gate.** Route-level axe sweep at two widths and two themes; theme
contrast test; grid keyboard e2e.
- Depends on: W1.13 (route list), W1.17.
- Size: S. Beside: anything.

**W1.37 Account-kind difference in roles UI.** Role choices come from the server's
`assignableRoles` per member; remove the `actorKind` checks at `team/TeamSettings.tsx:389,392`.
- Depends on: K1's role data. Size: S.

**W1.38 The web app as static files (R24.1).** Remove every server-written value from
`index.html` and the shell; the app reads `GET /client-config` at start (API and live
origins, sign-in methods, switches) and caches it; a build that runs from a plain static file
server against an API on another origin; the server's static serving stays as an option for a
self-hoster.
- Changes: `apps/web/index.html`, `main.tsx`, `app/config.ts`, `api/client.ts` (base origin),
  `sw.js` (config cached with the shell); server side: the config route, which is H1's (R30).
- Depends on: W1.10; H1's `client-config` route.
- Acceptance: CI job serves `apps/web/dist` from a static server on one port and the API on
  another, and the e2e smoke passes; a test fails if `index.html` contains a template marker.
- Size: S. Beside: anything.

**W1.39 Shape walk test (R22).** One Playwright spec per shape M1 ships ("Friends", "Team",
"Just me and my agents", "Documents", "Blank"): create a workspace from the shape, walk the
shell, every settings page, search, the command list, every empty state, the phone bar, and fail
on any part from a module that is off or any of its words (for "Friends": "project", "task",
"agent", "run", in every layout's words). The walk runs twice per shape (R31): with people only
(no such word anywhere), and with an agent member (the word "agent" allowed only on that
account's badge beside its name and on its profile line, which every shape shows because an
account's kind is account information, not the Agents module; this run also covers the
Add-an-agent offer to switch Agents on, which is offered and not required). The shapes' module lists come
from M1's packages; "Just me and my agents" has Team's modules and differs only in layout (R29). Emails are H1's check; the spec reads the module's
words from M1's registrations, so a new module's words are covered without editing the test.
- Changes: `e2e/shapes.spec.ts`, `e2e/support/shapes.ts`.
- Depends on: W1.13, W1.35, M1's shapes slice (M1.17); C2's chat-alone slice for "Friends" to
  pass (C2.40; `test.fixme` naming C2 until then).
- Acceptance: the spec runs in CI's e2e job for every shape; turning a module on in a shape
  makes its words allowed with no test edit.
- Size: M. Beside: anything.

## 6. Guard rails

1. **No new hand-written API types.** `api/handwritten.test.ts` counts `requestJson` calls and
   `export type` declarations in feature `api.ts`, `types.ts` and `wire.ts` files against
   `api/legacy-allowlist.json` (per file). A new file with calls, or a count going up, fails;
   counts can only go down, and the test prints how to move to `api/ops.ts`. ESLint
   `no-restricted-imports` forbids importing `api/client` outside `api/` for files not on the
   allowlist.
2. **No whole-project refetch.** (a) A source test fails on any `invalidateQueries` or
   `refetchQueries` whose predicate tests only `queryKey[0]` and `queryKey[1]`, and on
   `queryKey: ['project', id]` or `['op', ws]` prefixes without an operation name, outside
   `realtime/live/reconnect.ts`. (b) Every replay fixture asserts its fetch list; a new handler
   ships with a fixture. (c) In development builds, `invalidateQueries` warns when one call hits
   more than 20 queries.
3. **No module ids or module paths matched by hand in the shell.** A test scans `shell/**`,
   `app/**` and `realtime/live/**` for string literals equal to any manifest id, any
   registration path, `'/projects/'` or `'/w/'` path templates, and fails (the shell reads
   registrations). A second test fails on `<AppShell` outside `shell/` once W1.12c lands, and on
   literal `to="/..."` app paths outside `app/href.ts` after W1.14c.
4. **No page or row that skips the layout registry.** `experience/registry.test.tsx` extends to
   the route table and registrations: every route handle's `part` and every nav entry's `part`
   exists in `registry.json`, or the entry is on a short, reviewed "always shown" list in the
   test; the generated `PartId` type makes a typo a type error.
5. **Keys only from factories or ops.** ESLint ratchet on `queryKey: [` array literals (only
   `api/ops.ts` and key factory files may build arrays).
6. **Budget.** `web-budget-lib` limits with the headroom policy; a test that
   `*/module.ts` import only types, `define.ts`, icons and lazy `import()`s (M1's guard, 3.4) (so a
   registration can't drag a page into the initial load); a test that `main.tsx` imports only
   the shell stylesheets.
7. **The service worker keeps up with routes.** `serviceWorker.test.ts` asserts `APP_ROUTE`
   matches the first segment of every route in the table.
8. **Pieces stay whole.** Each piece's component test must exercise both widths and axe (a
   helper that fails if `renderPiece` was called at only one width in a file); the field registry
   test iterates T1's field type list.
9. **Fixtures follow the server.** Test fixtures for ops and events are typed with generated
   types (`satisfies`), so a server rename breaks them at type check.

## 7. Risks and open questions

**Risks.**

1. **The shell move collides with parallel work.** 36 files change in W1.12. Fallback: the
   `ShellPresence` pass-through means each PR is small and mergeable in any order; nothing
   breaks if a page keeps `<AppShell>` for weeks. Noticed by: merge conflicts in review; the
   guard in 6.3 only turns on in W1.12c.
2. **The thing store shows stale or wrong data.** A normalised cache has subtle bugs (a list
   rule that thinks a thing left a filter when it didn't). Noticed by replay fixtures and the
   parity e2e. Fallback: a per-list switch that treats every rule as `unknown` (refetch that
   list on a change of its kind), still far narrower than today.
3. **K2 or K5 arrive late or shaped differently.** Phase A's W1.1 to W1.3 deliver most of the
   performance win on today's wire with no dependency; W1.4 onward wait. If events don't carry
   `changes`, the default handler fetches the one thing (one small request per event), still
   precise. If they don't carry `requestId`, an echo causes one extra render, not a bug.
4. **Data router behaviour differs** (navigation timing, `useBlocker`, scroll restoration).
   W1.10 is a no-visible-change slice with the full e2e as its gate.
5. **Budget.** The data router and the store add maybe 10 to 15 KB gzip to the entry (estimate,
   not measured). W1.15's CSS move and App.tsx diet should more than cover it; measure in W1.10
   before committing to W1.11.
6. **Offline data on shared devices.** Persisting content changes the current privacy promise.
   Mitigated by per-account storage, clearing at sign-out and on session expiry, and the owner's
   call below.
7. **Two workspaces' links in old content.** Messages and pages hold `/projects/...` links whose
   workspace must be found; the redirect resolves through the projects list, which a person who
   left the project can't see: they get the not-found page, same as today.

**Questions for the owner** (each with my recommendation):

1. **One workspace at a time in the app?** Settled by R19 and R20: `/w/<slug>` with a switcher
   listing every workspace the account is in with unread marks; DMs are the same in every
   workspace. A merged cross-workspace Home would fight
   per-workspace layouts and modules.
2. **Keep recent things on the device for reading offline?** Settled by the accepted end-state
   change: an opt-in per person, cleared at sign-out, with a workspace setting to turn it off
   (H1/K1 own the setting).
3. **User-visible words in addresses**: `tasks` not `work-items`, `runs` not `launches`, `chat`
   for channels, `s/<key>` for a space, with redirects. Recommend yes, done with the workspace
   change so links break once, not twice.
4. **Phone navigation**: a bottom bar (Home, Inbox, Messages, Search, You; the current space
   from Home and the drawer) instead of only the drawer. Recommend yes; it is what people expect from a chat app on a phone.

## 8. Touch points

**Needs from others.**

- **K2**: generated `apps/web/src/api/generated/` with, per operation: name, kind, method, path,
  input and result types, the thing kind it acts on, `queueable`; the page shape
  (`{ items, nextCursor }`); the error list with codes (including the version conflict, outcome `conflict` with `code: "version"`, carrying the
  current version and the actor) and one plain sentence per code; the request id header name;
  generated event payload types. I assume an `Op<Name, Input, Result>` descriptor shape (3.5);
  if K2 generates functions instead, `ops.ts` wraps those.
- **K3**: the thing envelope (`kind`, `id`, `version`, `title`, `status`, `href`, `spaceId`,
  `fields`) in op results; events with `thing`, `version`, `changes` (changed field values) and
  `requestId`; a `thing_get`/resolve operation for `/t/:kind/:id`; the history operation for the
  history panel; accepting `/w/...` addresses as mentions and links (and old ones).
- **K5**: protocol 2 (K5.9: `hello{resumeFrom}`, `resync`, `access`, `watch`, ephemeral frames
  for stream deltas, awareness and document updates; K5.10: presence and typing); the live
  role's ticket from `api` (R24.5) naming the audiences, including the direct scope and other
  workspaces' unread counts (R20); a way to say whether protocol 2 is offered (for the
  fallback); the client config route (R24.1, or H1).
- **K4**: workspace slugs and space references (I assume space key, id accepted), reserved words,
  server-generated links (notifications, push, mail, MCP hrefs, manifest href templates) moving
  to `/w/...`, op paths that carry the workspace.
- **K1**: per-member `assignableRoles`; capabilities for settings visibility instead of role
  names (`settings/registry.ts:48-49`); the web permission hook `useCan` (K1.26), which pieces'
  `capabilities` and the shell's settings nav read instead of role names.
- **T1**: generated types for field definitions, every field type's value, view config, filter
  grammar, form config, chart config and aggregate results; tasks as records with typed views
  before W1.27.
- **M1**: the registration contract (`defineModule`, `modules/registry.ts`, generated
  `parts.ts`, M1.4) carrying the shell fields of 3.4, embed registrations (M1.5);
  `module_list` at runtime with nav, routes and panels for connected and built modules; the
  block format; the shapes (M1.17) for the shape walk test.
- **A1**: `requests::card` and `request_answer` (A1.8), the run page and run operations (A1.5),
  activity types, request kinds for push actions, deep-link templates for "Open in" (with A2).
- **C1**: `ThingComments({thing, anchor?, threadId?})` for the discussion panel shell, editors'
  use of `awareness` and `watch`, embed registrations for pages, sheets and boards.
- **T1**: the field registry (T1.13a), view adapters and `FilterEditor` (T1.13), the timeline
  (T1.14) and chart (T1.31 `ChartView`) renderers, and the Tasks page moves (T1.15, T1.16,
  T1.39) with W1's parity e2e.
- **C2**: chat, inbox and phone chat screens built on the shell, pieces and live client; push
  payload `url`s in the new form; inbox rows on the list piece (W1.28).
- **H1**: the per-workspace "keep data on devices" setting; export includes nothing W1 adds
  (W1 stores nothing on the server).

**Provides to others.**

- `api/ops.ts` (`callOp`, `opQuery`, `opList`, `useOp`) and the thing store (`useThing`,
  `useThingList`), for every workstream's web work.
- The live client (`useLiveKinds`, registered event handlers, `useStream`, `usePresence`,
  `useTyping`, `useAwareness`) and the replay harness.
- The persistent shell with `useShellPanel` and `useShellSpace`, drawn from M1's `defineModule`
  registrations (routes, nav, palette, shortcuts, settings, live, things, embeds, capture), the
  workspace switcher with unread marks, and the DM routes.
- `app/href.ts` and the route table, including `/w/:ws/t/:kind/:id` and `/w/:ws/dm/:id`.
- Pieces in `apps/web/src/pieces/` (table, list, board, calendar, gallery, detail, form,
  chips, discussion panel shell, history panel, request card, run view, panel blocks) and `ui/`
  additions (`SidePanel`, `Sheet`, grid primitives). Timeline, chart, filter builder and the
  field registry are T1's (R17).
- The outbound queue for any queueable operation, offline reading, Playwright projects and
  helpers for the test for "done", and the shape walk test (W1.39).

## 9. Changes to the end state

1. **Browser transport.** `END-STATE.md` section 4's standards table says "Server-sent events …
   Not WebSocket: one direction is enough". That is right for agents, but a browser sends
   presence, typing and subscription changes. Proposed wording for the "Stream and poll" row's
   notes: "Not WebSocket for agents: one direction is enough and it scales from the log. The
   browser may use one WebSocket carrying the same log frames plus presence and typing, or
   server-sent events with presence sent as operations; both read the same cursor."
2. **"The web app updates exactly the things an event names"** (section 5, Live updates) is
   practical only with one exception. Proposed: "The web app updates exactly the things an event
   names, and refetches a list only when it can't tell by itself whether the change moved a
   thing into or out of it. It never refetches a whole space."
3. **Offline reading** (section 5, The web app, last bullet) should name the device-storage
   trade-off. Proposed: "Installable on a phone, with push, sending that survives a lost
   connection, and recently opened things readable offline (kept on the device per person,
   cleared at sign-out, and a workspace can turn it off)."
4. **Account settings outside the workspace address.** Section 3 says a person's identity is
   global; the address scheme should say so. Proposed addition to "One shell, mounted once":
   "Account settings, sign-in and public forms live outside any workspace address
   (`/settings/account`, `/forms/...`)."
5. **Phone bar.** Section 8's Phone row should add a measurable bar: "Opens to content in under
   1.2 s warm and 3.5 s cold on a mid-range phone; every list and board usable at 390 px."

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| W1.1 | Realtime context split | S | none | W1.2, W1.10, W1.15, W1.17 |
| W1.2 | Every key from a factory | M | none | W1.1, W1.10, W1.17 |
| W1.3 | Per-event handlers, no project-wide refetch | M | W1.2 | W1.4, W1.10, W1.11 |
| W1.4 | Op client and error sentences | M | K2 generated client | W1.3, W1.10, W1.11 |
| W1.5 | The thing store | M | W1.4, K3 thing envelope | W1.10-W1.12 |
| W1.6 | Live client on protocol 2 | L | W1.3, W1.5, K5.9, K5.10, K3 events | W1.12, W1.13 |
| W1.6b | Remove the old dispatcher | S | W1.6, K5.9 deployed | anything |
| W1.7 | Recorded event streams | S | W1.3 | W1.4, W1.5 |
| W1.9a | Tasks onto the op client | M | W1.5, K2, T1 | W1.9b-f |
| W1.9b | Conversations, inbox, later onto the op client | M | W1.5, K2 (with C2) | W1.9a, c-f |
| W1.9c | Pages, sheets, whiteboards onto the op client | M | W1.5, K2 (with C1) | W1.9a-b, d-f |
| W1.9d | Collections, fields, views, forms onto the op client | M | W1.5, K2 (with T1) | W1.9a-c, e-f |
| W1.9e | Runs, launch, agents onto the op client | M | W1.5, K2 (with A1, A2) | W1.9a-d, f |
| W1.9f | Settings, team, spaces, onboarding onto the op client | M | W1.5, K2 | W1.9a-e |
| W1.10 | Data router, same URLs | M | none | Phase A, W1.15, W1.17 |
| W1.11 | Persistent shell | M | W1.10 | W1.4-W1.7 |
| W1.12a | Pages off AppShell: chat and inbox folders | M | W1.11 | W1.13, Phase A |
| W1.12b | Pages off AppShell: tasks, views, runs, pages folders | M | W1.11 | W1.12a, W1.13 |
| W1.12c | Pages off AppShell: the rest, delete workarounds | M | W1.12a, W1.12b | W1.13 |
| W1.13 | Shell drawn from module registrations | L | W1.11, M1.4 | W1.12, W1.14a |
| W1.14a | Workspace in the address, switcher with unread marks, DM routes, redirects | M | W1.10, W1.11, K4 slugs | W1.13, W1.12 |
| W1.14b | Space routes and thing addresses | M | W1.14a, W1.13, K4 spaces, K3 resolve | Phase C |
| W1.14c | Internal links through href helpers | S | W1.14b | Phase C |
| W1.14d | Delete legacy redirects | S | W1.14c, K4 link slice + one release | anything |
| W1.15 | Initial-load diet and budget policy | S | none | anything |
| W1.16 | (moved to T1.13a: field registry) | — | — | — |
| W1.17 | Pieces foundation (kit moves, SidePanel, Sheet, grid) | M | none | Phase A, Phase B |
| W1.18 | Table and list pieces; collections onto them | L | W1.17, W1.5 | W1.19, W1.20, T1.13 |
| W1.19 | Board piece; collections onto it | M | W1.17 | W1.18, W1.20, T1.13 |
| W1.20 | Calendar and gallery pieces (timeline is T1.14) | M | W1.17 | W1.18, W1.19 |
| W1.21 | Detail view, history panel, chips, discussion shell | M | W1.17, K3 history, C1.3 (tab hidden until) | W1.22 |
| W1.22 | Form piece (filter builder is T1.13) | M | W1.17, T1.13a (stopgap: today's editor) | W1.21 |
| W1.23 | (moved to T1.31: chart renderer) | — | — | — |
| W1.24 | Request card and run view | M | A1.5, A1.8 | Phase C |
| W1.25 | Panel block renderer | M | M1.12 block format (T1.31 for chart) | Phase C |
| W1.26 | Embeds through registrations and pieces | M | M1.5, W1.13, W1.18, C1 | W1.27c |
| W1.27a | (merged into T1.15) | — | — | — |
| W1.27b | (merged into T1.16) | — | — | — |
| W1.27c | Task detail onto the detail view | M | W1.21, W1.9a, T1.15, C1.3 | T1.16 |
| W1.27d | (merged into T1.15 and T1.39) | — | — | — |
| W1.28 | Other lists onto the list piece | M | W1.18, W1.20 (inbox with C2) | W1.27 |
| W1.29 | Phone navigation | M | W1.13, W1.21 | Phase C |
| W1.30 | Outbound queue for any queueable operation | M | W1.4, W1.5, K2 queueable flag | Phase C |
| W1.31 | Offline reading (opt-in per person, workspace can switch off) | M | W1.5, W1.30 | Phase C |
| W1.32 | Install, push and capture | S | W1.14a, W1.13, A1 requests | Phase C |
| W1.33 | Speed in CI | M | W1.11, W1.18 | Phase C |
| W1.34 | Test diet | M | W1.12c, W1.13 | Phase C |
| W1.35 | The test-for-done harness | M | none (better after W1.14a) | anything |
| W1.36 | Accessibility gate | S | W1.13, W1.17 | anything |
| W1.37 | Role choices from the server, not account kind | S | K1 role data | anything |
| W1.38 | The web app as static files, config fetched at start | S | W1.10, H1's `client-config` | anything |
| W1.39 | Shape walk test | M | W1.13, W1.35, M1.17 (C2.40 for "Friends") | anything |

## Review changes

- 3.5 example path `/api/v1/w/...` → `/api/v2/w/...` (R4).
- 3.6 wire rewritten to K5's protocol 2 (`hello`/`resumeFrom`, `watch`, `typing`, `awareness`, `resync`, `access`; no client subscriptions), WebSocket named (R15), list-refetch on `access` (accepted change); W1.6, W1.6b, W1.7 and touch points follow; W1.6 depends on K5.9 and K5.10.
- 3.6 live connection takes a ticket from `api` and connects to the live role; audiences include the direct scope and other workspaces' unread counts (R24.5, R20).
- 3.7 `PieceCommon` made generic (`columns` with render and editor, `groupOf`, `dateOf`); T1's adapters build them (R17).
- Timeline (W1.20 part) → T1.14, chart piece (W1.23) → T1.31, field registry (W1.16) → T1.13a, filter builder (W1.22 part) → T1.13: R17 gives T1 everything that knows fields; rows kept marked "moved".
- W1.27a, W1.27b and W1.27d marked merged into T1.15, T1.16 and T1.39 (true duplicates); W1 keeps writing the parity e2e and W1.27c.
- Discussion panel is a shell hosting C1's `ThingComments`; request card takes A1's `RequestCard` and answers with `request_answer`; run view grows from A1.5; W1.24 depends on A1.5 and A1.8.
- Panel blocks follow M1's block set; W1.25 exports `PanelBlocks` used by M1.14's `PanelHost`.
- 3.4 and W1.13: M1 owns `defineModule` and the registry (M1.4); W1 adds the shell fields and draws the shell; `defineWebModule` dropped (two builders of one thing). W1.13 depends on M1.4; W1.26 on M1.5.
- W1.18 and W1.19 no longer wait on the field registry; whichever of them and T1.13 lands second swaps the collection components.
- W1.31 is the one offline mechanism: opt-in per person, workspace switch, cleared at sign-out (accepted change); section 7 questions marked settled.
- Phone bar fixed to Home, Inbox, Messages, Search, You; modules that are off drop out; Chat only shows the conversation list (R22).
- Routes `/w/:ws/dm/:id` and `/dm/:id` added; switcher lists every workspace with unread marks (R19, R20) in 3.1 and W1.14a.
- "Static files" paragraph in 3.1 and new slice W1.38 (R24.1).
- New slice W1.39, the shape walk test W1 owns (R22).
- Touch points: K1's `useCan` (K1.26) named, which K1 assumes W1 consumes.

### Second pass

- Version conflicts use K2's outcome `conflict` with `code: "version"`, not `version_conflict` (3.5, W1.5, touch points; kernel review).
- W1.38: `client-config` is H1's route (R30).
- W1.39 runs twice per shape, people only and with an agent member (badge and profile only), and reads shapes from M1 where "Just me and my agents" has Team's modules (R29, R31).
- 3.6 notes the frame host page is served by `api`, not the static app (R31).
- W1.24 extracts `RunView` from A1.5's page and the request card from A1.8's `requests::card` (R28); W1.39's agent-member run also covers the offer to switch Agents on (agents and hosting review).
