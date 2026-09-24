# Workspace UI overhaul

Proposed plan, September 21, 2026. Based on the live hosted app and local source at `2128a0f`. This document plans the overhaul; it does not authorize implementation or deployment. It complements the product and launch plans without changing their execution or access boundaries.

The goal is a workspace that feels finished every time Mischa opens it: clear navigation, readable conversations, dependable drafts, immediate feedback, and consistent controls across messages, shared material, projects, and review. Slack is the main reference for everyday messaging and navigation. Linear is a secondary reference for compact project work.

The first implementation milestone should be one complete messaging flow inside the new shell. Use that working experience to establish the quality bar before migrating every other screen.

## What the current app needs

The existing green identity, durable Resources, scoped agent participation, task views, draft text storage, scroll restoration, and event connection provide useful foundations. Preserve those behaviors while rebuilding the UI around them.

| Current observation | Consequence | Planned change |
| --- | --- | --- |
| Messages has a 210px application sidebar plus a 247px conversation list at the inspected desktop size. | Navigation consumes substantial width before a thread or document opens. | A compact application rail and one contextual sidebar, both with clear collapse rules. |
| Large slogans and sparse card layouts lead Resources, Projects, Agents, and Inbox. | Screen space goes to introductions instead of everyday work. | Short page titles, useful toolbars, compact lists, and empty states placed where content belongs. |
| Desktop message text is 13px and the composer is 12px. At 390px wide, message and composer text are 11px; navigation labels are 7px. | Reading and tapping require too much effort. | A defined type scale and phone-specific navigation. |
| Search filters loaded record metadata in the current view. | Finding an earlier message or note does not work like a workspace search. | Global search with previews and links to the exact result, plus separate view filters. |
| Navigation uses component state and local storage. Thread opening navigates to another conversation. | Links, browser history, and following a side discussion lack a consistent model. | URL-backed navigation and a thread panel with a recoverable parent location. |
| The data model has no owner read cursor, unread counts, or personal favorites. | Activity dots cannot tell the owner what is new or needs reading. | Durable read state, mention references, and personal sidebar preferences. |
| Message draft text persists, but attachment selections, recipient, mode, reply, and correction state are component state. | Switching away can lose part of the intended message. | A complete draft model per conversation and thread. |
| The frontend spans three large TSX files and 4,803 lines of CSS with repeated overrides. | A visual fix can have unexpected effects elsewhere. | Shared components, semantic tokens, and feature modules, migrated incrementally. |

The [capture report](UI_OVERHAUL_AUDIT.md) records seven inspected states and the limits of this assessment. The hosted project list was empty, so populated project behavior was inspected in source rather than exercised against production data.

## Design direction

Mischa uses this for sustained desktop work and quick phone checks. Start with a light reading area, a darker green navigation rail, neutral sidebar surfaces, and a restrained green accent. Include light, dark, and system appearance in the completed overhaul. Preserve the existing identity while increasing contrast and simplifying the layout.

Use one system sans family. Proposed starting sizes are 14px controls and navigation, 15px conversation text, 12px secondary metadata, and 20–24px page titles. Phone messages and inputs start at 16px. Compact mode reduces spacing rather than shrinking text. Final colors must pass contrast checks in both themes.

Use a 4px spacing base, 32–36px desktop controls, and 44px primary touch targets. Use 6–8px control corners and modest panel corners. Keep rows flat; use cards for material previews when the preview is useful. Reserve shadows for floating elements. Standardize icon size, stroke, alignment, and accessible labels using the existing Lucide library.

Transitions should explain a change in roughly 120–200ms. Reduced motion removes movement. Content should appear immediately without page entrance choreography.

