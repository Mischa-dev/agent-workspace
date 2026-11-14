# C2: Chat, inbox and the daily feel

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner's draft, 2026-11-14, against `agent-workspace` main at fe95a74. Read-only: nothing was
built or run. "Inferred" marks a claim I reasoned from code without tracing every caller.

## 1. Goal

A team of people and agents can live in this app all day the way they live in a good team chat
product: the unread dot and the badge are always right, a DM or a reply in a thread you're in
always reaches you (and nothing else does unless you asked), channels can be private, joined,
left, arranged and muted, and the inbox is a triage queue you can clear with one hand. An idea in
chat becomes a task, decision, page, sheet row, whiteboard sticky or record in two clicks, with a
"made from" chip both ways. Search finds what you remember with filters you'd expect. On a phone,
every chat and inbox flow works with a thumb, and a request can be answered from the
notification. Agents show up in all of this as accounts doing work, visibly, and stoppable.

## 2. Where the code is today

### 2.1 Conversation kinds and private channels

- One table, `conversations`, `kind IN ('channel','thread','dm','group')`, with
  `visibility IN ('project','members')` (`migrations/0037_direct_conversations.sql:20-62`).
  DMs and groups must be `members` (`0037:43-60`); channels may be either; threads follow their
  parent (`0037:158-180`). Threads may hang under a DM or group since 0042.
- `conversation_members` (`0037:139-152`) holds who is in a `members` conversation. Its rows
  reference `project_memberships`, so a member must be in the conversation's project.
- The one visibility rule is `conversation_visible_to(target, viewer)` (`0037:158-180`). It
  answers only "project-wide, or on the member list" and does not check project membership
  (health check B finding 6).
- DMs and groups live in one hidden `kind = 'direct'` project per workspace (`0037:12-18`;
  `direct.rs:1042-1080` `space_project`). Only people can open the direct space:
  `ensure_direct_space` refuses non-person callers (`direct.rs:982-1000`). K4 moves DMs out.
- **"Private channels half built", exactly:**
  1. The schema allows a `members` channel (`0037:33-39`), the reading paths that go through
     `lock_conversation_scope` / `fetch_conversation_access` honour it (`conversations.rs:3805-3858`),
     and roles can name "private channels they join when they arrive"
     (`roles.rs:1842-1880` `apply_arrival_defaults`).
  2. Nothing can create one: `insert_channel` never sets `visibility` (`conversations.rs:953-996`),
     and HTTP `create_channel` takes only a name (`conversations.rs:868-950`).
  3. Nothing can add or remove channel members: the only `conversation_members` writers are the
     group code in `direct.rs` (`insert_members`, `direct.rs:1310`; `change_group_members`,
     `direct.rs:545`) and the role default above.
  4. **Two listing queries would leak a private channel the day one exists.** `list_channels`
     (`conversations.rs:795-822`) and the project sidebar's channel query
     (`conversations.rs:3419-3438`) select every `kind = 'channel'` row of the project with no
     `conversation_visible_to`. The sidebar's thread query (`conversations.rs:3445-3520`) also has
     no visibility filter and joins threads to any parent, including DMs and groups.
  5. **Possible leak today (medium confidence, verify with a test first):** the direct project's
     id is sent to the browser (`direct.rs:89-95`, `DirectOverview.project_id`). Any person with a
     membership in the direct space can call
     `GET /api/v1/projects/{directProjectId}/conversation-sidebar`, which passes
     `authorized_project` (`conversations.rs:3686-3735`, no `kind = 'direct'` refusal) and then
     lists the names of every thread under every DM and group in the workspace. Thread names are
     taken from message text (`conversations.rs:4867-4930`). No test covers the sidebar on the
     direct project (`workspace_isolation.rs:2762-2776` covers cross-workspace only).
- There is no joining or leaving: every member of a project is effectively in every channel of
  it, and the sidebar lists them all (`shell/ProjectSidebar.tsx:182-229`). No topic, description,
  channel details or tabs: `conversations` has no such columns (columns added over time:
  `0005:13-15`, `0037:25-28`, `0038:14`, `0042:79-83`, `20261028070000:53`). The header has
  Start a thread, notification level, archive (`conversations/ChannelPage.tsx:282-317`) and a
  pinned bar (`memory/ConversationMemory` `PinnedBar`, `ChannelPage.tsx:320`).

### 2.2 Unread state, and where it can be wrong

- Read state is one high-water mark per person and conversation:
  `conversation_read_states.highest_seen_sequence` (`0005:442-517`), versioned, refusing a
  backward move unless `aw.read_rewind` names it (`conversations.rs:2853-2963`).
- "Unread" everywhere is `next_entry_sequence - 1 > highest_seen_sequence`: channels
  (`conversations.rs:822-866`, `3546-3547`), threads (`3675`), DMs and groups (`direct.rs:226`),
  agents' briefing (`mcp/briefing.rs:174`). It is a boolean; there are no counts.
- Web: a page marks seen when it shows something new and its tab is visible
  (`conversations/useAutoMarkSeen.ts:28-87`). The New line and "N new" count exclude your own and
  removed messages (`conversations/useFirstUnread.ts:37-52`). Mark unread from a message is shared
  between web and MCP (`mark_unread_from`, `conversations.rs:2853`).

Where it is wrong or can drift (each becomes a rule in section 3.3):

| # | Problem | Evidence |
| --- | --- | --- |
| U1 | Every timeline entry counts, not only messages: a run's worker event or a system entry after you read makes a thread or channel bold | `next_entry_sequence - 1` above; `mark_unread_from` already works around it for "newest" (`conversations.rs:2878-2890`) |
| U2 | Your own message makes the conversation unread for you when it was sent anywhere but an open page: through an app or agent tool, a push-notification reply, a scheduled send, a forward, the share sheet. The send pipeline never moves the author's mark | no read-state write in `append_message_command_with` (`conversations.rs:1695-2068`; the only writers are listed by `grep conversation_read_states`) |
| U3 | A removed message keeps its entry, so a conversation whose newest message was removed stays bold though the page shows nothing new | sequence-based unread above; removal keeps the entry (`message_edits.rs:1-14`) |
| U4 | Reading through an app or agent tool leaves inbox notifications unread, and their push or email can still go out | HTTP calls `read_for_conversation` (`conversations.rs:2787`); MCP `mark` does not (`mcp/talk.rs:1395-1437`); push and email skip only when read (`push.rs:595`, `notification_email.rs:151`) |
| U5 | A muted conversation (level "nothing") still shows bold and counts in "next unread" (inferred: the sidebar reads no level) | `ProjectSidebar.tsx:190-208`, `shell/sidebarMove.ts:10-30` |
| U6 | The badge mixes units: unread inbox rows (including FYI rows) plus unread DM conversations | `shell/documentTitleModel.ts:11-13`; `shell/homeSpace.ts:18-28`; `ProjectRail.tsx:111` |
| U7 | DMs are read from one workspace only (the default one), so another workspace's unread DMs never show | `direct.rs:982-1000` (K4's to fix) |

### 2.3 Notifications: rules and settings

