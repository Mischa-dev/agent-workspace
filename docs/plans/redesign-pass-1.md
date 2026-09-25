# Redesign, pass one

Branch `redesign-pass-1`. Moves the web app onto the Agent Workspace design
system: a Discord-style rail with per-space sidebars, a Slack-style two-tone
look, and five presets where every color is a slot you can change. Not deployed
yet.

## What changed

- **Foundations.** New tokens (five presets, regions, type, spacing, radius,
  motion) generated from the design system, Figtree and JetBrains Mono
  self-hosted, Lucide icons instead of unicode glyphs, a new `src/ui/` component
  set (buttons, menus, dialogs, tooltips, fields, avatars, badges, banners,
  empty states), and the theme applied before first paint.
- **Shell.** Rail with Home, Messages (unread badge), each Project as a colored
  tile with the unread/current indicator, create, find (Cmd/Ctrl+K), Appearance
  and an account menu. Project sidebar with a space menu (new channel, archived
  conversations, settings), a collapsible Channels section, thread rows with
  Needs you / working / failed / done marks, and a New channel dialog. Messages
  sidebar with avatars and an Agents row. Phone drawers and a slimmer top bar.
- **Conversations.** Headers with a title menu, grouped messages with avatars
  and Agent/Worker tags, day and New dividers, hover actions (start or open a
  thread, copy a link to the message), highlighted mentions of you, intros at the
  start of each conversation, earlier-message loading, and a sticky-to-bottom
  timeline with a New messages jump button.
- **Composer.** One box with inline @mentions and # work item references, Enter
  to send, the character count only near the limit, and instant sends: the
  message shows dimmed right away and the draft comes back if the send fails.
- **Agents.** Reply status lines ("Muse is writing a reply…", passed, handed
  back, Ask again), the turn-limit banner in groups, run cards (state, branch,
  pull request and checks, Stop / Run again / Publish / Create pull request),
  approval and question cards, a Message / Run with Codex switch in threads, a
  thread details panel, and a redesigned Agents page with plain-language
  permissions and one-time tokens.
- **Sign-in and invitations** share a two-pane brand layout. **Archived
  conversations** lists archived channels and threads, and archived
  conversations open read-only with a banner and a locked composer.
- **Appearance page** (`/settings/appearance`): presets, main or every color,
  contrast warnings with Fix, shareable theme codes, density and text size, and
  a live preview of the shell.
- Docs: `apps/web/PRODUCT.md`, `apps/web/DESIGN.md` and
  `docs/frontend/theme-and-tokens.md` rewritten for the new direction.

## How it was checked

- `pnpm lint`, `pnpm typecheck`, `pnpm test` (131 tests, including the axe
  accessibility checks), `pnpm build`, `pnpm check:assets` all pass locally.
- Looked at every pass-one screen in a browser against a local backend with
  seeded data (Projects, channels, a thread, a work item, DMs and a group with
  agents replying, passing and mid-reply), in Indigo and Midnight, at desktop
  and phone widths. Run and approval cards were checked with a fixture thread,
  since the local backend had no trusted repository to run Codex against.

## Not done, and why

- **Red Hat was unreachable** over Tailscale all night, so everything was built
  and checked on the Mac instead. CI runs on the self-hosted Red Hat runner, so
  the PR's checks wait until that host is back.
- **No presence for people.** The server has no online/away state, so only your
  own avatar shows a dot.
- **No agent activity in the Messages sidebar.** The Messages overview doesn't
  include reply-request state, so "working" dots show inside the conversation
  but not on the sidebar row.
- **No mention counts per channel.** The sidebar API only says whether a channel
  is unread, so unread rows are bold without a count. Rail badges use Project
  attention counts and unread DMs.
- **Threads open as their own page**, not a side panel next to the channel, and
  the message a thread started from isn't shown inside it (the thread timeline
  doesn't include it).
- **No channel topics, channel member lists, in-conversation search,
  reactions, files or formatting.** None exist in the API yet; the design leaves
  room for them.
- **Themes are saved per browser.** Syncing to the account needs a server
  setting.
- **On phones the rail and sidebar are two drawers**, as before, rather than one
  combined drawer.
- **Size budgets were raised** to 560 KiB JS (176 KiB gzipped) and 104 KiB CSS.
  About 25 KiB of CSS is the legacy stylesheet, which goes away in pass two.

## Pass two

Home, Project overview, work items (list, board, detail), Project settings
(team, repositories, GitHub, Codex accounts, sponsored runs) and Create Project
still use the legacy stylesheet through token aliases.
They render inside the new shell and follow the theme, but haven't been
redesigned. Pass two moves them onto `src/ui/`, then deletes `src/styles.css`,
`src/styles/features.css` and the aliases in `tokens.css`.