Reference patterns are grounded in Slack's [navigation and sidebar documentation](https://slack.com/help/articles/212596808-Adjust-your-sidebar-preferences), [keyboard shortcuts](https://slack.com/help/articles/201374536-Slack-keyboard-shortcuts), and [search documentation](https://slack.com/help/articles/202528808-Search-in-Slack.), plus Linear's [board and list behavior](https://linear.app/docs/board-layout). These support the interaction direction; the proposed measurements and layout below are Workspace decisions.

## The application shell

| Area | Proposed behavior |
| --- | --- |
| Global bar | 44–48px high. Back/forward, workspace-wide search with a visible shortcut hint, and a compact create menu. Keep view filters in their own view. |
| Application rail | About 64px wide. Messages, Inbox, Resources, Spaces, Projects, and Agents with icons and readable short labels. Settings and account controls at the bottom. No multi-workspace switcher until the product actually supports multiple workspaces. |
| Context sidebar | Default about 248px, resizable within 220–320px. Messages shows Favorites, Channels, Direct messages, and Groups. Projects and Spaces show their relevant navigation. Persist collapsed sections and width. |
| Main area | A compact title and action row followed by the actual work. Conversations use available width; long prose has a readable line length. Tables and boards can use the full area. |
| Right panel | One panel host for a thread, task, Resource, agent profile, or review. Default about 380px, with resize and expand controls. Switching panel contents preserves drafts and provides a back action where necessary. |

At a 1280px desktop width, opening a right panel should collapse the contextual sidebar when needed to retain a usable main area. Never squeeze four columns into a narrow viewport. Wider screens may show all columns. Panel expansion can take over the main area without losing its origin.

Use real links for destinations and URL state for selected entities, project views, and panels. Proposed routes include `/messages/:conversationId`, `/projects/:projectId`, `/resources/:resourceId`, and `/spaces/:spaceId`. A message deep link identifies its parent conversation and message; a thread link includes its root. Refresh and browser back must restore a meaningful location. The server must support authenticated app-route fallback.

On phones, show one work area at a time. Conversation list opens conversation; thread opens a separate view with a back button. Use four bottom destinations, Messages, Inbox, Projects, and More. Resources, Spaces, Agents, and Settings remain labeled and reachable from More. Keep search accessible at the top. Account for safe areas, the software keyboard, and dynamic viewport height.

## Make messaging feel complete

The acceptance journey is: open a channel, read new activity, write a message with an attachment, follow a thread, inspect a saved note, and return to the original draft and reading position.

| Part | Required behavior |
| --- | --- |
| Conversation list | Favorites and collapsible groups; unread weight and counts; draft indicator; selected state distinct from unread; useful timestamps and previews; stable ordering while reading. |
| Timeline | Group consecutive messages from one author, date separators, clear timestamps, unread divider, readable Markdown, selectable text, code copy, attachment previews, and an anchored link to each message. |
| Long history | Load older messages in pages. Preserve the visible message and pixel offset when prepending history. New activity does not pull the reader away from older messages. Offer a new-message indicator and Jump to latest. |
| Message actions | Reply in thread, quote/reply, copy link, save as Resource, forward, and the existing correction action. Reveal on hover, keyboard focus, or a touch menu. Keep ordinary message deletion and reactions outside the first release unless separately scoped. |
| Thread panel | Root message plus replies, dedicated composer and draft, reply count, back/close, and expand. Opening an existing thread is a read operation; create a new thread only when the owner sends its first reply. Preserve existing thread IDs and context rules. |
| Composer | Auto-growing input, visible focus, formatting help, keyboard-accessible mention picker, attachment chips and upload progress. Show the selected recipient without making routing settings dominate the composer. Advanced group modes sit behind a labeled control. |
| Draft continuity | Store text, attachment revision IDs, recipient, group mode, reply target, and correction target together. Restore after navigation and reload. Preserve drafts on failure or sign-in interruption. If an attachment becomes unavailable, identify it and require a deliberate change. |
| Sending | Render a local pending message immediately, then reconcile it with the server receipt. Show Sending, Sent, or Failed. Reuse one command ID and the same payload for retries after uncertain delivery. Disable duplicate submission synchronously. Clear only the submitted draft version, not text typed while a send is completing. |
| Agent progress | Distinguish waiting for check-in, acknowledged, working, stopped, failed, and needing review. Derive these from observed events and timestamps. An expected check-in interval is not a promise of a response time. |
| Stop and correction | Keep Stop available while work is pending or active. Its scope must be clear. Distinguish stopping new work from confirmed cancellation of already-started work. Retain correction lineage and uncertainty when earlier output predates a correction. |

Default to Enter to send and Shift+Enter for a newline, with a preference for Cmd/Ctrl+Enter. Respect IME composition. No shortcut should submit a form while a picker is using Enter to choose an item.

Keep the current safe text contract initially. Improve rendering and compose affordances without replacing stored messages with a new rich-text document format. If rich-text authoring becomes necessary, specify its serialization and agent round trip as a separate change.

## Apply the same quality across the workspace

| Area | Planned experience | Completion example |
| --- | --- | --- |
| Inbox | Needs you, Updates, and History filters. Rows explain who or what needs attention and the next action. Selecting a row opens the relevant review panel. Keep waiting requests distinct from work actually running. | Review an output, open its evidence, return to the same row, and record a decision. Reading or dismissing an update never accepts work. |
| Projects | Compact project header; Outline, Board, Dependencies, and Activity views over the existing tasks. Persist view, filter, and selection. Use the same task detail panel everywhere. Show blockers in ordinary language. | Create a task with a title, add details later, change views, and open its linked conversation without losing project context. |
| Task editing | Inline title and status edits where permitted; properties in a consistent detail panel; advanced execution settings progressively disclosed. Any drag action has a keyboard and menu equivalent. | Invalid transitions show the actual reason and leave the existing task intact. |
| Resources | Searchable list as the default, optional previews, type and audience filters, sorting, and a full reader/editor. Show title, type, updated time, audience, and revision without opening every item. | Open a pinned attachment revision, compare it with current material, edit current content, and retain the original attachment identity. |
| Spaces | A clear home for a subject or project area. Group shared material, channels, and projects. Use compact material previews with formatting and useful titles. | Post or link a Resource with its actual audience visible. Membership in a space never silently grants access. |
| Agents | Compact directory with name, role, connection state, last check-in, and Message. Details open a profile. Connection setup has clear steps and a real readiness result. | A configured polling identity is visibly different from an agent with a successful client round trip. |
| Settings | Dedicated settings area with General, Appearance, Notifications, Connections, and Advanced. Use the same field, validation, and save patterns throughout. | A validation error preserves all entered values. Reopening settings returns to the relevant category. |
| Sign-in and recovery | Plain sign-in, useful error text, clear reconnecting state, and session-expiry recovery that preserves local work. | An expired session does not discard a draft or pretend an uncertain command failed safely. |

Pending notification work covers in-app unread, mentions, and review activity. Desktop push, email, and background phone notifications are later extensions requiring their own permission and delivery design.

## The shared component system

Create semantic tokens for backgrounds, text, borders, accent, selection, focus, and status in both themes. Define spacing, type, control heights, corner sizes, layering, and motion alongside them. Record the chosen values and component rules in `apps/web/DESIGN.md` during implementation.

Build a small reusable set: buttons and icon buttons, text fields and textareas, searchable selects, checkboxes, tabs, menus, tooltips, popovers, dialogs, sheets, split panes, avatars, badges, list rows, toasts, inline errors, skeletons, empty states, the composer, attachment tiles, and the detail panel host. Each must cover default, hover, focus, pressed/selected, disabled, pending, and error states when relevant.

Use one mature accessible interaction library for menus, dialogs, popovers, and composite inputs, chosen after a React/Vite compatibility check. Avoid maintaining custom focus and menu logic for every screen. Retain the current stack and icon set; a framework rewrite is unnecessary for this plan.

Keep a local component gallery covering both themes, narrow widths, long text, keyboard operation, and failure states. Extract components as the actual screens migrate rather than creating a large unused library first.

## Supporting application work

These are functional requirements, not CSS tasks. They should appear explicitly in implementation scope.

| Capability | Supporting change | Boundary to preserve |
| --- | --- | --- |
| Links and history | Router, canonical entity URLs, panel/view query state, server app-route fallback, scroll and focus restoration. | A copied URL grants no access. Missing or inaccessible records produce a useful destination state. |
| Unread and mentions | Owner read cursors, structured mention references, attention counts, and mark-read/mark-unread commands. Persist across devices. | Track the last visible message, not merely that the conversation mounted. Agent-authored or delayed events must not corrupt read order. Unread is separate from acceptance. |
| Favorites and preferences | Owner-scoped favorites, order, collapsed groups, appearance, and density. Browser-local storage is sufficient for panel widths. | Label device-only preferences where relevant. Clear private caches appropriately on sign-out without silently deleting drafts. |
| Global search | Permission-checked query endpoint over message text, Resource text and titles, projects, and tasks; pagination, excerpts, and result links. Start with PostgreSQL capabilities. | Respect current audience and revision rules. Search snippets must not expose inaccessible content. No external search service is required by this plan. |
| Timeline loading | Bounded message-history reads and stable pagination, plus a dedicated client cache. Evaluate UI windowing with measured long conversations. | Preserve ordering, root/reply identity, exact revision pins, and accessible history navigation. |
| Fast updates | Normalize records and index by kind/ID; keep existing server events; update affected data where feasible and resynchronize on gaps. Replace app-wide busy state with per-action pending state. | Keep command idempotency, expected revisions, access checks, and snapshot reconciliation. UI optimism never means work was approved or executed. |
| Review history | Durable owner review/dismissal state for inbox updates when existing domain records cannot represent it. | A dismissed update cannot erase an unresolved task, proposal, or execution decision. |

Complete draft persistence initially remains on the same browser. Cross-device draft synchronization is deferred and must not be implied by a Saved label. Drafts should be keyed by owner/workspace and conversation, with a versioned format and safe migration of current text-only drafts.

Search and history endpoints should coexist with the current snapshot during migration. Measure payload and render cost before choosing how far to change the snapshot model. This plan does not require replacing the entire transactional store.

## Implementation sequence

Each package produces working behavior and a reviewable local preview. Effort labels are relative planning sizes, not time estimates.

| Package | Scope | Depends on | Exit condition |
| --- | --- | --- | --- |
| UI-01 · Interaction and visual specification | Finalize shell, message/thread view, inbox detail, project view, Resource reader, phone flow, and state inventory in one coherent direction. Specify route and data contracts. Medium. | This proposed plan | The same conversation is specified at desktop, narrow desktop, and phone widths, including loading, failure, unread, and reconnecting states. |
| UI-02 · Foundations and shell | Tokens, component gallery, accessible controls, themes, routing, application rail, sidebar, panel host, focus and history rules. Large. | UI-01 | Navigate, reload, open a link, use Back, resize, and open/close a panel without losing location. Other screens remain reachable. |
| UI-03 · Messaging | Timeline, composer, full drafts, safe optimistic send, threaded side panel, attachment flow, progress and failure states, bounded history. Large. | UI-02 | The complete messaging journey passes at desktop and phone sizes, including uncertain send and reconnect. |
| UI-04 · Search and attention | Global search, favorites, owner read cursors, mentions, unread divider, activity counts, in-app notification preferences. Large. | UI-02 and message/link contracts from UI-03 | Find an old message by its body, open its exact location, and keep read state correct across two sessions and incoming messages. |
| UI-05 · Projects, Resources, Spaces | Migrate planning views, task details, material library/editor, attachments, and space navigation into shared components. Large. | UI-02; UI-03 links and drafts | Move between conversation, task, and Resource with preserved context; pin revisions and retain permissions. |
| UI-06 · Review, agents, settings | Inbox triage, review panels, agent directory/profiles, connection setup, settings, sign-in and session recovery. Large. | UI-02 and UI-04 attention model | Every required decision has a clear path; waiting, disconnected, working, and review states are accurate. |
| UI-07 · Qualification and release candidate | Cross-screen visual review, keyboard and screen-reader work, responsive/device testing, measured performance, cleanup of old CSS/components, rollout and rollback checks. Large. | UI-03 through UI-06 | The quality gate below passes, with remaining limitations explicitly recorded. |

The sequence is intentional. Finish the shell and messaging journey early enough to judge the feel before reproducing it across the rest of the product. Mobile and accessibility are acceptance conditions within every package; UI-07 performs the combined pass.

Recommended code boundaries are `app/` for routing and shell, `ui/` for shared controls, `features/messages/`, `features/inbox/`, `features/projects/`, `features/resources/`, `features/spaces/`, `features/agents/`, `features/settings/`, `data/` for queries/commands/events, and `styles/` for tokens and shared rules. Extract from the current files in working slices. Do not move every file first and postpone behavior checks.

## The quality gate

These are proposed acceptance targets, not current performance or compliance claims.

| Dimension | Required evidence before completion |
| --- | --- |
| Visual consistency | Review all six main sections, settings, sign-in, dialogs, and panels in both themes. Consistent controls, typography, spacing, truncation, status language, and selected states. No final override pile in CSS. |
| Navigation | Deep links, refresh, Back/Forward, sidebar collapse, panel transitions, and deleted/inaccessible targets tested. Switching context preserves the prior draft, selection, filters, and reading position. |
| Message reliability | Test double submit, IME Enter, new typing during pending send, lost response after server commit, reconnect, failed upload, stale attachment revision, correction, and stopped conversation. No duplicated message or unintended second agent admission. |
| Accessibility | Target WCAG 2.2 AA. Automated checks plus keyboard-only journeys and VoiceOver review. Verify names, landmarks, selected states, focus return, dialogs, menu keys, live status announcements, reduced motion, contrast, zoom, and reflow. Announcements must not read an entire refreshed timeline repeatedly. |
| Responsive behavior | Review 1440 and 1280 desktop, 1024 narrow desktop, 768 tablet, 390 phone, and 320 narrow phone layouts. Test real phone keyboard behavior and safe areas separately from viewport simulation. No app-wide horizontal scroll; code and boards can scroll inside their own region. |
| Performance | On a recorded reference device, target visible feedback within 100ms for local actions and warm conversation switches within 150ms at p95. Network completion is measured separately. Target field INP at or below 200ms where measurable. Record baselines before claiming improvement. |
| Representative volume | Isolated fixtures with at least 100 conversations, one 10,000-message history, a 1,000-task project, and long names and documents. Confirm bounded message reads, usable search, stable scrolling, and no full-history DOM requirement. Adjust scope if measurements expose store limits. |
| Recovery | Empty, first load, refresh, stale, reconnecting, unavailable, no access, invalid input, conflict, and success states exist for each flow. Keep the prior content visible during background refresh. Errors say what was retained and what the owner can do next. |
| Existing behavior | Continue passing `./workspace typecheck`, `./workspace test`, and `./workspace build`. Add focused UI and integration coverage for the changed journeys, commands, read state, search permissions, and retry behavior. Use isolated data with live agent dispatch disabled for UI fixtures. |

WCAG acceptance should use the [current W3C recommendation](https://www.w3.org/TR/WCAG22/). The performance target follows the [INP guidance](https://web.dev/articles/inp). Automated accessibility checks and screenshots alone cannot establish conformance.

## Rollout and scope

Implement on an isolated branch with a local preview and fixture data. Keep the old UI reachable during the migration. Use additive schema changes and versioned draft migration so rollback does not erase data. Before any hosted release, review the complete candidate, verify backup/restore compatibility, and prepare a deploy and rollback procedure against the actual hosted version.

The finished UI must continue to distinguish accepting a result from integrating code, sending a message from starting approved work, and configuring an agent from proving it is connected. Those are core product behaviors.

Defer reactions, calls, huddles, a plugin marketplace, multi-workspace administration, SSO, native desktop packaging, and push notifications. They are separate product expansions. Deep AgentOS and GitHub execution work remains governed by the existing roadmap.

This overhaul is complete when daily use has the same level of care across the main workspace and its failure states. A polished messaging screen alone is the first milestone, not the completion claim.