- `notifications` rows hold ids, collapse by `group_key`, dedupe per occurrence, and every read
  re-checks access (`migrations/20261016120000_notifications.sql:1-90`; `notifications.rs:1-23`,
  `239-396` `deliver`, `1320-1345` `VISIBLE`). They are written by the outbox publisher
  (`notifications.rs:400-460` `fan_out`). "Only people get notifications"
  (`notifications.rs:12`); agents are told through external events instead (A2's area).
- Kinds (`notifications.rs:57-82`): mention, comment_mention, assigned, approval_requested,
  question_asked, run_waiting/completed/failed, checks_failed, review_requested, task_changed,
  conversation_message, reminder, scheduled_send_failed, page_suggestion, page_comment, plus
  usage alerts.
- **A DM produces no notification at all** unless it @mentions you or you set that DM to "every
  message". `fan_out` for a new message only runs `message_mentions` and `followed_messages`
  (`notifications.rs:407-412`); the default level is `mentions`
  (`migrations/20261017160000_later_and_scheduling.sql:91-110`); nothing ever writes a default
  level for DMs (the only writer is `notification_settings.rs:659-697`). The test
  `private_conversations_notify_only_people_inside` shows the unmentioned group member getting
  nothing (`tests/integration/notifications.rs:209-277`). So no push for a DM.
- **Replying in a thread does not follow it**: there is no automatic level, so you hear later
  replies only when mentioned (same evidence).
- Mentions: names resolve to `message_mentions`; `@everyone` and `@here` are one regex and mean
  the same thing, "every person who can see the conversation" (`message_content.rs:13-16`,
  `notifications.rs:529-614`). Above 50 people it takes a role with `channels.moderate` and a raw
  `role IN ('owner','admin')` check (`notifications.rs:571-576`; `conversations.rs:4244-4272`).
  `@here` ignores presence (there is none for people).
- Settings (N4, `notification_settings.rs`): per group of kinds and channel (in app, push,
  email) with defaults (`131-141`); project mutes per group (`158-206`); per-conversation level
  all / mentions / nothing (`20261017160000:77-110`; slash `/mute`, `/notify`); quiet hours with
  a time zone and a "pause until" (`214-275`, keys in `settings.rs:211-288`); **email digests
  daily or weekly already exist** (`notification_email.rs:1-10`, `215-300`). R3 and END-STATE
  list digests and quiet hours as missing; they are built (see section 9).
- Push: Web Push with encrypted endpoints and VAPID (`push.rs:1-13`); actions Approve, Deny,
  Reply, Done, Snooze with a one-use token (`push/actions.rs:1-10`; `public/sw.js:64-122`).

### 2.4 Inbox, Later, reminders, questions

- Inbox HTTP: `/api/v1/inbox`, `/inbox/summary`, `/inbox/update` (`notifications.rs:2178-2190`)
  with views Inbox / Unread / Snoozed / Archived (`notifications.rs:116-142`), categories
  (`84-114`), actions read, unread, archive, restore, snooze, unsnooze on up to 200 ids
  (`144-157`, `MAX_UPDATE_IDS`).
- Web (`inbox/InboxPage.tsx`): "Needs you" and "FYI" sections decided **in the browser**
  (`inbox/api.ts:378-415` `NEEDS_YOU_KINDS`), grouped by subject, single keys already: j/k, n
  (next needing you), Enter/o, e/y/# archive, u read, s snooze to tomorrow, a approve, Shift+I
  mark all read (`InboxPage.tsx:368-418`). Approvals and questions are answered in place
  (`InboxPage.tsx:342-366`). Missing: multi-select, saved views, snooze until new activity,
  keyword alerts, a server-side needs-you rule.
- Questions from agents: `questions.rs:1-12` (AWD-62), a `question_asked` card. Approvals:
  `agent_approvals` rows for server-run agents. A1 replaces both with `requests`.
- Later (`later.rs:1-13`; `saved_items`, `20261017160000:11-37`): save a message or task, or a
  bare reminder, with an optional time; done/undone; a due reminder is a job that re-checks
  access, then notifies (people) or emits an external event (agents). Scheduled messages
  (`scheduled_messages.rs`).

### 2.5 Sidebar

- Rail of spaces with a per-person order (`shell/ProjectRail.tsx:128-152`), DMs under a Messages
  tile (`direct/DirectSidebar.tsx`), each project's sidebar listing all its channels and the
  threads that need attention (`ProjectSidebar.tsx:182-229`; server
  `project_sidebar_threads`, `conversations.rs:3570-3650`, `thread_attention` `3651-3684`).
  Home has Inbox, My work, Later, Runs, Pages, Whiteboards (`shell/HomeSidebar.tsx:10-20`).
- No sections a person arranges, no starred, no "all unreads" view, no cross-channel Threads
  view. Previous/next unread shortcuts walk the rendered sidebar (`shell/sidebarMove.ts`).

### 2.6 Composer and its extras

- One composer, `conversations/StructuredComposer.tsx` (1,045 lines): formatting
  (`:128-133`), attach, @mention, #task (`:931-948`), schedule (`:984`), unsent queue
  (`:786`), local drafts (`app/localDrafts.ts`, browser only), offline queue flushed by
  `app/outbox.ts:24-48` with the original idempotency key. Sending is optimistic: a dimmed copy
  until the server entry replaces it (`conversations/useMessageSender.ts:20-75`).
- Slash commands are one hard-coded list in the web (`later/slashCommands.ts:9-33`): remind,
  later, schedule, mute, unmute, notify, task, tag, launch. Manifests list the names
  (`modules/messages.json` `slots.slash` etc.) but don't define them.
- Message menu (`conversations/MessageActions.tsx:222-240`): Reply, Edit, Save for later,
  Forward, Create task (`conversations/useMessageLater.tsx:101-102`), Pin, Save as decision
  (`conversations/MessageMemory.tsx:61-67`), Mark unread, Copy, Remind, Remove. A task made from a
  message stores a `work_item_links` row to it (`work_items.rs:2444-2467`); a decision stores
  `source_message_id` (`memory.rs:990-1037`). No page, sheet row, sticky or record from a message,
  and no chip on the message saying what was made from it.
- Reactions: a fixed list of 12 emoji, enforced by a CHECK (`0048_message_reactions.sql:17-18`;
  `reactions.rs:58`). No custom emoji.
- Link previews for outside pages only, fetched with a guard (`link_previews.rs:1-15`). Links to
  this app's own things get no card.

### 2.7 Search and the command palette

- `search.rs`: full text over messages, threads, tasks, pages, decisions, records, files with
  access checks (`search.rs:1-8`, `29-77`). The function takes filters for conversation, author,
  since, until (`search.rs:80-86`), but **the HTTP route passes `SearchFilter::default()`**
  (`search.rs:520-553`), so the web has no filters. The web page is `memory/SearchPage.tsx`.
- The palette matches names locally and sends words to the search page
  (`shell/CommandPalette.tsx:190-204`). No saved searches, no `has:` or `is:` filters.

### 2.8 Presence, status, profiles

- Agents have presence and profiles (`agent_profiles.rs:1184-1312`; `agents/presenceModel.ts`).
  People have no presence, no status, no typing indicator, no avatar image (`avatar_url` is always
  `None`, e.g. `direct.rs:146-155`).

### 2.9 Agents in chat

- Reply requests show one status line per agent with progress text and Stop / Ask again
  (`conversations/replyStatusModel.ts`, `conversations/ReplyStatus.tsx:13-60`;
  `docs/plans/agent-tag.md`). Server-run agents also show run cards from the thread timeline's run
  projection (`conversations.rs:4432-4790`). Agents asking agents is limited to groups with a turn
  allowance (`replies.rs:101-106`, `128-156`); A2 owns changing that.

### 2.10 The phone

- Installable (`public/manifest.webmanifest`: shortcuts to New task, Inbox, Messages; share
  target to `/share`), service worker caching only the app shell, never API responses
  (`public/sw.js:1-13`). Under 860 px the rail and sidebar become one drawer opened by a menu
  button or a right swipe (`shell/mobileNavigation.ts:3`, `shell/AppShell.tsx:258-266`, `:333`).
  Pull to refresh, keyboard-aware viewport (`shell/PullToRefresh.tsx`,
  `shell/useKeyboardViewport.ts`), quick add (`quick-add/`). No bottom tab bar, no swipe actions
  on rows, no offline reading.

### 2.11 Permission differences by account kind in this area

Principle 1 forbids these; each is listed with what replaces it.

| Where | What | Replaced by |
| --- | --- | --- |
| `conversations.rs:3702-3707` and four copies (B finding 6) | Direct-space role follows the workspace role only for people | K4 (DMs leave the hidden project); until then K1's `effective_space_role` |
| `direct.rs:986-988` | Only people can have a direct space | K4 |
| `conversations.rs:2303-2309` | Sending with no MCP credential is allowed only for people | K1 (credential check by connection, not kind) |
| `replies.rs:105` | Agents' mentions in channels and threads ask nobody | A2 (loop limits as settings) |
| `notifications.rs:551`, `:643` | `@everyone` and "every message" reach people only | Delivery mechanics, not permission: keep, and route agents through A2's delivery (C2 passes the recipients) |
| `notifications.rs:571-576`, `conversations.rs:4262-4265` | `role IN ('owner','admin')` for large `@everyone` | K1 permission `channels.mention_everyone` |

## 3. The target design

### 3.1 Principles for this workstream

1. **One read path, one send path.** Every way of reading (web page, agent tool, app, push
   action, "mark all read") calls `read_state::mark_seen`; every way of sending already calls
   `append_message_command_with`, which gains the author's read move.
2. **Attention is server-decided.** Needs-you versus FYI, badge counts and unread counts are
   computed on the server so web, phone, push, email and agents agree.
3. **Notifications stay ids, not text** (keep `notifications.rs:12-17` contract).
4. **Every new write is an operation with an agent tool** (K2 working names; until K2 lands, an
   HTTP route plus an MCP tool sharing one function, as the send pipeline does).

### 3.2 Channels finished

**Model.** Channels stay `conversations` rows inside a project (a space after K4).

```sql
-- 2026xxxxxxxxxx_channels_finished.sql (expand only)
ALTER TABLE conversations
    ADD COLUMN topic TEXT CHECK (topic IS NULL OR char_length(topic) <= 250),
    ADD COLUMN description TEXT CHECK (description IS NULL OR char_length(description) <= 2000),
    -- New space members join it on arrival (today's behaviour for every channel).
    ADD COLUMN auto_join BOOLEAN NOT NULL DEFAULT false,
    -- Newest counted message, for unread (section 3.3).
    ADD COLUMN last_message_sequence BIGINT NOT NULL DEFAULT 0;

ALTER TABLE conversation_members
    ADD COLUMN role TEXT NOT NULL DEFAULT 'member' CHECK (role IN ('member','manager')),
    ADD COLUMN joined_via TEXT NOT NULL DEFAULT 'added'
        CHECK (joined_via IN ('added','joined','auto','creator','group'));
```

- **Membership is uniform.** A row in `conversation_members` means "in this conversation" for
  every kind. For a `members` (private) channel it also grants visibility (unchanged function).
  For a `project` (public) channel it means joined: it's in your sidebar and its level applies;
  visibility is unchanged, so anyone in the space can still browse, read and join it. Alternative
  considered: a separate `channel_joins` table. Rejected: two "who is in this" tables would drift,
  and private channels already use this one.
- **Backfill keeps today's behaviour**: every existing channel becomes `auto_join = true`, and
  every enabled member of its project gets a row (`joined_via = 'auto'`). Idempotent
  `INSERT ... SELECT ... ON CONFLICT DO NOTHING`, run by a resumable job in batches of 1,000
  channels ordered by id (a `backfill_cursors` row records the last id). Until it completes, the
  sidebar treats "no rows at all for this channel" as "everyone joined" (one `NOT EXISTS`), so
  order of arrival doesn't matter.
- Arrival: `apply_arrival_defaults` (`roles.rs:1842`) also adds the new member to every
  `auto_join` public channel of the space.

**Operations** (permission names are K1's catalogue; proposed ones marked *new*):

| Operation | Input | Permission | Effect |
| --- | --- | --- | --- |
| `channel_create` (extend) | `name, visibility: public\|private, topic?, description?, autoJoin?, memberIds?` | `channels.create`; private also *new* `channels.create_private` | Insert channel, creator as `manager`, members added, event `conversation.created` |
| `channel_update` | `channelId, expectedVersion, name?, topic?, description?, autoJoin?` (the Page tab is set with C1's `conversation_tab_set`) | `channels.moderate` or channel `manager` | Update, event `conversation.updated`, system entry "X set the topic" |
| `channel_join` | `channelId` | sees the channel (public), *new* `channels.browse` | Row `joined`, event |
| `channel_leave` | `channelId` | member | Delete row; for a private channel the last manager can't leave without handing over |
| `conversation_member_add` / `_remove` | `conversationId, accountIds[]` or `userGroupIds[]` | public: `channels.post`; private: channel `manager` or `channels.moderate` | Reuses `direct::change_group_members` generalised (`direct.rs:545`), audit, system entry |
| `channel_browse` (query) | `spaceId?, q?, cursor` | `channels.browse` | Public channels with member count, topic, joined flag; private ones only if a member |
| `channel_archive` / `_restore` | exists (`conversations.rs:2965-3137`) | unchanged | |

**Events** are K3's names: `conversation.created`, `conversation.updated`,
`conversation.member.added`, `conversation.member.removed` (join, leave, add, remove) and
`conversation.visibility.changed`.

**Reading paths fixed first** (P0.0 and P0.28; R10): `list_channels`, the sidebar's channel and
thread queries, `channel_browse` and every new list use one SQL helper. P0.28 adds it and P0.0
fixes the sidebar and `list_channels` leak; C2.1 keeps only what goes beyond them. The shape
P0.28 lands, for reference:

```sql
-- P0.28's helper; K1 later folds it into its general filter.
CREATE FUNCTION conversation_readable_by(target UUID, viewer UUID) RETURNS BOOLEAN
LANGUAGE sql STABLE AS $$
  SELECT conversation_visible_to(target, viewer)
     AND EXISTS (SELECT 1 FROM conversations c
                 JOIN project_memberships m ON m.project_id = c.project_id
                  AND m.actor_id = viewer AND m.enabled
                 WHERE c.id = target)
$$;
```

and the project sidebar refuses the direct project (`kind = 'direct'` returns NotFound) until K4
removes it.

**Channel details** (web, lazy): a side panel from the header with tabs registered in the layout
registry as parts `channel.details`, `channel.tab.page`, `channel.tab.pins`, `channel.tab.tasks`,
`channel.tab.files`, `channel.tab.members`:

- **Page**: a row in C1's `conversation_tabs` (C1.26), created on first open as "<channel>
  notes" in the space (C1's `page_create`, then `conversation_tab_set`). Shown as the first tab,
  editable live. C2 owns the header and the part `channel.tabs`; the Page tab stays hidden until
  C1.26 lands. No column on `conversations` and no `about` link.
- **Pins**: existing `conversation_pins` (memory.rs) listed.
- **Tasks**: tasks linked to the channel's messages or threads (K3 links where the source is a
  message in this conversation) plus tasks whose thread is here; a list piece (W1).
- **Files**: `files.conversation_id = channel or its threads` (`search.rs` already joins these).
- **Members**: list, add, remove, user groups added as a unit.

**Sidebar sections** a person arranges:

```sql
CREATE TABLE sidebar_sections (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 80),
    kind TEXT NOT NULL DEFAULT 'custom' CHECK (kind IN ('custom','starred')),
    position INTEGER NOT NULL,
    sort TEXT NOT NULL DEFAULT 'manual' CHECK (sort IN ('manual','alpha','recent')),
    show TEXT NOT NULL DEFAULT 'all' CHECK (show IN ('all','unread')),
    collapsed BOOLEAN NOT NULL DEFAULT false,
    UNIQUE (account_id, workspace_id, name)
);
CREATE TABLE sidebar_items (
    section_id UUID NOT NULL REFERENCES sidebar_sections(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    conversation_id UUID NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    position INTEGER NOT NULL,
    PRIMARY KEY (account_id, conversation_id)  -- a conversation sits in one section
);
```

A conversation not placed stays in its space's default section ("Channels" under the space;
"Direct messages" for DMs). Reads join through `conversation_readable_by`, so losing access
removes it from the sidebar without deleting the row. Operations `sidebar_section_write`,
`sidebar_item_move` (agent tools too; an agent can arrange its own). Web: a "Your sidebar" block
on Home listing sections across spaces, plus starred; drag to reorder (reuse `orderAfterDrop`,
`shell/dragOrder.ts`). This is the first cross-space chat list; W1 owns where it sits in the shell.

**Muting and levels**: keep `conversation_notification_levels` with `all | mentions | nothing`;
add `source TEXT NOT NULL DEFAULT 'chosen' CHECK (source IN ('chosen','auto'))` so automatic
following (3.4) never overrides a choice. "Mute" in the UI = level `nothing`, which also dims the
row and drops it from badges and "next unread" (rule R5 below).

### 3.3 Unread state that is exactly right

**Rules** (each pinned by a test in `apps/server/tests/integration/unread_rules.rs` unless noted):

- **R1 What counts.** Only messages count: not system entries, worker events or run events.
  `conversations.last_message_sequence` is set in the send pipeline in the same statement that
  allocates the sequence, and recomputed on removal as the newest non-removed message
  (`message_edits::apply_removal`). Unread = `last_message_sequence > highest_seen_sequence`.
- **R2 Your own message never makes it unread.** `append_message_command_with` calls
  `read_state::mark_seen(tx, author, conversation, new_sequence)` for the author, on every path
  (web, agent tools, app, push reply, scheduled, forward, share). Slack does the same: sending
  marks the conversation read.
- **R3 One mark-seen function.** `read_state::mark_seen` upserts the mark, emits
  `ReadStateUpdated`, and reads the notifications that point at messages up to the mark
  (`notifications::read_for_conversation`). HTTP mark-seen, MCP `mark_read`/`conversation_read`
  with `markRead`, push action "mark read", and "mark all read" call it. Fixes U4.
- **R4 Removal.** A removed message doesn't count; if it was the only unread, the conversation
  becomes read (follows from R1).
- **R5 Muted.** Level `nothing`: unread kept (you can still see where you were), but not bold,
  not in badges, not in "next unread", not in the Unreads view.
- **R6 Threads.** A thread's messages don't make its channel unread. A thread is unread for you
  only if you follow it (3.4) or open it; the channel shows "N threads with new replies" when any
  followed thread is unread.
- **R7 Archived and invisible.** An archived conversation, or one you can no longer read, is
  never unread and never counted.
- **R8 Hidden tab** doesn't mark read (web rule kept; `useAutoMarkSeen.test.ts`).
- **R9 Edits and reactions** never change unread (already true; test kept).
- **R10 Same for agents.** Briefing (`mcp/briefing.rs:174`) uses `read_state::unread_summary`,
  so an agent's "what's new" follows R1 to R9.

**Counts.** `read_state::unread_summary(executor, viewer, scope) -> Vec<UnreadRow>`:

```rust
pub struct UnreadRow {
    pub conversation_id: Uuid,
    pub unread: bool,              // R1..R7
    pub unread_messages: u16,      // capped at 99, others' non-removed messages after the mark
    pub mentions: u16,             // unread mention notifications for this conversation
    pub muted: bool,
}
pub enum UnreadScope { Space(Uuid), Direct, Conversations(Vec<Uuid>), OtherWorkspaces }
```

`Direct` is the account's DMs and groups wherever they live (the direct scope and any `kept`
workspace's DMs it can read; R20), the same in every workspace. `OtherWorkspaces` returns one
unread flag and mention count per workspace the account is in other than the current one, for
W1's switcher (R19, R20).

Counting uses `conversation_entries (conversation_id, sequence)` with `LIMIT 100` inside a
lateral subquery, so it is bounded per row. DMs and groups show the number; channels show bold,
plus a number only for mentions (Slack's convention).

**Badge.** `inbox::attention(viewer, workspace) -> Attention { needs_you_conversations: u32,
fyi_unread: bool }` counts distinct subjects among unread **needs-you** notifications (DM rows are
one per conversation, 3.4), excluding muted. The tab title, app badge (Badging API), rail tile and
push badge use it. FYI shows as a dot, not a number. Fixes U6.

**Needs you on the server.** `notifications.attention TEXT NOT NULL DEFAULT 'fyi' CHECK
(attention IN ('needs_you','fyi'))`, set at delivery from a kind table in `notifications.rs` (the
list now in `inbox/api.ts:378`), and recomputed on read for approvals and questions that are no
longer pending. Backfill: one `UPDATE ... WHERE kind IN (...)`, idempotent.

### 3.4 Notifications that reach you when they should

New kinds, groups and defaults (`notification_settings.rs` `Group`):

| Kind | When | Attention | Group | Push default |
| --- | --- | --- | --- | --- |
| `direct_message` | A message in a DM or group you're in, by someone else, level not `nothing` | needs you | *new* `direct_messages` | on |
| `thread_reply` | A message in a thread you follow, by someone else, not a mention of you | fyi | *new* `threads` | off |
| `keyword` | A message matching one of your keywords in a conversation you can read and haven't muted | fyi | mentions | on |
| `request` | A1's request addressed to you or a role you hold (replaces `approval_requested`, `question_asked` when A1 lands) | needs you | approvals | on |
| `connection_problem` | A2: an agent or app you manage lost its connection or its credential failed | needs you | *new* `agents` | on |
| `agent_unresponsive` | A2: an agent you manage or asked is marked "not responding" | fyi | `agents` | off |
| `budget_warning` | A2/H1: a budget you own reached its warning or limit | needs you | `agents` | on |
| `thing_update` | A2's `follows`: a thing you follow changed (C2 reads `follows` for things; conversations keep `conversation_notification_levels`) | fyi | *new* `following` | off |

- `group_key` for `direct_message` is `dm:<conversation>`, so a busy DM is one inbox row with a
  count; reading the DM reads it (R3).
- **Automatic following.** `conversation_notification_levels` rows with `level = 'all', source =
  'auto'` are written (`ON CONFLICT DO NOTHING`) when a person starts a thread, posts in it, is
  mentioned in it, or a thread is opened from their message. Unfollow writes `mentions` with
  `source = 'chosen'`. `followed_messages` (`notifications.rs:619-675`) then emits `thread_reply`
  for threads and keeps `conversation_message` for channels a person set to every message.
- **`@here` means present.** `@everyone` stays "everyone who can read it"; `@here` becomes
  "everyone who can read it and was active in the last 10 minutes and isn't away" (presence,
  3.6). `@channel` is accepted as an alias of `@everyone`. One regex per meaning in
  `message_content.rs`.
- **User groups** (K1 data): `@design` resolves to the group's current members who can read the
  conversation, at send time, stored in `message_mentions` with `via_group_id`. Above
  `EVERYONE_OPEN_LIMIT` it needs `channels.mention_everyone` like `@everyone`.
- **Keywords**:

```sql
CREATE TABLE notification_keywords (
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    keyword TEXT NOT NULL CHECK (char_length(keyword) BETWEEN 2 AND 60),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account_id, workspace_id, lower(keyword))  -- as a unique index
);
```
  Matching in `fan_out` for `MessageCreated`: keywords of accounts who are members of the
  message's space (at most 50 per person), matched with `plain_text ~* ('\m' || regexp_escape(k) || '\M')`
  in one query, then filtered through `conversation_readable_by` and level. Keyword matches in
  messages the person authored are skipped.
- **Mentions in task comments by agents** (lead notes C6; B finding 3): done by P0.4 (R10). C1
  later moves comments into the one message system, which makes this automatic.
- **Schedules.** Extend quiet hours to days: settings `notifications.quiet_days` (a set of
  weekdays fully quiet) next to `quiet_hours_start/_end` (`settings.rs:211-247`). "Away until"
  from status (3.6) sets `notifications.paused_until` (`settings.rs:288`) so one mechanism holds
  pushes and emails. Requests marked urgent by A1 can break through quiet hours if the person
  allows it (setting `notifications.urgent_breaks_quiet`, default off).
- **Email digests** (exist): include `direct_message` and needs-you items first, then FYI, and
  link each to the app; respect pause; one digest per workspace.

### 3.5 One inbox you triage

**Views as tabs.**

```sql
CREATE TABLE inbox_views (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (char_length(name) BETWEEN 1 AND 40),
    position INTEGER NOT NULL,
    filter JSONB NOT NULL CHECK (jsonb_typeof(filter) = 'object' AND octet_length(filter::text) <= 4096),
    density TEXT NOT NULL DEFAULT 'detailed' CHECK (density IN ('detailed','compact')),
    UNIQUE (account_id, workspace_id, name)
);
```

Filter shape (validated by a Rust struct, unknown keys refused):

```json
{ "attention": "needs_you", "kinds": ["mention","direct_message"], "spaces": ["<uuid>"],
  "from": ["<account uuid>"], "fromKind": "agent", "conversations": ["<uuid>"],
  "state": "unread" }
```

`fromKind` filters by who acted (a display filter, not a permission). Built-in tabs (not rows):
Needs you, All, Mentions, DMs, Requests, Cleared. Agents can make views for themselves too
(`inbox_view_write`), useful for an assistant triaging for a person it acts for.

**Actions.** Extend `UpdateAction` (`notifications.rs:146-157`): `ClearAndWake` (archive, then
the next occurrence opens a new row: already today's behaviour, now named), `SnoozeUntilActivity`
(new column `notifications.wake_on_activity BOOLEAN NOT NULL DEFAULT false`; `deliver`'s
`ON CONFLICT` clears `snoozed_until` when it's set), and `Done` for requests (answers nothing,
removes it from Needs you). Bulk: the web gains multi-select (x to select, Shift+j/k to extend,
then e/u/s on all), sending one `inbox_update` with up to 200 ids (already supported).

**Keys** (kept and added): j/k, x select, e clear, u read toggle, s snooze menu (1 later today,
2 tomorrow, 3 next week, 4 until activity), Enter open, r reply inline (mentions, DMs),
a/d approve/deny, 1-9 pick a request option, [ ] previous/next tab, Shift+I mark all read.

**Request cards** from any account (A1 `requests`): a card shows the asker, what it's about (with
a link), the options or a text field, and "Answer", answered in place through A1's
`request_answer {requestId, optionId?, text?, expectedVersion}`. The card is W1.24's
`RequestCard({card})`, drawn from A1's `requests::card(request_id, viewer)`; C2 places it in the
inbox and push. Until A1 lands, approvals and questions keep today's cards
(`InboxPage.tsx:342-366`).

### 3.6 People features

- **Status and away.** One table for every account (people and agents; R12: A2 writes agents'
  status through the same operation, and A2 defines no table of its own). Keyed by account and a
  nullable workspace (R19): a status a person sets by hand is global (`workspace_id NULL`, shown
  in every workspace); a work line an agent or host writes about work in a workspace is stored
  for that workspace and shown only there.

```sql
CREATE TABLE account_statuses (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    workspace_id UUID REFERENCES organizations(id) ON DELETE CASCADE,  -- NULL: global (R19)
    emoji TEXT CHECK (emoji IS NULL OR char_length(emoji) <= 16),
    text TEXT CHECK (text IS NULL OR char_length(text) <= 120),        -- A2's line length
    state TEXT CHECK (state IS NULL OR state IN
        ('available','working','asleep','paused','unresponsive')),     -- A2's states
    clear_at TIMESTAMPTZ,        -- A2's `until`
    away_until TIMESTAMPTZ,      -- shows "Away until …"; pauses notifications to then
    set_by TEXT NOT NULL DEFAULT 'self' CHECK (set_by IN ('self','host','workspace')),
    set_by_account_id UUID REFERENCES actors(id),  -- a host or app may set it for an agent
    paused_by_account_id UUID REFERENCES actors(id),
    paused_reason TEXT CHECK (paused_reason IS NULL OR char_length(paused_reason) <= 200),
    paused_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE NULLS NOT DISTINCT (account_id, workspace_id)
);
```
  Operations `status_set(text?, emoji?, state?, clearAt?, awayUntil?, workspace?)` (A2's
  `status_set {line?, state?, until?}` is this operation; `workspace` given means a work line
  for that workspace, absent means the global status; A2.10 writes the work line with
  `workspace` set) and `status_clear`. Allowed for the account itself, its host, or a manager
  of the account. Pausing an agent is
  A2's operation and writes the `paused_*` columns here. Shown next to names in the composer's
  mention list, member lists, DM headers and profiles: the global status plus, inside a
  workspace, that workspace's line. Status text is visible to everyone who shares a workspace
  with the account (the global one) or is in that workspace (a work line).
- **Presence.** No C2 table: K5 owns presence (`realtime_connections`, an unlogged table;
  online = seen within 90 s; `presence::online_in(workspace)`, K5.10), fed by browser sockets and
  agents' streams alike. C2 reads it. "Active" for `@here` = online by K5 and not away (C2's
  `away_until` or an A2 `state` of asleep or paused).
- **Typing.** Ephemeral, never stored: K5 protocol 2's `typing {conversationId}` frame sent by
  the composer at most every 3 s while typing, expiring after 6 s, relayed only to accounts that
  can read the conversation. Agents' "writing a reply" is already shown from reply requests and
  stays; A2's `presence_set {conversationId, state: typing|working|idle}` publishes the same
  `typing` frame through `realtime::publish_ephemeral` for agents. Online counts agent streams
  as well as browser sockets (`presence::online_in`).
- **Profiles for every account.** A profile panel for any account: name, avatar (an uploaded
  image through the files seam, or initials), title, status, local time, the spaces you share,
  "Message", and for agents what A2's profile adds. `account_profiles (account_id PRIMARY KEY,
  title, avatar_file_id, pronouns, time_zone)`: the profile is global to the account (R19);
  "the spaces you share" lists only spaces in workspaces the viewer shares, never the account's
  other workspaces. Agents' existing `agent_profiles` stays A2's.
- **User groups for mentions and membership** (K1 data: `account_groups` and
  `account_group_members`; C2 needs a `handle` on `account_groups`, requested of K1): C2 adds mention resolution (3.4), autocomplete
  (groups listed with member count), and "add group to channel" (adds current members, joined
  via `group`; optional `sync` so later group members join too).
- **Guests limited to named channels** (K1 role `guest`, lacking `channels.browse` and
  `channels.create`): `channel_browse`, the sidebar and search only show conversations the guest
  is a member of. With K1's SQL filter this is `conversation_readable_by` plus "public channel
  readable without membership only with `channels.browse`". Isolation case: a guest added to one
  channel in a space can't list, read, search or be notified from another.
- **Custom emoji and any emoji.** Reactions accept any single Unicode emoji or a workspace's
  custom one:

```sql
CREATE TABLE custom_emoji (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (name ~ '^[a-z0-9_+-]{2,32}$'),
    file_id UUID NOT NULL,           -- image through the files seam
    created_by UUID NOT NULL REFERENCES actors(id),
    retired_at TIMESTAMPTZ,
    UNIQUE (workspace_id, name)
);
ALTER TABLE message_reactions DROP CONSTRAINT message_reactions_emoji_allowed,
    ADD CONSTRAINT message_reactions_emoji_shape CHECK (octet_length(emoji) <= 64);
