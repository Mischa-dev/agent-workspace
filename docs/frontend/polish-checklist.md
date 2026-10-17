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
   to it ("Connect GitHub", "Create a task"), not just "Nothing here".
3. **Error** uses `ErrorState` or a `Banner` with a retry. No screen tells you
   to reload the page.
4. **Saving** disables only the control that's busy, and a failure says what
   failed in words (the API's message, not a status code).

## Content

5. **Dates and times** come from `ui/time.ts` (`formatRelative`,
   `formatDateTime`, `formatShortDate`, `dayLabel`), with `formatFullDate` in a
   tooltip or `title` where the short form hides the day. No
   `toLocaleString` in a page.
6. **Agents and providers are named by their data.** Show the agent's display
   name and the harness or provider the run used. Never hard-code "Codex",
   "Claude" or a model name in copy; they are examples, not the product.
7. **Words** are plain and sentence case: buttons are verbs ("Launch 3 tasks"),
   headings are nouns, and nothing says "Error 500", "null" or an internal id.

## Markup

8. **Use the kit.** Controls, headers, sections, rows, chips and dialogs come
   from `ui/index.ts`. No raw `<button>`, `<input>`, `<select>` or ad-hoc
   colours in a page; new styles use the tokens in `theme-and-tokens.md`.
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

## Areas

The areas below passed this list in W13. A workstream that adds or
reshapes pages in an area re-runs the list for those pages.

| Area | Pages |
| --- | --- |
| Home and onboarding | Home, first-run setup |
| Conversations | Channels, DMs, threads, inbox |
| Tasks | List, board, detail, milestones, views |
| Runs and launch | Launch dialog, launches, run cards |
| Settings | Account, workspace, project |
| Agents | Agent settings, external agents, runners |
