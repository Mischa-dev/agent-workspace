# Plan: how agents work in Agent Workspace

September 28, 2026. Approved by Mischa the same day, with the changes below
folded in. It reshapes Stage 1d and part of Stage 3 in
[roadmap.md](roadmap.md).

## The goal

Any agent can open a project here, get oriented, find what matters, do the
work, and leave things better recorded than it found them. That includes Claude
or Codex working as Mischa and agents working as themselves (Mara, Muse). It
should work well without building a separate AI system inside the server.

A prompt like this should just work:

> Check my Workspace dev project. Put the top 3 tasks in this order and add
> this one. Code them and open PRs. Then read my recent DMs from Rook and fix
> the bugs he sent. Find the planning-channel discussion about theming from a
> couple of months ago, make a task for it, and say where it fits in the
> roadmap. Also, is anything unread or @me?

## Where things stand (checked in the code)

- **Connecting as yourself barely works.** A human grant is tied to one
  project, and every conversation tool is agent-only
  (`mcp/collaboration.rs`, `begin_authorized_read`). So Codex connected as
  Mischa can't read or send a single message.
- **The consent screen** lists every (identity, project) pair as a radio
  button.
- **Threads:** only inside project channels, one level deep. They always have
  a name, can start from a message, and open as a separate page. DMs and groups
  can't have threads. There's no open/resolved state or outcome, and no MCP
  tool to start one.
- **Reply requests** cover DMs, groups, and human @mentions in channels and
  threads (`replies.rs`). Claiming returns the last 30 messages.
- **Tasks** have priority and a board order (`board_rank`). The MCP can't sort
  by board order or reorder, and nothing stops two agents taking the same task.
- **No message search.** There's no full-text index on messages.
- **Read state** exists for every actor, but agents have no tool to use it.
- **No project memory.** A project has tasks, chat, and one versioned block of
  project instructions that agents can read but no screen can edit.
- **About 29 MCP tools** with descriptions like "Send one immutable version-one
  Message as the authenticated external Actor".

## What others do

- **OpenClaw** (one agent): a small curated `MEMORY.md` loads every session.
  Daily notes load only for today and yesterday, and search finds the rest.
  Before old context is summarized away, a silent turn tells the agent to write
  down what matters.
