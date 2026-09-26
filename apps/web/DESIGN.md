# Design System

The full reference, with live components and every token per theme, is the
Agent Workspace design system artifact (private to the owner). This file is the
in-repo summary that code follows.

## Theme

Two-tone by default: a deep indigo rail and sidebar beside a light reading
surface (the Indigo preset). Five presets ship: Indigo, Midnight (dark
everywhere), Daylight (light everywhere), Evergreen and Graphite, plus "Match
system" (Indigo in light mode, Midnight in dark). Every color token is a theme
slot a person can override on the Appearance page; see
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
length capped at 760px. At 860px and below the rail, sidebar and details become
drawers and a top bar opens them. Conversation pages own their scrolling: header
and composer stay put, the timeline scrolls and stays on the newest message.

Other pages use `.aw-page` (1040px) with a `PageHeader`: `--narrow` (720px) for
Home, `--form` (640px) for Create Project, `--settings` (880px) and `--wide`
(1280px) for work items. Project settings are tabs (`PageTabs`, addressed by
`?tab=`), so each part is one short page. A work item reads like a document: the
title and description are edited in place, a properties panel sits on the right
(under the title below 1100px), and a save bar appears at the bottom only when
there are unsaved edits.

## Components

In `src/ui/`: `Icon` (Lucide), `Button`, `IconButton` (with tooltip),
`Tooltip`, `Menu` (portal, arrow keys), `Dialog` (focus trap; `onSubmit` makes
it a form), `TextField`, `TextArea`, `SelectField` (all with an optional
leading `prefix`), `Checkbox`, `Switch` (for settings that apply at once),
`SegmentedControl`, `Avatar` (round for people, square for agents, slate for
workers), `AvatarStack`, `ActorTag`, `Badge`, `Dots`, `StatusChip`, `Banner`
(takes a ref, so an error can take focus), `EmptyState`, `ErrorState`,
`ConversationIntro`, `MessageSkeleton`, `PageHeader`, `Section`, `PageTabs`,
`DetailList` (label and value pairs) and `RowList` + `Row` (bordered lists of
Projects, members, repositories and links; a `.aw-row__link` makes the whole
row clickable).

Conversation pieces in `src/conversations/`: `ConversationHeader`,
`ConversationTimeline` + `TimelineEntries` (day dividers, New line, grouping,
hover actions, message links), `ActivityRow` for worker and system events,
`ReplyStatusList`, `StructuredComposer` (inline @mentions and # work items),
`ThreadDialog`. Thread pieces in `src/threads/ThreadPresentation.tsx`:
`RunCard`, `ApprovalCard`, `RunBadge`, `ThreadDetails`. Work item marks in
`src/work-items/WorkItemIcons.tsx`: `StatusIcon` (a circle that fills as work
moves from backlog to review, checked when done) and `PriorityIcon` (signal
bars, a red square for urgent). Loading and error states for Project pages are
`ProjectPageLoading` and `ProjectPageError`.

## Motion

120ms hovers, 180ms menus and tooltips, 260ms dialogs and drawers, all eased
out. Motion reports state (the rail indicator, writing dots, a working agent's
ring). Every animation has a reduced-motion fallback.

## Copy

Plain, specific, sentence case. Name things the way people see them: "Mention
someone", "Start a thread", "Message #design". "Project" is capitalized as the
product's name for a workspace; "work item", "thread" and "channel" are not. Agent states are plain verbs:
"Muse is writing a reply…", "Muse passed", "Muse handed this back". Errors say
what happened and what to do. Never "Actor", "durable" or "AI".
