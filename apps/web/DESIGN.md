# Design System

The full reference, with live components and every token per theme, is the
Agent Workspace design system artifact (private to the owner). This file is the
in-repo summary that code follows.

## Theme

The default follows the device: "Match system" shows Indigo (a deep indigo rail
and sidebar beside a light reading surface) in light mode and Midnight (dark
everywhere) in dark mode. Five presets ship, Indigo, Midnight, Daylight (light
everywhere), Evergreen and Graphite, plus classic editor themes (Nord, Dracula,
Solarized and others) loaded only for people who pick one. Every color token is
a theme slot a person can override on the Appearance page; see
`docs/frontend/theme-and-tokens.md`.

## Color

Tokens live in `src/styles/tokens.css`, generated from the design system's
`tokens.json`, and are grouped by region:

- Rail: `rail-bg`, `rail-fg`, `rail-tile`, `rail-tile-active`, `rail-indicator`.
- Sidebar: `sidebar-bg`, `sidebar-fg`, `sidebar-fg-strong` (unread),
  `sidebar-fg-muted` (labels), `sidebar-hover`, `sidebar-active-bg` +
  `sidebar-active-fg`, `sidebar-border`.
- Content: `content-bg`, `content-fg`, `content-fg-muted`, `content-hover`,
  `content-border`, `control-border`, `surface-raised`, `surface-sunken`,
  `highlight-bg` (messages that mention you).
- Accent: `accent`, `accent-hover`, `on-accent`, `accent-soft`, `link`,
  `focus-ring`, `mention-fg` + `mention-bg`.
- Signals: `badge-bg` + `badge-fg` (mentions, New, Needs you), `presence-online`,
  `agent` + `agent-soft` (the Agent tag), `agent-working` (activity),
  `success`/`warning`/`danger` with `-soft` fills.
- Identity swatches `swatch-*` color avatars and project tiles.

Components read `--r-bg`, `--r-fg`, `--r-fg-muted`, `--r-hover` and `--r-focus`,
which `data-region="rail|sidebar|content"` sets, so one control works in every
region. Menus, dialogs and the composer always use the content palette.

## Typography

Figtree (variable) for everything, JetBrains Mono for code, branches and IDs,
both self-hosted. Nine styles as `--t-*` shorthands: display 28/36, title 18/24,
heading 15/22 bold, body 15/22, ui 14/20, ui-strong 14/20 bold, small 13/18,
caption 12/16, tag 11/14 bold. Text size scales them all through
`--text-scale`. Sentence case everywhere; no uppercase eyebrows.

## Layout

Rail 72px, sidebar 264px, header 52px, details panel 420px, message line
length capped at 760px. Conversation pages own their scrolling: header and
composer stay put, the timeline scrolls and stays on the newest message.

Phones (860px and below): the rail and sidebar open together as one drawer from
the menu button, and it stays open when you pick a Project or Messages so the
next tap chooses a channel. Conversation headers carry that button (a thread
shows a back arrow to its channel instead); other pages get a thin bar with it
and the page's name. Details open as a drawer on the right when the page's own
toggle is pressed. Swipe right to open the drawer, left to close it. Touch
screens get 44px controls and menu items (40px buttons), no tooltips or
hover-only controls, 16px fields (iOS zooms into smaller ones), and a tap on a
message shows its toolbar. Menus and pickers sit above drawers and dialogs
(`--layer-popover`). The app is installable (manifest,
`public/sw.js`), follows the keyboard on iOS, and keeps clear of the home
indicator.

Other pages use `.aw-page` (1040px) with a `PageHeader`: `--narrow` (720px) for
Home, `--form` (640px) for New project, `--settings` (880px) and `--wide`
(1280px) for tasks. Project settings are tabs (`PageTabs`, addressed by
`?tab=`), so each part is one short page. A task reads like a document: the
title and description are edited in place, a properties panel sits on the right
(under the title below 1100px), and a save bar appears at the bottom only when
there are unsaved edits.

