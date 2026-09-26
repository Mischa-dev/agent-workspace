# Redesign, pass two

Branch `redesign-pass-2`. Moves the rest of the web app onto the design system
from pass one, then deletes the pre-redesign stylesheet (`src/styles.css`,
`src/styles/features.css`) and the token aliases that kept it working.

## What changed

- **Home.** "Welcome back" with how many things need you across your Projects,
  then your Projects as rows (tile, key, when you last opened it, a "needs you"
  count). Notices, like "Beta Project was archived", show as banners.
- **Project overview.** The Project's tile, key and your role, then threads
  that need you (waiting on an answer, or failed), threads with work running
  when there are any, the channels, and the five newest work items.
- **Create Project.** One card with the name and key. The key is suggested from
  the name ("Design review" becomes DR) until you type your own, and the hint
  shows how work items will be numbered.
- **Project settings** are tabs: General (details, the viewer note, archive
  with a typed confirmation), Members (invite links, members with role and
  enable controls, invitation history), Agents, Codex (your Codex account and
  run requests from agents), Repositories (status per repository, registration
  form) and GitHub (installation, reachable repositories, automation switches,
  repository selection). Tabs you can't use aren't shown, and each tab has its
  own address (`?tab=`).
- **Work items.** A toolbar with search, type, status and priority filters, an
  archived toggle and a List / Board switch. List rows show priority bars, the
  key, a status circle, the title, the type and the assignee. The board has
  status headers with counts, cards that highlight the column you drag into,
  and move controls that appear on hover or keyboard focus. New work item is a
  dialog.
- **Work item detail.** The title and description are edited in place like a
  document. Status, priority, type and assignee sit in a panel on the right,
  with Open in Codex, Start a run (now a dialog) and Archive below it. The
  acceptance criteria are a checklist with progress, hover actions and inline
  editing. Links cover threads, repositories and pull requests, and the activity
  feed shows comments with avatars. A save bar appears only when there are
  unsaved edits.
- **Shared pieces** in `src/ui/`: `Checkbox`, `PageTabs`, `DetailList`,
  `RowList` / `Row`, a `prefix` slot on selects, `Dialog` forms with a set
  return-focus target, focusable `Banner`s, and new icons. Pass one's loading
  and error states for Project pages moved to `ProjectPageStates`.
- **Removed:** the legacy stylesheet (about 25 KiB), the token aliases, the
  Project context drawer (its details are on the pages now), `AppShell`'s legacy
  wrapper and the unused `SystemTimelineEntry`.
- **Server copy:** the generic 409 messages ("The request conflicts with current
  durable state.", "The resource changed after it was loaded.") now read "That
  conflicts with a change made elsewhere. Refresh and try again." and "Someone
  changed this after you opened it. Refresh and try again."

## How it was checked

- `pnpm lint`, typecheck, `pnpm test` (131 tests, including the axe checks),
  `pnpm build`, `pnpm check:assets` and `cargo check` pass locally.
- Looked at Home, the overview, the list, the board (including keyboard focus
  on a card), the detail page (adding and ticking a criterion, changing
  priority, saving, the Start a run dialog), Create Project and every settings
  tab against the local backend in Evergreen and Midnight, at desktop and phone
  widths.

## Not done, and why

- **Codex account states weren't seen live.** The local backend refuses to
  create a Codex profile without Codex set up on the Mac, so the signed-in,
  device-code and API-key states are covered by tests only.
- **GitHub's connected and setup states weren't seen live**, since GitHub isn't
  configured locally. Tests cover them.
- **"Needs you" on the overview had nothing in it locally**, so the waiting and
  working rows were only checked in code.
- **Work item properties still save with the title and description** (one Save
  for everything), not instantly the way Linear does. The server's conflict
  handling works per save, and changing that is a separate piece of work.
- **The assignee picker is still a select plus a search box**, not a single
  searchable picker.
- **Some activity text comes from the server** and reads "Work Item created"
  with a capital I.
- **Budgets:** CSS fell to about 92 KiB and its limit was lowered from 104 to
  100 KiB. JavaScript is about 553 KiB (175 KiB gzipped) against its 560 / 176
  KiB limits, so there's little room left before those need attention.