```
  Unicode validity is checked in Rust against a generated list (`emoji/list.json`, shared with the
  web as a parity fixture like `ui/markdown/parseParity.json`); a custom one is stored as
  `:name:` and must exist in the workspace. The 12 current names stay valid in URLs and tools.
  Expand first (drop the CHECK only after the Rust check ships in the same slice).
- **Shared channels between workspaces** (later, sketched only): a channel with
  `shared_with` rows naming another workspace on the same server; messages stay in the owning
  workspace; members from the other workspace join as guests of that one channel with their
  own identity. Cross-server sharing would need a federation protocol and is out of scope.

### 3.7 Chat to work

**Makers.** A module registers how to make one of its things from messages (M1's registration;
until M1, a static list in `chat_to_work.rs`):

```rust
pub struct MakeInput { pub messages: Vec<Uuid>, pub selection: Option<TextSelection>,
                       pub target: serde_json::Value /* module-specific: sheet id, column, board */ }
#[async_trait]
pub trait MessageMaker: Send + Sync {
    fn kind(&self) -> &'static str;                // "task", "decision", "page", "sheet_row", "sticky", "record"
    fn permission(&self) -> Permission;            // the module's own create permission
    async fn make(&self, tx: &mut Transaction<'_, Postgres>, actor: &Actor,
                  source: &MessageSource, input: MakeInput) -> Result<ThingRef, AppError>;
}
```

One operation, `message_make {messageIds[1..20], selection?, kind, target, clientRequestId}`:
checks the actor can read every message (`conversation_readable_by`), calls the maker, writes a
K3 link `made_from` from the new thing to each message, emits `thing.made_from_message`, and
posts nothing in the channel by default (an option "Say so in the thread" posts a system entry).
Makers: task (exists, `work_items.rs:2444`, moves to K3 links), decision (exists), page (C1's
`page_create {from: {messageIds}}` with the quoted messages as blocks with attribution), sheet
row (C1's `sheet_update` with `rows: [{keyColumn, values}]` and a chosen column mapping),
whiteboard sticky (C1's `whiteboard_update_elements` adding an element with a link back), record
(T1's `record_create_many` into a chosen collection, message text in its title or a long-text
field). Names are K2's canonical ones. The `made_from` links are written by one idempotent helper
(`chat_to_work::link_made_from(tx, thing, messages)`), which C1's `page_create from` calls too, so
there is one writer. A thing made from a DM or group message goes to a space the person picks,
never into the direct scope (R20); makers whose module is off are not offered (R22).

**Selection.** In the web, Shift-click selects a range of messages (or text inside one); the
selection bar offers "Make…" with the makers the actor's role allows. Agents call the same tool.

**Chip.** The timeline asks K3 for incoming `made_from` links of the visible messages (one batched
read) and shows "→ Task AWD-142 · In progress" chips, re-checked for the viewer (a thing they
can't see shows no chip). The made thing shows "Made from a message in #design" linking back.

**Threads** keep status and outcome (`conversations.rs:1301-1440`). Polish: a cross-space
"Threads" view of threads you follow, newest reply first, with status chips; "Close with outcome"
in the thread header; a thread's outcome shows under its origin message.

**Slash commands registered by modules.** C2 defines the entry shape; M1 adds it to the
registration contract as `Registrar::maker` and slash entries (M1.4, M1.5). Manifest
`slots.slash` entries become objects:
`{ "name": "task", "usage": "title", "description": "...", "operation": "task_create",
"args": "text" | "none" | "when_text", "where": "space" | "any" }`. The composer builds its list
from the enabled modules' entries (`later/slashCommands.ts` becomes the reader, with web-only
handlers kept for `remind`, `schedule`, `notify`). M1 owns the registry; C2 defines the entry
shape and the composer.

**Internal link cards.** When a message links to this app (a path matching any registered thing
`href`, from manifests' `things[].href`), the server stores a reference (K3 link `mentions`) and
the timeline shows a card drawn by the kind's card piece (W1), read live and access-checked per
viewer; nothing is fetched over the network and no text is copied.

### 3.8 Search as people use it

- **One query grammar**, parsed the same in Rust and TypeScript (parity fixture
  `search/queryParity.json`): free words plus `from:@handle`, `in:#channel` / `in:@dm-person`,
  `during:2026-10` / `before:` / `after:` / `on:`, `has:file|link|reaction|pin|task`,
  `is:thread|dm|unread|saved|mine`, `kind:message|task|page|...`, quoted phrases, `-word`.
