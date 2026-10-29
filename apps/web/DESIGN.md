# Design System

This is the reference every page follows. The live version is in the app at
`/design-system` ("See every component" on Appearance): every token and shared
component, in whichever theme you pick, so a change can be checked in all of
them at once. `docs/frontend/polish-checklist.md` is the short list a page
passes before its pull request is ready; `docs/frontend/theme-and-tokens.md`
explains how themes and overrides resolve.

## Rules

1. **Tokens, not values.** Colors, type, spacing, corners, shadows, focus rings
   and motion come from `src/styles/tokens.css`. `src/styles/designSystem.test.ts`
   reads every stylesheet and fails on a hand-typed color, font size or weight,
   an on-grid spacing length, a corner radius, a transition time or a focus
   ring drawn another way. A value that truly can't use a token (an emoji's
   size, a viewer that is always dark) says why on the same line:
   `font-size: 20px; /* design-exception: emoji */`.
2. **Kit before CSS.** Build pages from `src/ui` (exported from `ui/index.ts`).
   A new pattern that two pages need goes into the kit, with its states, and
   onto `/design-system`, before the second page copies it.
3. **One of each.** One primary button per view, one focus ring, one card
   surface, one empty state, one way to show an error. If a page needs
   something the kit doesn't have, add a variant rather than a lookalike.
4. **Every state.** Default, hover, pressed, focus-visible, disabled and busy
   for controls; loading, empty and error for content. Both themes, desktop
   and a 390px phone.
5. **Every part takes part in layouts.** A new user-facing part (a sidebar
   row, a tab, a section, a field on a task, a composer extra) registers in
   `src/experience/registry.json` with a label, a one-line description and a
   group, says which presets show it (`only` lists, or `hide` lists that
   leave it out), and checks `useProjectExperience(projectId).shows(part)`
   (or `useHomeExperience()` on workspace pages) before it renders. Hiding is
   visual only: never gate what someone may do on a layout. Nouns in copy
   use `experience.word(...)` where a layout can rename them.

## Theme