## Components

In `src/ui/`: `Icon` (Lucide), `Button`, `IconButton` (with tooltip),
`Tooltip`, `Menu` (portal, arrow keys), `Dialog` (focus trap; `onSubmit` makes
it a form), `TextField`, `TextArea`, `SelectField` (all with an optional
leading `prefix`), `Checkbox`, `Switch` (for settings that apply at once),
`SegmentedControl`, `Avatar` (round for people, square for agents),
`AvatarStack`, `ActorTag`, `Badge`, `Dots`, `StatusChip`, `Banner` (takes a
ref, so an error can take focus; `onRetry` adds "Try again"), `EmptyState`,
`ErrorState` (`error` explains the cause in plain words, `onRetry` adds "Try
again"), `ConfirmDialog` (for what can't be undone; focus starts on Cancel),
`ConversationIntro`, `MessageSkeleton` (chat only), `PageHeader`, `Section`,
`PageTabs`, `ButtonLink`, `DetailList` (label and value pairs) and `RowList` +
`Row` (bordered lists of projects, members, repositories and links; a
`.aw-row__link` makes the whole row clickable). Text helpers: `countLabel`
("1 task", "3 tasks"), `plainPreview` (Markdown as one plain line for list
rows, quotes and toasts), `formatShortcut` ("⌘↩" on a Mac, "Ctrl+Enter"
elsewhere) and the date helpers in `ui/time.ts`.

Conversation pieces in `src/conversations/`: `ConversationHeader`,
`ConversationTimeline` + `TimelineEntries` (day dividers, New line, grouping,
hover actions, message links), `ActivityRow` for worker and system events,
`ReplyStatusList`, `StructuredComposer` (inline @mentions and # tasks),
`ThreadDialog`. Thread pieces in `src/threads/ThreadPresentation.tsx`:
`RunCard`, `ApprovalCard`, `RunBadge`, `ThreadDetails`. Task marks in
`src/work-items/WorkItemIcons.tsx`: `StatusIcon` (a circle that fills as work
moves from backlog to review, checked when done) and `PriorityIcon` (signal
bars, a red square for urgent). Loading and error states for project pages are
`ProjectPageLoading` and `ProjectPageError`.

## Motion

120ms hovers, 180ms menus and tooltips, 260ms dialogs and drawers, all eased
out. Motion reports state (the rail indicator, writing dots, a working agent's
ring). Every animation has a reduced-motion fallback.

## Copy

Plain, specific, sentence case. Name things the way people see them: "Mention
someone", "Start a thread", "Message #design". Agent states are plain verbs:
"Muse is writing a reply…", "Muse passed", "Muse handed this back". Errors say
what failed and what to do ("The reminder wasn't set. Try again."), never just
"That didn't work", and only blame the connection when it's the connection.
Never "Actor", "durable" or "AI", and never a harness, provider or model name
that isn't the data being shown (Claude and Codex are examples, not the
product).

One word for each thing:

| Thing | Say | Not |
| --- | --- | --- |
| Unit of work | task ("New task", "3 tasks") | work item, issue |
| Container | project, lowercase in a sentence | Project mid-sentence |
| Top level | workspace | organization |
| Agent work | run (Launch is only a button verb) | launch as a noun, worker |
| Who does it | agent | bot, worker, Actor |
| A drawing | whiteboard (board is the task board) | board |
| Sign-in name | username | login name |
| Waiting on a person | Needs you | needs attention, waiting for you |
| Spelling | canceled | cancelled |

Buttons are verbs: "New X" opens a form and its submit says "Create X"; Cancel
in a dialog is secondary. Toast titles have no full stop. Reversible actions
(archive, trash, mark done) happen at once with Undo in a toast; ones that
can't be undone (delete, revoke, remove someone) ask in a `ConfirmDialog`.