- HTTP `/api/v1/search` accepts the parsed filter (fixing `search.rs:540`); the agent tool
  `search` accepts the same grammar string. K3 owns the index and kinds; C2 owns the grammar,
  filter-to-index mapping and the UI.
- **Saved searches**: `saved_searches (id, workspace_id, account_id, name, query, position)`,
  shown in the search page and the palette; operations `saved_search_write`.
- **Jump-to** in the palette: `GET /api/v1/jump?q=` returns at most 20 ranked conversations,
  people, spaces, tasks by key, pages by title, using recency of your own visits
  (`shell/recentPlaces.ts`) as a boost; it's a name match, not full text, and must answer in
  under 100 ms.
- **Access**: every hit comes through K1's SQL filter and `conversation_readable_by`; guests and
  apps acting for a person see only what their role and limit allow.
- **Answers with sources** (later): "Ask" on the search page sends the top results the viewer
  can read (never more) to the workspace's chosen model through the existing AI gateway (`ai/`),
  returns an answer with numbered citations to those hits, and stores nothing but an audit row.

### 3.9 Speed and feel

**Targets** (measured with `telemetry` from real browsers, p75 over a week, desktop / phone):

| Action | Target |
| --- | --- |
| Switch to a conversation already in cache | 100 ms / 200 ms to first paint of messages |
| Open a conversation cold | 500 ms / 1,000 ms |
| Send: message visible (dimmed) | 50 ms |
| Send: confirmed | 400 ms / 800 ms |
| Inbox open | 400 ms / 800 ms |
| Jump-to results | 150 ms |