The default follows the device: "Match system" shows Daylight (light
everywhere, with a quiet grey sidebar so the conversation leads) in light mode
and Midnight (dark everywhere) in dark mode. Five presets ship, Daylight,
Midnight, Indigo (the earlier default: a deep navy rail and sidebar beside a
light reading surface), Evergreen and Graphite, plus classic editor themes (Nord, Dracula,
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
both self-hosted. Set type with one `font: var(--t-*)` shorthand; each follows
the text size setting through `--text-scale`. Sentence case everywhere; no
uppercase eyebrows.

| Style | Size / line | Weight | Use |
| --- | --- | --- | --- |
| `--t-display` | 28/36 | 700 | Document titles: a task, a page |
| `--t-headline` | 24/32 | 700 | Page titles (`PageHeader`) |
| `--t-title` | 18/24 | 650 | Dialog titles, page sections (`Section`) |
| `--t-heading` | 15/22 | 600 | Card titles, small sections |
| `--t-body` | 15/22 | 400 | Messages, descriptions, anything read |
| `--t-ui` | 14/20 | 500 | Controls, rows, menus |
| `--t-ui-strong` | 14/20 | 600 | Buttons, row titles, names |
| `--t-label` | 13/18 | 600 | Field labels, column headers |
| `--t-small` | 13/18 | 400 | Secondary lines, hints, meta |
| `--t-caption` | 12/16 | 500 | Timestamps, counts |
| `--t-caption-strong` | 12/16 | 600 | Group labels, small emphasis |
| `--t-tag` | 11/14 | 600 | Chips and tags |
| `--t-code` | 13/20 mono | 400 | Code blocks |
| `--t-code-sm` | 12/16 mono | 500 | Keys, branches, ids |

To change one part, follow the shorthand with a token:
`font: var(--t-small); font-weight: var(--weight-semibold);`. Weights are
`--weight-regular` 400, `--weight-medium` 500, `--weight-semibold` 600 and
`--weight-bold` 700 (titles only). Big titles take
`letter-spacing: var(--tracking-tight)`.

## Space, corners and depth

Spacing is a 4px grid: `--space-1` 4, `-2` 8, `-3` 12, `-4` 16, `-5` 20, `-6`
24, `-8` 32, `-10` 40, `-12` 48. The half steps `--space-half` 2, `--space-1-5`
6 and `--space-2-5` 10 are for dense controls (chips, menu items, rows). Lay out
siblings with `gap`, not margins. Between page sections 24 (`--space-6`), inside
a card 12–16, between a label and its field 6.

| Radius | px | Use |
| --- | --- | --- |
| `--radius-xs` | 4 | Tags, kbd, inline code |
| `--radius-sm` | 6 | Buttons, fields, menu items |
| `--radius-md` | 8 | Cards, row lists, menus, popovers |
| `--radius-lg` | 12 | Dialogs, page marks |
| `--radius-full` | — | Chips, avatars, badges, switches |

Depth is three shadows, each a theme slot: `--shadow-card` (a 1px lift for
cards and row lists in light themes, none in dark ones, where the border does
the work), `--shadow-popover` (menus, pickers, tooltips, toasts) and
`--shadow-dialog` (dialogs and drawers). Stacking uses the `--layer-*` tokens.

**Focus** is one ring everywhere: `outline: var(--focus-width) solid
var(--r-focus); outline-offset: var(--focus-offset)`, which `base.css` already
gives every link, button and field. Inside a box that clips its children use
`outline-offset: calc(-1 * var(--focus-width))` or `box-shadow:
var(--shadow-focus)`. Never remove an outline without drawing one of these.

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

The core set, with what each one is for. Their variants and states are all on
`/design-system`.

| Component | Variants and sizes | States | Notes |
| --- | --- | --- | --- |
| `Button`, `ButtonLink` | primary, secondary, ghost, danger; sm 28, md 32, lg 40 (36/40/44 on touch) | hover, pressed, focus, disabled (grey in every variant), busy (keeps its color, shows a spinner) | One primary per view. Ghost in rows, toolbars and section headers. Danger only to confirm what can't be undone. Labels are verbs. |
| `IconButton` | sm, md; `pressed` | hover, pressed, expanded, focus, disabled | `label` is its name and tooltip. |
| `TextField`, `TextArea`, `SelectField` | 40px; `compact` 32px for toolbars; `prefix`, `mono` | focus, invalid (`error` replaces `hint`), disabled | Always a visible label, or an `aria-label` in a toolbar. |
| `Checkbox`, `Switch`, `SegmentedControl` | — | checked, focus, disabled | Checkbox saves with a form; Switch applies at once; SegmentedControl switches views (arrow keys move). |
| `PageHeader` | `icon` tile or a `mark` (project, avatar) | — | Title, one sentence, actions on the right. Every page starts with one. |
| `Section` | `description`, `actions` | — | A page's titled parts. `actions` are ghost `sm` buttons. |
| `RowList` + `Row`, `DetailList` | — | hover when the row links somewhere | The card surface for lists. Never a card inside a card. |
| `.aw-card` | — | — | A single raised box (a form, a one-time token). |
| `StatusChip`, `Badge`, `ActorTag` | chip tones muted, agent, warning, success, danger; badge mention or neutral | — | Chips name a state; badges count; tags mark who (Agent, App). |
| `Banner`, `EmptyState`, `ErrorState`, skeletons | banner tones info, success, warning, danger, agent, offline | — | Loading is a skeleton shaped like the page; empty names the next step; errors say the cause and offer Try again. |
| `Dialog`, `ConfirmDialog`, `Menu`, `Popover`, `Tooltip`, toasts | — | open, focus trapped and restored | Escape closes what it opened. |

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

`--duration-fast` 120ms for hovers, `--duration-standard` 180ms for menus and
tooltips, `--duration-slow` 260ms for dialogs and drawers, all with
`--ease-out`; `--duration-fade` 1200ms for a highlight fading out. Motion reports state (the rail indicator, writing dots, a working agent's
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
