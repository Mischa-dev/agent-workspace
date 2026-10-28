# Polish checklist

Every page passes this before its pull request is marked ready. It is short on
purpose: walk your own pages once in a browser (desktop and a 390 px phone,
light and dark) and fix what fails. Workstreams that land after W13 run it on
their own pages; nobody else will.

## States

1. **Loading** shows a skeleton shaped like the page from `ui/skeletons.tsx`
   (`ListSkeleton`, `SettingsSkeleton`, `PageSkeleton`, `DocumentSkeleton`),
   never a bare "Loading…" line or a spinner over a blank page.
2. **Empty** uses `EmptyState` and names the next step with a button or link
   to it ("Connect GitHub", "New task"), not just "Nothing here".
3. **Error** uses `<ErrorState error={query.error} onRetry={…}>` or a `Banner`
   with `onRetry`, so it says the real cause (offline, gone, no access, server
   trouble) and offers "Try again". No screen tells you to reload the page.
4. **Saving** disables only the control that's busy, and a failure says what
   failed in words (the API's message, not a status code).

## Content

5. **Dates and times** come from `ui/time.ts` (`formatRelative`,
   `formatDateTime`, `formatShortDate`, `dayLabel`, and `formatRelativeShort`
   for right-aligned time columns), with `formatFullDate` only in a tooltip or
   `title`, never inline. No `toLocaleString` in a page.
6. **Agents and providers are named by their data.** Show the agent's display
   name and the harness or provider the run used. Never hard-code "Codex",
   "Claude" or a model name in copy; they are examples, not the product.
7. **Words** are plain and sentence case and use the one vocabulary in
   `apps/web/DESIGN.md` (task, project, run, agent, whiteboard): buttons are
   verbs ("Launch 3 tasks"), headings are nouns, counts come from `countLabel`
   ("1 task"), Markdown shown as a preview goes through `plainPreview`, and
   nothing says "Error 500", "null", an enum value or an internal id.

## Markup

8. **Use the kit and the tokens.** Controls, headers, sections, rows, chips and
   dialogs come from `ui/index.ts`, used the way `apps/web/DESIGN.md` says (one
   primary button, ghost actions in section headers, rows instead of a card in
   a card). No raw `<button>`, `<input>` or `<select>` in a page. New styles
   use the tokens; `src/styles/designSystem.test.ts` fails on hand-typed
   colors, type, spacing, corners, focus rings and timings. Check new pieces
   on `/design-system` in a light and a dark theme.
9. **Keyboard**: every action is reachable with Tab, the focus ring shows,
   dialogs trap and restore focus, and Escape closes what it opened.
10. **Accessible names**: icon buttons have a `label`, lists and regions have a
    heading or `aria-label`, and live counts use `aria-live="polite"`.

## Layout

11. **Phone (390 px)**: no horizontal scroll, touch targets at least 44 px,
    tables become stacked rows or scroll inside their own box, and sticky bars
    don't cover the keyboard.
12. **Dark and light** both read: no text on a background the theme didn't
    choose.

## Code

13. **Size**: a page file stays under about 500 lines; split parts into their
    own files before it grows past that.
14. **Weight**: new pages are lazy routes. Anything in the initial load (shell,
    Home) stays lean, and `corepack pnpm check:assets` passes.
15. **Errors are reported**: unexpected errors reach `AppErrorBoundary`, which
    sends them to telemetry; don't swallow them in a `catch` that only logs.

## Layouts

16. **Every part takes part**: a new user-facing part is in the layout
    registry (`src/experience/registry.json`) and its presets say whether they
    show it; the page hides it when the layout does, and the nouns a layout
    renames (task, milestone, page, channel) come from `experience.word`.
    Check the page with Software team, Personal life and Bug reporting.

## Areas

The areas below passed this list in W13 (#149). A workstream that adds or
reshapes pages in an area re-runs the list for those pages.

| Area | Pages |
| --- | --- |
| Home and onboarding | Home, first-run setup |
| Conversations | Channels, DMs, threads, inbox |
| Tasks | List, board, detail, milestones, views |
| Runs and launch | Launch dialog, launches, run cards; Runs list and run page (W12) |
| Settings | Account, workspace, project |
| Agents | Agent settings, external agents, runners; project agents and harnesses (W12) |