- Server: timeline endpoint p95 under 120 ms for 50 messages (a `Server-Timing` header and an
  integration test that fails over 10 queries per request). Prefetch the timeline on sidebar
  hover (desktop) and for the top 5 unread conversations after load.
- Optimistic send and the offline queue exist (`useMessageSender.ts`, `app/outbox.ts`); add a
  visible queued state in the sidebar ("2 waiting to send") and retry on reconnect with the same
  idempotency key (already the case in `flushOutbox`).
- **Drafts across devices**: `message_drafts (account_id, conversation_id, content JSONB,
  updated_at)`, saved 2 s after typing stops; local drafts stay the first source while offline;
  a "Drafts and sent" view lists them. Conflict: newest `updated_at` wins, the other kept as a
  recoverable copy for a day.
- **Keyboard coverage**: every chat action reachable (list in slice C2.30), and the shortcut
  sheet (`shell/shortcutSheet.ts`) generated from one table.
- **Gate**: a "Chat surfaces" section added to `docs/frontend/polish-checklist.md` (section 6).

### 3.10 The phone

- **One-handed**: W1 owns the shell and the bottom bar (Home, Inbox, Messages, Search, You,
  each a registry part; W1 3.9); with only Chat on, Home is the conversation list (3.12). Within C2's pages: the composer sits above
  the keyboard (exists), message actions open as a bottom sheet on long-press, reactions in the
  sheet's first row, inbox rows swipe left to clear and right to snooze, DM rows swipe to mark
  read, and the conversation header collapses on scroll.
