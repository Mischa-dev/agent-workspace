# UI overhaul capture report

September 21, 2026. Read-only navigation of `https://workspace.mischanelson.dev` in the in-app browser, plus local source inspection at commit `2128a0f`. Desktop captures are 1280 × 720; the phone-layout capture is 390 × 844. The browser viewport was restored after inspection.

Screenshots remain in the private local folder `.local/ui-overhaul-audit-2026-09-21/`. They are current product evidence, not proposed designs. The linked files are local to this checkout and are not bundled into Git by this report.

The app has a consistent green identity and recognizable sections. The largest opportunities are readable text, navigation continuity, complete messaging behavior, and bringing frequently used work closer to the top of the screen.

| Step | State | Assessment |
| --- | --- | --- |
| 1 | Start here space | Shared material, channels, and projects are discoverable. The large preview displays raw Markdown syntax and leaves little useful material visible. |
| 2 | Ember conversation | Author, waiting state, context, and recipient are visible. Navigation occupies two left columns; text is small; the composer and empty timeline use space unevenly. |
| 3 | Projects with no projects | The initial action is clear. Repeated introduction and empty-state messaging dominate the screen. The empty-state text extends beyond the captured right edge and should be checked in layout tests. Populated views were not available in this live dataset. |
| 4 | Agents | Roles, last check-in, and actions are visible. Tall cards and decorative introductory copy consume space; operational state should have stronger hierarchy. |
| 5 | Inbox with pending requests | Review and waiting activity are separated. The large caught-up state pushes pending items down, and raw `pending` labels do not explain the next expected event. |
| 6 | Resources | Type, title, and revision are visible. The card grid provides limited scanning density and exposes Markdown markers in note excerpts. |
| 7 | Messages at phone width | The page fits horizontally, but six navigation items and a horizontal conversation strip crowd the layout. Computed labels are 7px, metadata 8px, messages 11px, and composer text 11px. |

## 1. Space

![Current space board](../.local/ui-overhaul-audit-2026-09-21/01-space.png)

Use formatted excerpts and shorter previews. Keep audience and revision information available. The adjacent channel/project area is useful and should remain part of the space experience.

## 2. Messages

![Current conversation](../.local/ui-overhaul-audit-2026-09-21/02-messages.png)

At this viewport, computed message text is 13px, composer text 12px, and conversation previews 10px. Activity dots currently derive from active requests; they do not mean unread. The source already stores message text drafts and scroll positions, so preserve and extend those behaviors.

## 3. Projects

![Current project empty state](../.local/ui-overhaul-audit-2026-09-21/03-projects-empty.png)

Replace the large introductory layout with a concise title, creation action, and useful initial task capture. Inspect populated Outline, Board, Dependencies, and Activity views in an isolated fixture environment during implementation.

## 4. Agents

![Current agent directory](../.local/ui-overhaul-audit-2026-09-21/04-agents.png)

Preserve role and check-in information. A compact directory and focused profile will make connection status easier to scan. Last-seen evidence does not establish that the configured client can complete real work.

## 5. Inbox

![Current inbox](../.local/ui-overhaul-audit-2026-09-21/05-inbox.png)

Use an inbox list with clear categories and a detail pane. A review count, an unread count, and a running-work count need separate meanings. Replace raw transport states with precise user-facing descriptions.

## 6. Resources

![Current resource library](../.local/ui-overhaul-audit-2026-09-21/06-resources.png)

Default to a sortable list with optional previews. Keep one durable Resource identity and distinguish its latest revision from the revision attached to a conversation or task.

## 7. Phone messages

![Current phone conversation](../.local/ui-overhaul-audit-2026-09-21/07-mobile-messages.png)

The document width matched the 390px viewport in this state, so this finding is about readability and interaction, not a demonstrated horizontal page overflow. Use a full conversation-list view, then a full conversation view with Back. Verify real device keyboard behavior separately.

## Source grounding and limits

- `apps/web/src/main.tsx` owns application state, snapshot refresh, event subscription, view-local filtering, navigation, custom modal behavior, and messaging. The inspected source has a modal focus loop, status/alert roles, text draft storage, and scroll restoration. Those are strengths to retain.
- `apps/web/src/components.tsx` contains project views, detail panels, Resource editing, task and approval forms, settings, and agent identity configuration.
- `apps/web/src/connected.tsx` contains spaces, planning views, Resource organization, and agent connection controls.
- `packages/shared/src/index.ts` defines the current conversation and message models. Owner read state, structured mentions, and personal favorites need new contracts.
- `apps/web/src/style.css` has 4,803 lines, several responsive groups, and later overrides. Existing reduced-motion CSS should be retained; JavaScript-triggered smooth scrolling also needs review.
- The hosted footer says Local workspace. Source ties that label to worker availability, so the proposed status design should distinguish hosting location, browser connection, worker health, and agent readiness.

No messages were sent, records created, credentials opened, work started, or production settings changed. The review did not test live sends, uploads, cancellation, approval, session expiry, screen-reader operation, keyboard completeness, contrast ratios, performance, or populated project flows. Their acceptance requirements belong in the overhaul plan; this report makes no compliance or performance claim.