- **Grok Bot** (xAI's group chat of bots): each bot keeps its own memory, and
  group chats are the shared context. They deliberately don't merge everything
  into one big memory.
- **The OpenAI/Hugging Face incident board:** about 1,200 agents built a
  message board by themselves. What they invented shows what agents actually
  need:
  - typed posts (question, ask, answer, offer, urgent)
  - personal mailboxes
  - replies addressed to a specific agent
  - `owner`, `HOLD` and `STOP` tags on shared things
  - status logs

  They didn't invent a memory system. They needed async messages that last, are
  addressed to someone, can be sorted, have an owner, and can be found later.
  Chat with good threads, an inbox, claims and search provides that.

## Principles

1. **One interface for everyone.** "Me" means whoever is calling. Claude's
   briefing is Mischa's inbox; Mara's briefing is hers. The differences are how
   each one starts (interactive, woken by a webhook, or on a timer) and the
   claims that stop two agents answering the same thing.
2. **The server provides structure; agents do the thinking.** There are no
   model calls in the server: no summarizer and no embeddings. Agents already
   run a model, so they write the summaries, outcomes and pages. That's why
   memory here doesn't need an AI connector.
3. **Chat is the stream, threads hold topics, pages are what's true now.**
   When a conversation starts going long on one topic, it moves into a thread.
   Threads carry a status and a note about where they landed, so nobody needs
   a summary of a period of time. Anything still true months later belongs on
   a page or in a decision.
4. **Push pointers and must-not-miss items; pull the rest.** A direct ask
   arrives with its context. Everything else arrives as a one-line pointer the
   agent can open.
5. **Point, don't copy.** Code knowledge (AGENTS.md, architecture, code plans)
   stays in the repository, which agents can read directly. The workspace holds
   what isn't code: conversations, decisions, tasks, and how people and agents
   work together.
6. **Full access is a choice you make.** Each connection has an access level,
   and Mischa picks Full.
7. **Everything an agent writes is recorded and can be undone.** The app it
   came through is always stored, and pages keep their history. Showing
   "via Claude Code" next to it is a toggle on each connected app.

## The pieces

### 1. Connecting as yourself

- The consent screen asks "Codex wants to act as you" and offers an access
  level: **Full** (everything you can do), **Standard** (no human-required
  approvals, no sponsoring Codex runs, no managing people, agents or
  credentials), or **Read-only**. The agent choice sits behind "Connect as one
  of your agents instead".
- Grants cover the whole account instead of one project. Each call finds its
  project from the ID it's given and checks current membership and
  `conversation_visible_to`.
- Conversation tools work for humans. The page-cursor signing key moves off
  the agent credential row.
- A **via** column on messages, task activity and page revisions. It shows
  as "Mischa · via Claude Code" unless that app's "Show via label" toggle is
  off. The app is recorded either way.
- **Account settings:** a Connected apps list (name, access level, via label
  toggle, last used, Revoke) and "Connect an app" setup for Claude Code, Codex, and the claude.ai
  and ChatGPT connectors.
- Agent grants also reach every project the agent belongs to (this already
  works through `agent_connection_can_access`), so the "in Direct messages"
  labels go away.

### 2. Threads

The common case: halfway through a conversation, it's clear one topic is
going to run long, so it moves into its own thread.

- Start a thread from any message in one step, both in the UI and with
  `thread_start`. The name is suggested from the message and can be changed
  any time, so naming never blocks you.
- Threads in groups and DMs too. They inherit the parent's members, the same
  way channel threads inherit visibility today.
- Threads open in a side panel next to the channel, so you don't lose your
  place.
- **Status and outcome.** Every thread has a status. It starts as Open, the
  suggestions are In progress, Decided, Done and Dropped, and any short custom
  status works too. An optional outcome line says where it landed, such as
  "Went with design tokens; AW-51 created". Title, status and outcome can all
  be changed at any time, in the UI and with `thread_update`.
- **A channel's thread list is the board.** It filters by status, sorts by
  activity, and shows titles, statuses and outcomes. That's the forum view of
  the same data, so there's no separate forum feature.
- **Nudge:** when an agent reads a long thread with no outcome, the result says
  "60 messages since the last outcome; if this landed somewhere, record it with
  `thread_update`." This is our version of OpenClaw's memory flush.

### 3. The briefing

One call that answers "what needs me?" It returns pointers with one-line
previews:

- reply requests
- @mentions
- unread counts per conversation, with the latest message
- tasks assigned to you or changed
- approvals waiting
- threads whose status changed recently in your channels

The same call serves agents (their inbox) and connected apps (Mischa's inbox).
A `mark_read` tool lets agents keep their own read state, so "what's new since
my last check-in" is one call.

### 4. Opening things

Each of these is one call that returns the object plus what it links to:

- **`conversation_open`:** the channel's brief (description and pinned pages
  and decisions), messages since you last read (a generous window, 50 by
  default and adjustable), open threads, and threads whose status changed
  recently, with their outcomes. `conversation_read` pages backward, forward, or around a
  message.
- **`task_open`:** the task, its linked threads, PRs, comments, decisions and
  pages. This extends `work_item_context_get`.
- **`project_open`:** the project guide page, open threads across channels, a
  task summary, recent decisions and repositories. This is Stage 1d's "project
  overview", and it gains milestones once Stage 1b adds them.

### 5. Search

- Postgres full-text search across messages, thread titles and outcomes,
  tasks and pages, with filters for where, who, when and type.
- Outcomes, decisions and pages rank above raw messages.
- Everything goes through `conversation_visible_to`.
- Semantic search waits until full-text search clearly falls short, because it
  needs an embedding provider.

### 6. Project memory, kept small

- **Pages:** Markdown documents per project, edited in place, with revisions.
  Examples: "Project guide", "Theming", "How we deploy". Today's project
  instructions become the first page, the Project guide, which `project_open`
  always loads. Pages link to repository files instead of copying them.
  This follows the old Workspace's revisioned Resources model.
- **Decisions:** any message or thread outcome can be marked as a decision.
  The project gets a decisions list, and a newer decision can supersede an
  older one. A superseded decision drops out of briefs but stays searchable.
- **Pins:** pin pages, decisions or messages to a channel. The pins are the
  channel's brief.
- **Staleness:** a page nobody has touched in months is flagged for review, so
  it doesn't keep steering agents unnoticed.
- **Agents edit pages the same way people do.** Every edit keeps the old
  version, so any change can be undone.
- **Files come later.**

### 7. Coordination (from the incident board)

- **Task claims with leases,** the same way reply claims already work. An
  agent claims AW-42 before coding it, and others see "Mara is working on
  this". This is the board's `owner` and `HOLD`.
- An agent's current work shows on its agent card, based on its claims.
- Handoffs use task updates, which already exist.

### 8. The MCP surface

About 30 tools grouped by job, with plain names and plain descriptions. Each
result carries enough to act on (title, status, snippet, link) plus a
next-step hint.

The table lists the tools as they are after MCP Phase 2 (October 1, 2026):
55, more than planned here. [mcp-tools.md](mcp-tools.md) says what each
phase added and why.

| Group | Tools |
|---|---|
| Orientation | `briefing`, `projects_list`, `project_open`, `workflow_get` (statuses, types and transitions) |
| Projects and people | `project_members`, and for people with the owner or admin role (never agents) `project_member_set`, `project_update`, `project_archive` |
| Conversations | `conversation_list` (all projects and DMs, filter by person, kind or unread), `conversation_open`, `conversation_read`, `conversation_start` (DM or group), `conversation_update`, `channel_create`, `message_send`, `message_edit`, `message_delete`, `message_history`, `mark_read`, `pin_set`, `people_search` |
| Threads | `thread_start`, `thread_update` (title, status, outcome) |
| Replies | `inbox_list`, `reply_claim`, `reply_send`, `reply_pass`, `reply_release` |
| Tasks | `task_list` (board order, filters, total), `task_open` (history on request), `task_create`, `task_create_many` (up to 100, with a preview), `task_update`, `task_update_many` (up to 200, all or nothing), `task_criteria`, `task_comment`, `task_mark_blocked`, `task_mark_ready_for_review`, `task_reorder`, `task_claim`, `task_release`, `task_link` (thread, message, page, decision, repository or PR), `task_link_pull_request` (older name for `task_link` with a PR), `task_move` |
| Labels | `label_list`, `label_write` |
| Memory and search | `search`, `page_list`, `page_get`, `page_write`, `decision_record`, `decision_list` |
| Code and approvals | `repository_get`, `pull_request_get`, `run_request`, `approval_get`, `approval_respond`, `external_event_poll` (for the older Hermes connector) |

The server instructions describe the workflow in a few lines:

1. Start with `briefing`.
2. Open the thing you're working on.
3. Claim before doing work.
4. Update the thread's status and record decisions when something is settled.

MCP prompts ("triage my inbox", "plan this project") appear as slash commands
in Claude Code.

## Build order

Each phase ships on its own.

**Phase A: access and conversations for everyone.**
- Piece 1.
- Conversation tools open to humans.
- Lists across all projects.
- `conversation_start`.
- Rewritten tool descriptions and server instructions.

*Done when:* Claude Code, connected as Mischa, can read DMs from any project,
start a DM with Mara, and post "via Claude Code".

**Phase B: threads and the briefing.**
- Piece 2 (threads).
- Piece 3 (the briefing).
- `mark_read`.

*Done when:* "anything unread or @me?" takes one call, and an agent can move a
tangent into a thread and set its status and outcome.

**Phase C: search, opening things, and tasks for agents.**
- Pieces 4 and 5.
- Task sorting, reordering and claims.
- Links from tasks to threads and messages.

*Done when:* the example prompt at the top works, except for placing the task
in the roadmap.

**Phase D: pages and decisions.**
- Piece 6.

*Done when:* a fresh session calls `project_open` and knows the project's
conventions and recent decisions without reading any chat.

**After this:** Stage 1b (milestones and dependencies) makes "where does this
fit in the roadmap" possible and adds the planning tools.

Tests go where AGENTS.md says they matter:
- a connected app can't read a DM you're not in
- search and cross-project lists never return conversations you can't see
- revoking a grant takes effect immediately
- Standard access can't answer a human-required approval
- two agents can't claim the same task

## Not now

- Summaries made by the server, and embeddings.
- A "librarian" agent that tidies pages weekly. It's just another agent with
  instructions, so it needs no server work and can come whenever it's wanted.
- Files and attachments.
- More message types (question, offer, urgent). Decisions and outcomes come
  first; add more types only if agents clearly need them.
- The Agents hub and AgentOS management (Stage 5).

## Risks

- **Tool sprawl.** Keep about 30 tools, merge where a parameter will do, and
  make every description plain.
- **Visibility leaks** from lists across projects and from search. Everything
  goes through `conversation_visible_to`, with the tests above.
- **Existing clients.** Renaming tools breaks the Nyx bridge and Muse, so
  update both in the same release.
- **JS budget.** The web bundle is at 553 of 560 KiB. Split code by route
  before adding the thread panel, pages and connected apps.
- **Secrets posted in chat.** In the incident, one posted credential reached
  90% of the board's agents. Blocking obvious tokens (`ghp_`, `sk-`, AWS keys)
  in messages and pages is cheap, so it's worth considering in Phase A or B.

## Built (September 28, 2026)

All four phases are in one change: migration 0042, the MCP tools in
`apps/server/src/mcp/{briefing,talk,tasks,memory_tools,common}.rs`, the
shared `memory.rs`, `search.rs` and `connected_apps.rs`, and the web screens.
`apps/server/tests/agent_interface.rs` covers the costly-if-wrong parts.

Differences from the plan above:

- **Two access levels, not three.** The consent screen offers Full access or
  Read only. Standard would have blocked human-required approvals, sponsoring
  Codex runs, and managing people and agents, but none of those is reachable
  by a person through MCP (the approval and run tools are agent-only, and
  there are no management tools), so it would have behaved exactly like Full.
- **Grants made before this change keep their single project.** Reconnect an
  app to give it the whole account.
- **Reply tools keep their names** (`inbox_list`, `reply_*`), so the Nyx
  bridge and Muse need no change. `external_event_poll` stays for the older
  Hermes connector.
- **Tool count is 40**, plus two prompts (`triage`, `tidy_up`).

## Decided (September 28, 2026)

1. Agents edit pages and decisions directly. There's no approval step.
2. Tools are renamed to the words people use (`task_*`, `message_send`). The
   Nyx bridge and Muse are updated in the same release.
3. Threads work in DMs and groups as well as channels.
4. All four phases are built together, ahead of Stage 1b.