- **Answer a request from a notification**: push actions extend to A1 requests (options as
  buttons, up to the platform's limit; more options or a text answer open the card). DMs get
  Reply and Mark read actions. On platforms without action buttons, the tap opens the card with
  focus on the answer.
- **Share into the app**: `/share` (exists) gains "Send to…" (conversation picker with recent
  ones first), "Make a task", "Save to Later".
- **Quick capture**: quick add gains "Note to self" (a personal DM, K4's personal space) and
  "Message…".
- **Offline reading** uses W1.31's one mechanism (accepted end-state change: opt-in per
  person, a workspace can switch it off, cleared at sign-out): C2 sets what chat keeps (the
  last 50 messages of the 30 most recent conversations, within W1's caps) and the wording. Default
  off; the service worker's promise (`public/sw.js:1-5`) is reworded to say "unless you turn on
  offline reading". No second persister.
- **Installable web app or native wrapper.** Recommendation: **stay an installable web app, keep
  push transport-neutral, and decide on a thin wrapper by measurement, not now.** Reasons:
  Android's browsers deliver Web Push with action buttons, which covers "answer from the
  notification"; iOS delivers Web Push to apps added to the Home Screen (since 16.4) with badges,
  but current reports say no action buttons there, so on iOS "answer" means tap-to-open-card. A
  wrapper adds two app-store pipelines and review for a team that wants self-hosting to be one
  command. What would change the answer: if measured iOS delivery or the lack of action buttons
  hurts (C2.34 records delivery and tap-through), a wrapper that only adds native push and share
  is a later slice; the server's `push_subscriptions` gets a `transport` column (`web_push`,
  `apns`, `fcm`) so the rest of the pipeline doesn't change.

### 3.11 Agents in chat, as people see them

- **Working line**: keep reply status lines (`replyStatusModel.ts`); with A1, the line reads from
  the run's latest activity for any agent, not only reply requests, so "Muse is working ·
  Updating Budget sheet" shows whoever hosts Muse.
- **Run chip**: a message posted as part of a run (A1 gives `messages.run_id`, today
  `messages.agent_run_id`) shows a chip "Run 1234 · 3 min · $0.04" linking to the run page.
- **Several agents in one room**: when a message asks more than one agent, their status lines
  collapse into one row ("3 agents asked · Muse answering · Hermes passed"), expandable. A2 owns
  who may answer and loop limits; C2 shows A2's decision ("Stopped after 6 agent turns · Allow
  more") with the button calling A2's operation.
- **Stop**: per-agent Stop exists; add "Stop all in this conversation" (one call to A2's
  `reply_stop_all` / A1's run cancel), shown when two or more are working. Stop and Stop all
  are permitted by K1.14's `conversations.stop_agents` in the conversation (default holders
  match `channels.post`), the same for people and agents.

### 3.12 DMs and groups belong to the people in them (R20)

DMs and group conversations belong to their participants, Discord-style, not to a workspace.

- **Where they live.** New DMs and groups carry the server's direct scope as their workspace
  (K4: one reserved `organizations` row with `kind = 'direct'`, never listed, no members). Access
  is participation only (`conversation_member_visible`, K4's helper under R3): no role, admin,
  workspace owner or staff console reads a DM.
- **One DM per pair, server-wide** (R31): one personal DM per pair on the server, and at most
  one kept DM per pair in each `kept` workspace (K4's `conversations.kept` tells them apart).
  `dm_open {accountId}` returns the existing personal DM for the pair if there is one (a unique
  index on the ordered pair within the direct scope), else creates it; inside a `kept` workspace
  it returns that workspace's DM, and the personal one stays in the list, labelled. Reaching
  someone you share no workspace with does not exist and needs a ruling before anyone adds it.
  K4.24 provides the server side.
  Groups (`group_open {accountIds[]}`) are not deduplicated. Existing DMs keep their workspace id
  and are treated as personal; where a pair has DMs in two workspaces, both stay, labelled with
  the workspace they started in; nothing merges.
- **Who may DM whom.** An account may start a DM or add someone to a group if (1) they share a
  workspace, (2) its role in that shared workspace allows starting DMs (K4.12's permission,
  `direct.start`), and (3) the other account's own setting allows it: `dm_policy` = `anyone`
  (anyone I share a workspace with, default) or `ask` (ask me first: the first message arrives as
  a request the recipient accepts or declines; nothing else reaches them until then). (4) Neither
  has blocked the other: `account_blocks (account_id, blocked_account_id, created_at)`; a block
  hides the DM, refuses new messages both ways, drops mentions from the blocked account in
  shared channels from the blocker's inbox, and is invisible to the blocked account. Same rule
  for every kind of account. Settings are global (the account's), so `account_settings` gains
  `dm_policy`.
- **Readable for good; sending needs a shared workspace.** A DM stays readable to its
  participants after they stop sharing a workspace; the composer then says "You no longer share
  a workspace with <name>" and sending is refused (`dm_not_shared`), re-checked per send.
- **One list everywhere.** The DM list (and the "Direct messages" sidebar section) is the same
  in every workspace's shell; `UnreadScope::Direct` counts it; W1 routes `/w/<slug>/dm/<id>` and
  `/dm/<id>`. The workspace switcher shows unread marks for other workspaces from
  `UnreadScope::OtherWorkspaces` (W1 draws it).
- **Kept DMs.** A workspace setting `direct_messages`: `personal` (default) or `kept`. In a
  `kept` workspace, a DM or group between its members started inside it is created with that
  workspace's id, follows its retention and export, and ends with membership (sending and
  reading stop when a participant leaves), as in Slack. Same table and visibility rule. The
  "who may DM whom" checks still apply. The setting changes only new DMs.
- **Things made from a DM message** go to a space the person picks (makers, 3.7).
- **Operations, each with an agent tool:** `dm_open`, `group_open`, `account_block`,
  `account_unblock`, `dm_request_answer {conversationId, accept}`, `account_setting_set
  {dmPolicy}`; `workspace_update {directMessages}` (workspace admin, K1 permission
  `workspace.manage`). An agent's subscription says whether it includes DMs (A2).
- **Isolation cases:** a workspace owner can't list, read or search a personal DM between two
  members; no membership row may reference the direct scope; the P0.0 class of leak has a case
  here; a blocked account learns nothing.

Slice C2.39.

### 3.13 Chat standing alone (R22)

A workspace whose shape is "Friends" has only Chat on (M1's shapes). C2 makes that a whole
product, not a trimmed one:

- **Home is the conversation list**: with Tasks, Pages and Agents off, `/w/<slug>/home` shows
  the conversation list (channels of the workspace's own space and the DMs), unread first, with
  "New message" and "New channel". Channels live in the workspace's own space (K4); no project
  list, no space picker until a second space exists.
- **Invitations are a link**: an "Invite people" button makes a join link (K4's invitation, R30;
  with a link token, `invite_link_create {expiresIn?, maxUses?}`), shareable from the phone's
  share sheet; joining by link lands in the conversation list.
- **No words from modules that are off**: no "project", "task", "agent" or "run" anywhere in
  C2's screens, empty states, notification text, push text or email digests for that workspace;
  makers and slash commands of modules that are off are not offered; notification kinds of
  modules that are off are not shown in settings. Words come from module registrations, so a
  module turned on brings its words. An account's kind is account information, not a module
  (R31): an agent may join whichever modules are on, its badge beside its name and its profile
  line say "agent" in every shape, and adding one with Agents off offers, without requiring,
  switching Agents on.
- **The phone**: the bottom bar shows Home (conversations), Inbox, Messages, Search, You; nothing
  else.
- Proof: W1.39's shape walk test for "Friends" passes.

Slice C2.40.

## 4. Migration path

- **Additive first.** Every table and column above is new or a widening; nothing is renamed.
  Migrations are timestamp-named and don't assume each other's data.
- **Unread (R1, R2)**: add `last_message_sequence DEFAULT 0`; backfill with a resumable job
  (batches by conversation id, cursor row) setting it to the newest non-removed message
  sequence. Until a conversation is backfilled (`last_message_sequence = 0 AND
  next_entry_sequence > 1`), readers fall back to today's `next_entry_sequence - 1`. The send
  pipeline sets it from day one, so new activity is right at once.
- **Mark-seen unification**: `read_state::mark_seen` extracted from
  `conversations.rs:2696-2800`; the HTTP handler and `mcp/talk.rs:1395` call it in the same PR;
  no data change.
- **Channel membership backfill**: `auto_join = true` for existing channels in the migration
  (cheap UPDATE); member rows by a job; readers treat a channel with no rows as "everyone
  joined" until the job marks it done (`channel_backfills` table, dropped in the cleanup slice
  C2.7b once every channel is done in production).
- **Needs-you attention column**: written by new deliveries; backfill UPDATE in the migration
  (bounded by the `notifications_unread` index; old archived rows left `fyi`, which is harmless).
- **Reactions**: the Rust validation ships with the CHECK change in one migration that first
  adds the new check, then drops the old.
- **Search grammar**: the old `q`, `projectId`, `kinds` parameters stay accepted; `query` with
  the grammar is new.
- **Slash commands**: manifests accept both string and object entries during the move; the web
  list is the fallback for strings.
- **Old paths deleted**: `NEEDS_YOU_KINDS` in the web goes in C2.4 once the server sends
  `attention`; the MCP read-state copy goes in C2.2; the `channel_backfills` fallback in C2.7b.

## 5. Slices

Ordered by what a daily user notices first. "Beside" means it can run in parallel.

**C2.1 Close the listing gaps (S).** R10: P0.0 fixes the sidebar and `list_channels` leak and
P0.28 adds `conversation_readable_by`; this slice keeps only what goes beyond them: refuse the
sidebar for `kind = 'direct'` projects, search coverage, and the private-channel cases.
Changes `conversations.rs:3419-3520`, `search.rs`. Depends on P0.0, P0.28. Accept: a test
in `tests/integration/conversation_privacy.rs` creates a `members` channel by SQL, a DM thread and
a group thread, and asserts a non-member sees none of them in `/channels`, the sidebar of the
space or of the direct project, or search; first commit is the failing test for the direct-project
sidebar to confirm or refute the suspected leak.

**C2.2 One read path (S).** P0.5 extracts `conversations::mark_seen` (reading through an app
clears notifications; R10). This slice moves it into `read_state::mark_seen` and adds the push
action and "mark all read" callers; `conversation_mark_read` (K2's name; `mark_read` stays an
alias) and `conversation_read{markRead}` already call it. Depends on P0.5. Accept:
`unread_rules.rs::reading_through_an_app_reads_notifications` (mention, then MCP mark_read as an
app acting for the person: inbox unread count drops, queued push job sends nothing).

**C2.3 Unread rules (M).** `last_message_sequence`, backfill job, R1, R2, R4, R5, R7,
`read_state::unread_summary`, counts in sidebar and DM list responses, briefing switched (R10).
Depends on C2.2. Accept in `unread_rules.rs`: worker event doesn't unread; own message via MCP,
push reply, scheduled send and forward leaves it read; removing the only unread message reads
it; muted conversation has `unread=true, muted=true` and isn't counted; archived never counts;
DM count is 3 after 3 messages and caps at 99; agent briefing agrees with the web for the same
conversation.

**C2.4 Badge and needs-you on the server (S).** `notifications.attention`, `inbox::attention`,
summary returns `needsYouConversations` and `fyiUnread`; title, rail and app badge use it; web
`NEEDS_YOU_KINDS` deleted. Depends on C2.3. Beside C2.5. Accept: 5 mentions in one channel and 2
DMs give badge 3; an FYI run completion adds no number; `inbox/api` test reads `attention`.

**C2.5 DMs and threads reach you (M).** Kinds `direct_message`, `thread_reply`; groups
`direct_messages`, `threads` with push defaults; automatic following with `source = 'auto'`;
the A2 kinds `connection_problem`, `agent_unresponsive`, `budget_warning` and `thing_update`
(reading A2's `follows`) with group `agents` and `following`. Agent comment mentions are P0.4's.
Beside C2.4. Accept in `notifications.rs` tests: an
unmentioned group member gets one collapsing `direct_message` row and a push job; replying in a
thread then someone else replying gives `thread_reply`; an explicit "mentions" choice is never
overwritten by auto-follow; a DM set to nothing sends nothing; a budget warning reaches the
budget's owner as needs-you; the kinds table test covers the new kinds.

**C2.6 Private channels (M).** `channel_create` with visibility; `conversation_member_add/_remove`
generalised from `direct.rs:545`; manager role; system entries; web create dialog and members
tab. Depends on C2.1. Accept: `conversation_privacy.rs` cases for create, add, remove (removed
member loses timeline, search, notifications, push re-check), and the existing
`apply_arrival_defaults` now has a channel to act on; isolation cases for each new route and tool.

**C2.7 Join, leave, browse, topic (M).** `auto_join`, membership backfill job, `channel_join`,
`channel_leave`, `channel_browse`, `channel_update` (topic, description). Sidebar shows joined
channels; "Browse channels" page (lazy, part `channel.browse`). Depends on C2.6. Accept: after the
backfill every member still sees every existing channel; leaving hides it from the sidebar but
search still finds public messages; a new space member joins auto-join channels only.
**C2.7b** (S): remove the "no rows means everyone" fallback after production backfill completes.

**C2.8 Channel details and tabs (M).** Details panel with Page, Pins, Tasks, Files, Members
tabs; registry parts including `channel.tabs` (C2 owns it); the Page tab reads C1's
`conversation_tabs` and is hidden until C1.26 lands; the Tasks tab only when Tasks is on (R22).
Depends on C2.7, K3 links (falls back to `work_item_links` until K3). Does not depend on
C1.26. Accept: `ChannelDetails.test.tsx` renders only
registered parts; server test that the Tasks tab lists only tasks the viewer can see.

**C2.9 Sidebar sections (S/M).** Tables, operations and tools, Home "Your sidebar" block,
starred, drag order, collapse, unread-only sections; W1's shell places the block. Depends on
C2.3 (counts), W1.11, W1.13. Beside C2.8.
Accept: losing access hides the item and keeps the row; agent tool can arrange its own; registry
test for the new part `home.sidebar_sections`.

**C2.10 Triage, faster (S).** `wake_on_activity`, snooze menu with "until activity", multi-select
and bulk keys, Cleared tab, inline reply on mentions and DMs. Depends on C2.4. Accept:
`notifications.rs` test that a snoozed-until-activity row returns unread on the next occurrence;
`InboxPage` keyboard tests for x/Shift+j/e on a selection.

**C2.11 Saved inbox views (M).** `inbox_views`, filter struct, tabs, density, tools. Depends on
C2.4. Beside C2.10. Accept: filter validation refuses unknown keys; a view filtered to one space
never shows another's rows; reorder persists.

**C2.12 Request cards (S).** The inbox and push place W1.24's `RequestCard({card})`, drawn from
A1's `requests::card`, answering through `request_answer`; push actions for request options.
Depends on A1.8, W1.24 (until then, today's cards). Accept: a request
raised by an agent and one raised by a person render the same card; answering from push uses the
token path (`push/actions.rs`) and re-checks access.

**C2.13 Keyword alerts (S).** Table, settings UI, matching in `fan_out`. Depends on C2.5. Accept:
keyword in a private channel the person isn't in sends nothing; in a muted conversation nothing;
own messages never.

**C2.14 Schedules and away (S).** `notifications.quiet_days`, away-until pauses, urgent
breakthrough setting, digest includes DMs. Depends on C2.15 for "away" (or ships the setting
first). Accept: unit tests next to `notification_settings.rs:860-930` for days and away.

**C2.15 Status (M).** `account_statuses` keyed by account and nullable workspace (R19) with
A2's columns (state, `paused_*`, `set_by`; line up to 120), `status_set` and `status_clear`
(A2 writes agents' lines through the same operation; R12), display in mentions, members,
headers, profiles; clear job. Beside most. Accept: status set by an app acting for a person is
that person's; an agent's status set through its connection; clear_at removes it; a person's
hand-set status shows in every workspace they are in; an agent's work line in workspace A is
not visible in workspace B.

**C2.16 Presence, typing and `@here` (M).** Presence read from K5's `presence::online_in`
(no C2 table), typing through K5's `typing` frame, `@here` meaning online and not away.
Depends on K5.10.
Accept: `@here` notifies only recently active people; typing never reaches someone who can't
read the conversation (re-authorised like other socket events).

**C2.17 Profiles and avatars (M).** `account_profiles`, avatar upload, profile panel for every
account. The profile is global to the account (R19): `account_profiles` is keyed by account
only, and "the spaces you share" lists only shared workspaces' spaces, never the account's other
workspaces. Depends on K1 accounts naming. Beside C2.16.

**C2.18 User group mentions (S).** Resolution, autocomplete, add-to-channel. Depends on K1.27
(`account_groups`, with the `handle` C2 requests). Accept: a group member who can't read the conversation isn't notified; large group needs
`channels.mention_everyone`.

**C2.19 Any emoji and custom emoji (M).** Shared emoji list fixture, reactions widened, custom
emoji upload and picker. Beside most. Accept: parity test Rust/TS on the list; invalid strings
refused; a retired custom emoji still renders on old reactions.

**C2.20 Make from messages: registry and chip (M).** `message_make`, maker trait, task and
decision moved onto it, K3 `made_from` links, chip both ways, selection of several messages.
Depends on K3 links (or writes `work_item_links` meanwhile). Accept: a maker refuses when the
actor can't read one of the messages; the chip hides for a viewer who can't see the task.

**C2.21 Make a page or a record (S).** Page (`page_create from`, C1.25) and record
(`record_create_many`) makers; from a DM the person picks a space. Depends on C2.20, C1.25, T1
record operations.

**C2.22 Make a sheet row or a sticky (S).** Makers on `sheet_update` (rows by key) and
`whiteboard_update_elements`. Depends on C2.20, C1 sheet and whiteboard operations.

**C2.23 Internal link cards (S/M).** Href matching from manifests, `mentions` links, card pieces.
Depends on K3 things, W1.21 (`ThingChip` and cards). Accept: a link to a private thread shows a card only to
people who can read it.

**C2.24 Slash commands from manifests (S).** Object entries (C2 defines the shape), composer reads
them from M1's registry. Depends on M1.4, M1.5 (or manifests directly meanwhile). Accept: switching a module off removes its commands; manifest
test that every `operation` named exists.

**C2.25 Search filters and grammar (M).** Parser in both languages with a parity fixture, HTTP and
tool accept it, filter chips on the search page. Depends on K3.17 and K3.18 for new kinds only. Accept:
`search.rs` tests for each filter; a guest's `in:#other` returns nothing.

**C2.26 Saved searches and jump-to (S).** Table, palette section, `/api/v1/jump`. Depends on
C2.25. Accept: jump endpoint under 10 queries; results access-checked.

**C2.27 Answers with sources (M, later).** Depends on C2.25, the AI gateway, K3.17 and K3.18. Accept:
the context sent to the model contains only hits the viewer can read (a test with a private
message present in the corpus); answers cite.

**C2.28 Drafts across devices (S/M).** `message_drafts`, sync, Drafts and sent view. Beside most.

**C2.29 Speed (M).** Server-Timing, query-count test on timeline, prefetch, telemetry for the
targets, a dashboard row. Accept: the query-count test; telemetry events named in
`telemetry` tests.

**C2.30 Keyboard and the chat gate (S).** Shortcuts: Ctrl/Cmd+K jump, Alt+Up/Down conversations,
Alt+Shift+Up/Down unread (exist), Esc mark read, Shift+Esc mark all read, Up edit last (exists),
R reply in thread on a focused message, E react, T make task, S save for later, Ctrl/Cmd+Shift+A
all unreads, Ctrl/Cmd+Shift+T threads; one shortcut table; checklist section added.

**C2.31 Phone flows (M).** Bottom-sheet message actions, swipe on inbox and DM rows, collapsing
header, one-handed reach checks at 390 px. Depends on W1.29 (bottom bar).

**C2.32 Share and capture (S).** "Send to…" on `/share`, note to self. Depends on K4 personal
space for note to self (falls back to a self-DM).

**C2.33 Offline reading for chat (S).** What chat keeps in W1.31's store and the reworded
service-worker promise; no second persister. Depends on W1.31. Accept: a test that sign-out
clears chat's entries; nothing persisted with the person's setting off or the workspace's switch
off.

**C2.34 Push that acts (S).** DM Reply and Mark read actions, Badging API from C2.4's count,
delivery and tap-through telemetry, `push_subscriptions.transport` column. Accept: reply from
push leaves the DM read (C2.3 R2) and clears its notification.

**C2.35 Agents in the room (M).** Run chip, collapsed multi-agent status row, "Stop all", A2's
limit message. Depends on A1.3 (`messages.agent_run_id`), A2.8 (`reply_stop_all`), A2 limits.
Accept: Stop all cancels every pending and claimed request in the conversation for a person
with `conversations.stop_agents`, and equally for an agent with it; without it, refused.

**C2.36 Threads view and outcome polish (S/M).** Cross-space Threads view of followed threads,
outcome under the origin message. Depends on C2.5.

**C2.37 Guests (S).** Lists, browse, search, mentions honour the guest role. Depends on K1 guest
role and SQL filter, C2.7. Accept: the guest isolation case in 3.6.

**C2.38 Shared channels (L, later, sketch only).** Split into slices when picked up.

**C2.39 DMs belong to their participants (L; R20).** Section 3.12: DMs and groups in the
direct scope, `dm_open` one per pair, `group_open`, the who-may-DM-whom check (shared workspace,
`direct.start` in it, the other's `dm_policy`, blocks), "ask me first" requests, sending refused
without a shared workspace, the same DM list in every workspace, `UnreadScope::Direct` and
`OtherWorkspaces`, the `kept` workspace setting, existing DMs labelled where a pair has two.
Every operation has an agent tool.
- Depends on: K4's direct scope (the reserved row and DMs out of the hidden project), K4.12's
  permission, C2.3 (counts), P0.0.
- Accept: `tests/integration/direct_scope.rs`: starting a DM twice returns the same one; the
  same DM appears in both workspaces' sidebars; with `ask`, the first message is a request and
  nothing else arrives until accepted; a block refuses sends both ways and hides the DM; after the
  two stop sharing a workspace the DM is readable and sending is refused; in a `kept` workspace a
  new DM carries the workspace id and stops on leaving; a workspace owner can't read, list or
  search a personal DM (isolation case over HTTP and MCP); no membership row references the
  direct scope.

**C2.40 Chat standing alone (M; R22).** Section 3.13: Home as the conversation list when only
Chat is on, invite by link, no project, task, agent or run words in C2's screens, notification
text, push and digests for that workspace, makers and slash commands filtered by enabled
modules, the phone at 390 px.
- Depends on: M1.17 (the "Friends" shape), C2.9, W1.13; K4's invitation link token (R30).
- Accept: W1.39's "Friends" spec passes in both runs (people only; with an agent member, the
  word only on its badge and profile) (it is `test.fixme` naming this slice until then); an
  email digest for a chat-only workspace of people only contains none of the four words; an
  invite link opened on a phone lands in the conversation list.

## 6. Guard rails

- **Unread rules suite** `tests/integration/unread_rules.rs`: R1 to R10, each through the web
  route and an agent tool, so the two can't diverge again.
- **Read path lint**: a test (style of `agent_parity.rs`) that fails when a file other than
  `read_state.rs` (and migrations) writes `conversation_read_states`.
- **Listing lint**: a test that greps server SQL for `kind = 'channel'` / `kind = 'thread'` /
  `FROM conversations` in a query that returns rows to a client and fails unless the query also
  calls `conversation_readable_by` (or K1's filter), with an allow-list file for lock and write
  paths. This closes the class of C2.1.
- **Private-conversation net**: extend `workspace_isolation.rs` with a "same workspace, not a
  member" pass over every route that returns conversations, messages, threads, files, search hits,
  sidebar rows, inbox rows, link cards or chips (the health check found no in-workspace privacy
  net; H report).
- **Notification kinds table**: a unit test that every kind has a group, an attention value and
  push defaults (extending `every_kind_has_a_group_and_defaults_keep_the_inbox`,
  `notification_settings.rs:847`).
- **Chat surfaces checklist** added to `docs/frontend/polish-checklist.md` as items 17 to 22:
  (17) unread follows the rules: the page marks seen only when visible and never moves the New
  line under the reader; (18) every list of conversations shows muted rows dimmed and private
  ones with a lock; (19) every action has a key and is in the shortcut sheet; (20) it works at
  390 px one-handed: primary actions in the lower half, long-press for the rest; (21) offline:
  sending queues visibly and nothing is lost on reload; (22) agents appear with their name, a
  working line and a Stop, with no vendor name in copy. `registry.test.tsx` already fails on
  unregistered rows and tabs.
- **Perf**: the timeline query-count test (C2.29) and the existing bundle budgets; new chat
  pages are lazy.

## 7. Risks and open questions

- **Notification volume after C2.5.** DMs and followed threads now notify, so people who were
  used to silence get pushes. Notice: push volume per person in telemetry and mute rates. Fallback:
  `thread_reply` push default stays off; the DM push default is a workspace setting.
- **Backfills on the live server.** Channel membership rows are channels × members; on one server
  this is small, but the job is batched and resumable, and readers tolerate partial state.
  Notice: the backfill cursor and a count check. Fallback: leave `auto_join` channels row-less
  longer (the fallback reader).
- **The suspected direct-project sidebar leak** is P0.0's (R10); C2.1 keeps a failing-first
  test for the direct-project sidebar in case P0.0 missed it.
- **Typing and presence across processes** depend on K5.10; before it, typing shows only for
  people on the same process. Acceptable degradation; documented.
- **Offline reading stores message text on a device.** Settled by the accepted end-state change
  (opt-in per person, workspace switch, cleared at sign-out; W1.31).
- **DMs outside any workspace (R20)** make "who can read this" depend on participation alone; a
  bug there leaks across workspaces. Notice: C2.39's isolation cases and K4's guard that no
  listing of the direct scope is unfiltered. Fallback: a workspace can choose `kept`.
- **Makers spread across modules** could reintroduce per-module copies. The trait plus one
  operation keeps access and linking in one place.

Owner questions, with recommendations:

1. Should DMs notify by default (push on)? Recommend yes; it's what every chat user expects.
2. Should replying in a thread follow it automatically? Recommend yes, with push off by default.
3. Offline reading: settled (opt-in per person, a workspace can switch it off).
4. Native wrapper for iOS? Recommend no for now; decide after C2.34's measurements.
5. Should `@here` mean "active now" (online by K5's presence and not away) rather than an
   alias of `@everyone`? Recommend yes.

## 8. Touch points

**Needs from:**

- **P0**: P0.0 (listing leak), P0.4 (agent comment mentions), P0.5 (`mark_seen`), P0.28
  (`conversation_readable_by`); C2.1, C2.2 and C2.5 keep only what goes beyond them (R10).
- **K1**: `conversation_readable_by` folded into its access SQL filter; permissions *new*
  `channels.browse`, `channels.create_private`, `channels.mention_everyone` (not in K1's
  catalogue yet); `channels.moderate` covering comments conversations; the guest role; groups as
  `account_groups`/`account_group_members` with a `handle` (requested); `effective_space_role`
  replacing the pasted direct-space CASE; `conversations.stop_agents` (K1.14).
- **K2**: operation definitions for every new write (listed in 3.2 to 3.9); until then C2 writes
  shared functions with an HTTP route and an MCP tool each.
- **K3**: `links` with kinds `made_from`, `mentions`; thing registry `href` matching for link
  cards; the search index and kinds (K3.17, K3.18); events `thing.made_from_message`.
- **K4**: DMs out of the hidden project (fixes U7 and lets C2.1's direct-project refusal go);
  the direct scope (R20: the reserved `kind = 'direct'` row) and `direct.start` (K4.12);
  `conversation_member_visible`; spaces for "channels in a space"; the workspace's own space for
  a chat-only workspace (R22); personal space for note to self.
- **K5**: protocol 2's `typing` frame and `presence::online_in` (K5.10); live audiences for the
  direct scope and other workspaces' unread counts (R20); leases for the backfill jobs.
- **A1**: `requests::card` and `request_answer` (A1.8); `messages.run_id`; run activity for the
  working line; run cancel.
- **A2**: agent status writes into C2's `account_statuses` (no A2 table; R12), `status_set` as
  one operation, `presence_set` publishing K5's `typing` frame; `follows` for things; the
  notification kinds `connection_problem`, `agent_unresponsive`, `budget_warning`; loop-limit
  state and an operation to allow more turns; stop-all; delivery of DM, mention and keyword
  events to agents (C2 computes recipients; A2 delivers); whether an agent's subscription
  includes DMs (R20).
- **M1**: slash entries and makers as module registrations (`Registrar::maker`); the shapes,
  "Friends" in particular (M1.17); panels for channel tabs from modules
  (`conversation.tab` slot).
- **C1**: `page_create {from: {messageIds}}` with attributed blocks, `sheet_update` rows by key,
  `whiteboard_update_elements`, `conversation_tabs` (C1.26); task comments as conversations.
- **T1**: `record_create_many`; list piece for the channel Tasks tab.
- **W1**: phone bottom bar (W1.29), shell placement of "Your sidebar" (W1.13), `RequestCard`
  (W1.24), card and list pieces, the live client, offline (W1.31), the DM routes and switcher
  marks (W1.14a), the shape walk test (W1.39).
- **H1**: none beyond export including the new tables (sidebar, inbox views, statuses, keywords,
  custom emoji, drafts).

**Provides to:**

- `read_state::{mark_seen, unread_summary}` for everyone who shows unread (briefing, A2).
- `inbox::attention` for badges anywhere (W1 shell, push).
- `account_statuses` for A2's agent status and W1's identity chips (presence is K5's).
- The one `made_from` helper for makers and C1's `page_create from`.
- Part `channel.tabs` and the channel header that draws C1's tabs.
- DMs owned by their participants (C2.39) and chat standing alone (C2.40).
- The `MessageMaker` contract for C1, T1 and any module.
- The search query grammar for K3's index and agents' `search` tool.
- `conversation_readable_by` as the interim narrow filter for K1 to absorb.

## 9. Changes to the end state

1. Section 8, Inbox row, "Biggest gaps": digests and quiet hours exist
   (`notification_email.rs`, `notification_settings.rs:214-275`). Proposed: "Saved inbox views,
   snooze until something happens, keywords, DMs and followed threads that notify, badges that
   count conversations."
2. Section 3, Agents, **Status**: make it one status for every account. Proposed: "**Status**:
   one line per account (person or agent) with an optional 'away until'. An agent or its host
   keeps an agent's current ('Working on AWD-142', 'Asleep until 09:00', 'Paused'); a person sets
   their own."
3. Section 8, Chat row: add "an unread state defined by written rules (own messages and run
   events never unread a conversation), DMs and followed threads that always reach you".
4. Section 5, The web app: "Installable on a phone, with push" — add "and, by a person's choice,
   recent conversations readable offline on that device."
5. Section 8, Chat strong-today list should also note a privacy item: listing queries must use
   the one readable filter (the end state already says lists can't drift from single checks; the
   code doesn't yet).

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| C2.1 | Close the listing gaps (beyond P0.0, P0.28) | S | P0.0, P0.28 | C2.2, C2.15, C2.19 |
| C2.2 | One read path (from P0.5's `mark_seen`) | S | P0.5 | C2.1 |
| C2.3 | Unread rules and counts | M | C2.2 | C2.6 |
| C2.4 | Badge and needs-you on the server | S | C2.3 | C2.5 |
| C2.5 | DMs and threads reach you; A2 kinds | M | — (P0.4 does comment mentions) | C2.4, C2.6 |
| C2.6 | Private channels | M | C2.1 | C2.3, C2.5 |
| C2.7 | Join, leave, browse, topic | M | C2.6 | C2.9, C2.10 |
| C2.7b | Remove membership fallback | S | C2.7 in production | anything |
| C2.8 | Channel details and tabs (part `channel.tabs`) | M | C2.7, K3 links (soft) | C2.9 |
| C2.9 | Sidebar sections | S/M | C2.3, W1.11, W1.13 | C2.8 |
| C2.10 | Triage, faster | S | C2.4 | C2.11 |
| C2.11 | Saved inbox views | M | C2.4 | C2.10 |
| C2.12 | Request cards | S | A1.8, W1.24 | C2.10, C2.11 |
| C2.13 | Keyword alerts | S | C2.5 | C2.14 |
| C2.14 | Schedules and away | S | C2.15 (partly) | C2.13 |
| C2.15 | Status (account and nullable workspace; A2's columns) | M | — | most |
| C2.16 | Presence, typing, @here (K5's presence) | M | K5.10 | C2.17 |
| C2.17 | Profiles and avatars | M | K1 accounts | C2.16 |
| C2.18 | User group mentions | S | K1.27 (`handle`) | C2.17 |
| C2.19 | Any emoji and custom emoji | M | — | most |
| C2.20 | Make from messages: registry and chip | M | K3 links (soft) | C2.23 |
| C2.21 | Make a page or a record | S | C2.20, C1.25, T1 | C2.22 |
| C2.22 | Make a sheet row or a sticky | S | C2.20, C1 | C2.21 |
| C2.23 | Internal link cards | S/M | K3 things, W1.21 | C2.20 |
| C2.24 | Slash commands from manifests | S | M1.4, M1.5 (soft) | most |
| C2.25 | Search filters and grammar | M | — (K3.17, K3.18 for new kinds) | most |
| C2.26 | Saved searches and jump-to | S | C2.25 | C2.28 |
| C2.27 | Answers with sources | M | C2.25, K3.17, K3.18 | — |
| C2.28 | Drafts across devices | S/M | — | most |
| C2.29 | Speed | M | — | most |
| C2.30 | Keyboard and the chat gate | S | — | most |
| C2.31 | Phone flows | M | W1.29 | C2.32 |
| C2.32 | Share and capture | S | K4 personal space (soft) | C2.31 |
| C2.33 | Offline reading for chat (W1.31's store) | S | W1.31 | C2.31 |
| C2.34 | Push that acts | S | C2.3, C2.4 | C2.31 |
| C2.35 | Agents in the room | M | A1.3, A2.8, A2 limits | C2.36 |
| C2.36 | Threads view and outcome | S/M | C2.5 | C2.35 |
| C2.37 | Guests | S | K1 guest role, C2.7 | C2.18 |
| C2.38 | Shared channels (sketch) | L | C2.37, K4 | — |
| C2.39 | DMs belong to their participants | L | K4 direct scope, K4.12, C2.3, P0.0 | C2.40 |
| C2.40 | Chat standing alone | M | M1.17, C2.9, W1.13, invitation links | C2.39 |

## Review changes

- 3.2 and C2.1: the listing fix is P0.0's and `conversation_readable_by` is P0.28's (R10); C2.1 keeps the direct-project refusal, search coverage and private-channel cases.
- C2.2: P0.5's `conversations::mark_seen` moves into `read_state::mark_seen` with push and mark-all-read callers (R10); `mark_read` is K2's `conversation_mark_read` alias.
- 3.4 and C2.5: task-comment mentions are P0.4's (R10); added A2's notification kinds (`connection_problem`, `agent_unresponsive`, `budget_warning`) and `thing_update` from A2's `follows`.
- 3.2 and C2.8: dropped `conversations.page_id` and `pageId`; the Page tab reads C1's `conversation_tabs`; C2 owns part `channel.tabs`; C2.8 no longer depends on C1 (one builder per piece, no cycle).
- 3.6 and C2.15: `account_statuses` absorbs A2's columns (state, `paused_*`, `set_by`, 120-character line) per R12, and is keyed by account and nullable workspace per R19 (global hand-set status, per-workspace work line).
- 3.6 and C2.16: no `account_presence`; presence is K5's `realtime_connections`/`presence::online_in`; typing is K5's `typing` frame; `@here` = online and not away.
- 3.6 and C2.18: groups are K1's `account_groups` (a `handle` requested).
- 3.7 and C2.21/22: makers use K2's names (`sheet_update` rows by key, `whiteboard_update_elements`, `record_create_many`, `page_create from`); one idempotent `made_from` helper that C1 also calls; DM-made things go to a picked space (R20).
- 3.5 and C2.12: the card is W1.24's `RequestCard` drawn from A1's `requests::card` (R17).
- 3.10 and C2.33: offline is W1.31's single mechanism, opt-in per person with a workspace switch (accepted change); phone bar aligned with W1 (Home, Inbox, Messages, Search, You).
- New 3.12 and slice C2.39: DMs belong to their participants (R20): direct scope, one DM per pair, who may DM whom (shared workspace, role, `dm_policy`, block), readable for good but sending needs a shared workspace, one list everywhere, switcher counts, `kept` setting.
- New 3.13 and slice C2.40: chat standing alone (R22): Home as conversation list, invite links, no module words when off, phone.
- C2.17: profiles global to the account; never list other workspaces (R19).
- Depends-on columns made real: C2.9 on W1.11/W1.13, C2.12 on A1.8/W1.24, C2.16 on K5.10, C2.18 on K1.27, C2.24 on M1.4/M1.5, C2.25/C2.27 on K3.17/K3.18, C2.31 on W1.29, C2.33 on W1.31.
- Touch points, risks and owner questions updated for the above.

### Second pass

- 3.2: events use K3's names (`conversation.member.added/removed`, `conversation.visibility.changed`); typing uses `realtime::publish_ephemeral`; online counts agent streams (kernel review).
- 3.12 and C2.39: one personal DM per pair plus at most one kept DM per pair per `kept` workspace; `dm_open` in a kept workspace returns its DM; no reaching strangers; K4.24 is the server side (R31).
- 3.13 and C2.40: an agent may join a chat-only workspace whatever is on; its kind shows on badge and profile; the join offers switching Agents on; W1.39 runs twice; invitation links are K4's (R30, R31).
- 3.11 and C2.35: Stop and Stop all use `conversations.stop_agents`; C2.35 depends on A1.3 and A2.8 (agents and hosting review).
- 3.6: `status_set` allowed for the account itself, its host or a manager; A2.10 sets `workspace` for work lines.
