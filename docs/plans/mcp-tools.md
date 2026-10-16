# Plan: an MCP surface that can do everything the app can

September 30, 2026. A draft for Mischa; nothing here is approved yet. It
follows [agent-interface.md](agent-interface.md), which built the current 37
tools, and it feeds Stage 1d of [roadmap.md](roadmap.md).

## Why

On September 30, Claude Code connected as Mischa and tried a simple job: merge
the old Agent Workspace project (AW) into Agent Workspace Development (AWD),
then rename AWD. It could do almost none of it:

- `briefing`, `project_open` and `task_open` all failed in Claude Code. Their
  results left out empty lists that their outputSchemas said were required.
  This is fixed on `claude/mcp-output-schemas`; see "Done" below.
- There is no way to move a task, rename or archive a project, or even list a
  project's statuses.
- `task_open` was the only way to read a task's description, so without it
  there was no safe way to copy tasks by hand.

The rule from now on: **if a person can do it in the web app, an agent with
the same access can do it through MCP**, except for the few things listed under
"Never through MCP".

## Principles

These add to the ones in agent-interface.md.

1. **Parity by default.** Each feature ships with its MCP tool, not after it.
2. **Every result is checkable.** Results must match their outputSchema. The
   agent interface test checks every call, so this can't silently break again.
3. **Find things the way people say them.** Every tool takes an ID, a link, or
   a human key: `AW-12`, project `AWD`, `#general`, `@mara`.
4. **Writes return the thing they changed:** its new version, link and key, so
   the next call needs no extra read.
5. **Errors say what to do next.** "No status called Doing here. Statuses: Backlog,
   Todo, In Progress, Review, Done." beats `invalid_status`.
6. **Nothing is lost.** Archive instead of delete, keep revisions on edits,
   and keep old keys working after a move or rename.
7. **Every write follows the same pattern:** `clientRequestId` for safe
   retries, `expectedVersion` on anything versioned, `mark_via` attribution,
   and an audit row. (`thread_update` misses the first three today.)
8. **Accurate hints.** Archive, remove and move tools carry
   `destructive_hint`, so clients ask before running them.
9. **Many tools are fine if each one is clear.** Claude Code and similar
   clients load tool schemas on demand, so a long list costs little. Confusing
   overlaps cost more. Merge tools only when one parameter does the job.

## The catalog

Key: ✓ exists, ~ exists but changes, + new. "People" means a person connected
through an app with Full access; "agents" means agent accounts. Both are still
limited by their role in the project.

### Orientation

| Tool | | What it does |
|---|---|---|
| `briefing` | ✓ | What needs you. Fixed. |
| `projects_list` | ~ | Add `includeArchived`, and each project's key and your role. |
| `project_open` | ~ | Also return the workflow (statuses with IDs and categories), labels, member count, and archived state. |

### Projects and people

| Tool | | What it does | Who |
|---|---|---|---|
| `project_create` | + | Name, key, description, optional guide page. | People with the org role to create projects |
| `project_update` | + | Rename, change the description, change the key (old keys keep working; see decision 2). | Owner or admin |
| `project_archive` | + | Archive or restore (`archived: false`). Refused while Codex runs are active. Destructive. | Owner or admin |
| `project_members` | + | Members with roles, kind (person or agent), and last active. | Anyone who can see members |
| `project_member_set` | + | Add an org member, change a role, or remove someone. Destructive when removing. | Owner or admin, people only |
| `workflow_get` | + | Task types, statuses (with category and order), and allowed transitions. | Everyone |
| `workflow_update` | + | Add, rename, reorder or retire statuses and types. Phase 5. | Owner or admin |

### Tasks

| Tool | | What it does |
|---|---|---|
| `task_list` | ~ | Add filters: label, type, priority, created by, updated since, blocked, claimed, archived, and several projects at once. Return a total count. |
| `task_open` | ~ | Add `include: ["history"]` for the full timeline (status changes, edits, who and when). |
| `task_create` | ~ | Add status, type, labels, acceptance criteria and board position. |
| `task_create_many` | + | Create up to 100 tasks, with subtasks referring to each other inside the batch. `preview: true` checks without writing. This is the "batch plan import" from roadmap 1d. |
| `task_update` | ~ | Add type, parent (re-parent or make top-level), labels to add or remove, `archived`, and status with an exact position. |
| `task_update_many` | + | Apply one change to many tasks, all or nothing: "move these 8 to Done", "label all of these `mobile`". |
| `task_move` | + | Move tasks to another project. Same row and history; old keys keep resolving (decision 1). Destructive. |
| `task_criteria` | + | Acceptance criteria: add, edit, check, uncheck, remove and reorder, in one call with a list of operations. |
| `task_comment` | ✓ | |
| `task_mark_blocked`, `task_mark_ready_for_review` | ✓ | Unchanged, so the Nyx bridge keeps working. |
| `task_reorder` | ~ | Add "before task X" and "after task X" alongside top and bottom. |
| `task_claim`, `task_release` | ✓ | |
| `task_link` | ~ | Add repositories and pull requests. `task_link_pull_request` stays as an alias for one release. |
| `label_list`, `label_write` | + | Project labels: create, rename, recolor, archive. Needs a labels table. |
| `task_depend` | + | "Blocks" and "blocked by", rejecting cycles with the path shown. Stage 1b. |
| `milestone_list`, `milestone_write` | + | Milestones with target dates and progress. Stage 1b. |

### Conversations

| Tool | | What it does |
|---|---|---|
| `conversation_list`, `conversation_open`, `conversation_read`, `conversation_start` | ✓ | |
| `channel_create` | + | Name and topic. |
| `conversation_update` | + | Rename, set the topic, archive or restore any channel, thread or group. For threads it also sets status and outcome, so `thread_update` becomes an alias. |
| `conversation_members` | ✓ | Add or remove people and agents in a group, or leave one. People add and remove anyone; agents can only leave, whatever their role. At most 20 members. Destructive. |
| `message_send` | ✓ | |
| `message_edit` | ✓ | Edit your own message. The old text is kept as a revision and the message shows "edited" (decision 3). |
| `message_delete` | ✓ | Remove your own message, leaving "message removed" in its place, with its text and every revision's text deleted; who wrote each version and when is kept. Owners and admins (people, not agents) can remove anyone's message, for example a leaked secret. Agents remove only their own, whatever their role. Destructive. |
| `message_history` | ✓ | Every version of a message, oldest first; a removed message's versions show as redacted. |
| `message_react` | ✓ | Add or take away an emoji reaction from a fixed list of 12. Agents signal 👀 (working on it) or ✅ (done) without posting "on it". Anyone who can post can react; nobody is notified. |
| `pin_set` | + | Pin or unpin a message, page or decision in a conversation. Today pages and decisions can only be pinned, never unpinned. |
| `mark_read` | ✓ | Mark read, or `unread: true` to mark it unread again from a message, to come back to later. |
| `thread_start` | ✓ | |
| `thread_update` | ~ | Becomes an alias of `conversation_update`, and gains idempotency and via attribution. |
| `people_search` | ✓ | |

### Replies (agents)

| Tool | | What it does |
|---|---|---|
| `inbox_list`, `reply_claim`, `reply_send`, `reply_pass`, `reply_release` | ✓ | `reply_send` takes `final: false` for an interim message that keeps the claim. |
| `reply_progress` | + | A one-line status on a claimed request, shown next to the agent's name; renews the claim. See [agent-tag.md](agent-tag.md). |
| `reply_stop` | + | People: stop an agent's work on a request, as the Stop button does. Its queued follow-ups stop too. See [agent-runner.md](../agent-runner.md). |
| `conversation_continue` | ✓ | Let the agents in a group take more turns, as the web app's Continue button does. People only. |

### Memory and search

| Tool | | What it does |
|---|---|---|
| `search` | ✓ | |
| `page_list`, `page_get`, `page_write` | ✓ | |
| `decision_record` | ✓ | |
| `decision_list` | + | Decisions in a project, with `includeSuperseded`. |
| `decision_update` | + | Edit the detail, supersede with an existing decision, or withdraw. |

### Code, runs and approvals

| Tool | | What it does |
|---|---|---|
| `repository_list` | + | Repositories in a project, with mirror state. |
| `repository_get` | ✓ | |
| `pull_request_list` | + | Across repositories, filtered by state, checks, review, linked task, and "needs me". |
| `pull_request_get` | ✓ | |
| `run_start` | + | A person starts a Codex run on a thread or task. Agents keep `run_request`, which asks a person to approve. |
| `run_request` | ✓ | |
| `run_list`, `run_get` | + | Runs with status, and the result and errors of one run. |
| `run_control` | + | Interrupt, retry or acknowledge a failure. |
| `approval_list` | + | Approvals waiting on you. |
| `approval_get`, `approval_respond` | ~ | Open to people with Full access, including approvals that require a person. |
| `external_event_poll` | ✓ | Legacy; kept for the Hermes connector. |

### Agents

| Tool | | What it does |
|---|---|---|
| `agent_list` | + | Agents you work with: their projects, last check-in, and what they've claimed. The start of the Stage 5 hub. |

### Prompts (slash commands in Claude Code)

`triage` and `tidy_up` exist. Add:
- `standup`: what changed since yesterday.
- `plan_project`: turn a goal into tasks with `task_create_many`, with a preview.
- `review_prs`: open PRs, failing checks, and the tasks they belong to.
- `merge_projects`: a guided version of the September 30 job.

### Never through MCP

These stay in the web app, because they create or reveal secrets, or they
give out access without a person seeing it:

- creating agent accounts, and issuing, rotating or revoking tokens
- connecting apps, and OAuth or WorkOS settings
- execution-profile logins and API keys
- the GitHub App installation and webhooks
- invitations to people outside the organization

## Decisions for Mischa

1. **How a task moves between projects.** Recommended: a real move. The task
   keeps its row, history, comments and links, and changes `project_id`. It
   keeps its number when that number is free in the target project, and gets
   the next free number otherwise. Every key it ever had goes in a new
   `work_item_key_aliases` table, so `task_open AW-7` still works. Status and
   type map by name, then by category. An assignee who isn't in the target
   project is cleared, with a comment saying so. Subtasks move with their
   parent.
   - *Cost:* one migration. Twelve foreign keys on tables that hang off
     `work_items(project_id, id)` gain `ON UPDATE CASCADE`.
   - *Alternative:* copy the task into the target and archive the original. It's
     simpler, but the history splits in two and links point at the archived
     copy.
2. **Changing a project's key.** A trigger blocks this today once any task is
   numbered. Recommended: allow it, and record the old keys of every task in
   the same alias table. Keys must be unique in the organization, so taking a
   key from an archived project first renames that project's key, for example
   to `AWOLD`.
3. **Editing and deleting messages.** Messages can't change at all today; a
   trigger enforces it. Recommended: allow authors to edit, with every
   revision kept and visible, and allow deleting, which leaves a "message
   removed" marker. Owners can remove anyone's message.
   - The main reason is a credential posted in chat. agent-interface.md lists
     that as a risk, and right now there's no way to take one down.
   - Search and briefings use the current text. Revisions stay in the audit
     log.
4. **Old tool names.** Recommended: `thread_update` and
   `task_link_pull_request` stay as aliases for one release, then go once the
   Nyx bridge and Muse have moved.
5. **Admin through MCP.** Recommended: project and member admin is open to
   *people* with Full access and an owner or admin role. Agents never manage
   projects or members, even if they're given the role.

### For the September 30 merge specifically

AWD-4 to AWD-9 (this plan's tasks) now use numbers 5 to 7. So AW-5 to AW-7
move in as AWD-10 to AWD-12, with their old keys kept as aliases.

That makes key `AW` for the renamed project awkward: its own AW-5 would clash
with the alias for the old AW-5. Two clean options:

- **Keep key `AWD`** and only rename the project to "Agent Workspace".
  Recommended: nothing clashes.
- **Take key `AW`, and drop the aliases of a project whose key is reused.**
  Old links to AW-5 to AW-7 would then open the new tasks with those numbers.

Either way, archive the old project first. Its key is released as `AWOLD`, and
its aliases resolve as AWOLD-5 to AWOLD-7.

## Build order

Each phase ships on its own and ends with a check you can try.

**Phase 0: done.** The outputSchema fix. A unit test covers the three affected
results, and the agent interface test now checks every tool result against
its schema.

**Phase 1: run the workspace from chat.**
- `workflow_get`, `project_update`, `project_archive`
- `task_move` and the key aliases (decisions 1 and 2)
- `project_members` and `project_member_set`
- `channel_create`, `conversation_update`, `pin_set`, `decision_list`
- `projects_list` and `project_open` changes

*Done when:* the September 30 merge works end to end with one prompt (AWD-5).

**Phase 2: tasks at full strength.**
- the `task_create`, `task_update`, `task_reorder` and `task_link` changes
- `task_create_many` and `task_update_many`
- `task_criteria` and `task_open` history
- labels

*Done when:* an agent imports a Markdown plan as tasks with a preview, then
reorganizes the board without the web app.

**Phase 3: conversations.**
- `message_edit` and `message_delete` (decision 3)
- reactions
- `conversation_members`, `conversation_continue`, and marking unread

*Done when:* an agent can take down a pasted token and react ✅ to the message
that asked for it.

**Phase 4: code and runs for people.**
- `run_start`, `run_list`, `run_get` and `run_control`
- approvals for people
- `pull_request_list`, `repository_list` and `agent_list`

*Done when:* "start Codex on AW-6, tell me when the PR's checks pass" works as
Mischa.

**Phase 5, with Stage 1b:**
- `task_depend`, milestones and `workflow_update`
- the new prompts
- pages exposed as MCP resources, so clients can @-mention them

## Tests

Following AGENTS.md, tests go where a bug would be silent and costly:
- Every tool result matches its outputSchema. This is already in place.
- A task move never makes a task visible to someone outside both projects,
  and old keys resolve only for people who can see the task.
- Admin tools refuse agents, Read-only access, and members without the owner
  or admin role.
- A deleted message disappears from search, briefings and `conversation_read`
  for everyone.
- Bulk updates are all or nothing, and replaying the same `clientRequestId`
  changes nothing.
- Archiving a project with active runs is refused.

## Risks

- **Moving tasks touches twelve foreign keys.** Do it in one migration with a
  test that moves a task carrying every kind of child row.
- **Admin tools are powerful.** Destructive hints, owner-only checks, audit
  rows and the via label keep every change visible and reversible.
- **Mutable messages change an old guarantee.** Revisions keep the history,
  and the trigger is replaced rather than dropped, so only the edit and delete
  commands can change a message.

## Decided (September 30, 2026)

1. Tasks move for real: same row and history, a new number, and old keys kept
   as aliases.
2. The merged project keeps key `AWD`; only its name changes to "Agent
   Workspace". Changing a project's key isn't needed for Phase 1, so
   `project_update` changes the name and description only, for now.
3. People and agents can edit and delete their own messages. Revisions are
   kept, and owners can remove anyone's message.

Decisions 4 and 5 go ahead as recommended unless Mischa says otherwise.

## Built so far (September 30, 2026)

On `claude/mcp-phase-1`: `workflow_get`, `project_members`,
`project_member_set`, `project_update` (rename), `project_archive` (archive
and restore), `channel_create`, `conversation_update`, `pin_set`,
`decision_list`, `projects_list` with `includeArchived`, and project keys
(such as `AWD`) accepted wherever a `projectId` is. That's 49 tools. Channel
archiving and membership changes now share one code path with the web app.

## Decision 1, revisited (September 30, 2026)

A real move isn't possible without taking apart guarantees the database
was built on. A task's links point at things that stay in the old project:
conversations, messages, repositories and pull requests. Comments, mentions
and acceptance criteria require their authors to be members of the task's
project. And comments, mentions and message references are immutable, with
triggers that reject any update, including a cascading `project_id` change.

Options:

1. **Carry over, then archive the original (recommended).** The new task
   gets everything that can move:
   - title, description, priority and labels
   - status and type, matched by name, then by category
   - the assignee, if they're in the target project
   - acceptance criteria and subtasks
   - every comment, quoted with its original author and date

   The original is archived with "Moved to AWD-12" and keeps its full
   history and links. Its old key goes in `work_item_key_aliases`, so
   `task_open AW-7` and old links open the new task. Links to threads and PRs
   are listed on the new task, pointing back to where they still live.
2. **Loosen the database to allow a real move.** This means dropping the
   immutability triggers for moves, relaxing the membership checks on
   comment authors, and rewriting or dropping cross-project links. It's a lot
   of risky work for a rare operation.

Mischa chose option 1. `task_move` carries a task and its subtasks over and
archives the originals. Migration 0043 adds `work_item_key_aliases`. A moved
task's old key opens the new one for anyone who can see it; anyone else gets
the archived original, which they could always see. This also means
`taskId`, `workItemId` and `parentId` take task keys in every tool now. Moving
a task again returns where it went. That brings the count to 50 tools.

## Built in Phase 2 (October 1, 2026)

On `claude/mcp-phase-2`, tasks at full strength. Five new tools bring the
count to 55, which `tests/integration/m6_mcp_foundation.rs` checks:

- `label_list` and `label_write`: project labels with a name, color and
  description. Labels are archived and restored, never deleted. Migration
  0044 adds them. A project has at most 200 active labels, and a task at
  most 20.
- `task_criteria`: add, edit, check, uncheck, remove and move acceptance
  criteria in one call, all or nothing. A criterion is named by its ID or
  by its position in the version you read.
- `task_create_many`: up to 100 tasks in one project, with subtasks through
  `parentRef` (an earlier task in the list) or `parentId` (an existing
  task). `preview: true` runs the same code and rolls back, so the keys it
  shows are the ones the tasks would get.
- `task_update_many`: one change to up to 200 tasks, all or nothing: status,
  priority, assignee, type, labels, or archived. It has a preview too.

Changed tools:

- `task_create` takes status, type, labels, acceptance criteria and a place
  in its column. Subtasks now keep their priority and assignee; they were
  dropped before.
- `task_update` takes type, parent (`parentId`, or `clearParent` for
  top-level), labels to add and remove, a place in its column, and
  `archived`. Any mix of changes is one new version.
- `task_list` filters by several projects at once, labels, type, priority,
  who created it, updated since, blocked and claimed. It can list archived
  tasks and returns a total.
- `task_open` shows labels, and the task's timeline with
  `include: ["history"]`.
- `task_reorder` places tasks just above or below another task.
- `task_link` links repositories and pull requests too.
  `task_link_pull_request` stays as its older name (decision 4).
- `project_open` lists the project's labels, and `task_move` carries labels
  across by name.

This phase's check passes as a test: an agent imports a plan with a preview,
then reorganizes the board without the web app
(`an_agent_imports_a_plan_and_reorganizes_the_board` in
`tests/integration/mcp_task_batches.rs`).

Differences from the plan:

- **Labels can only be managed through MCP for now.** The web app shows them
  as read-only chips on the board, the list and the task page. There's no
  labels screen yet.
- **Only `task_create_many` creates labels,** and only with
  `createMissingLabels: true`. `task_create` and `task_update` refuse a label
  the project doesn't have and list the ones it does; `label_write` creates
  one.
- **A bulk status change is one step.** `task_update_many` moves every task
  straight to the new status. If any task can't go there directly, nothing
  changes, and the refusal lists the steps that task needs.
- **`archived` goes on its own.** `task_update` and `task_update_many` don't
  mix archiving or restoring with other changes. Make those with the new
  version.
- **Linking changes a task's version.** Every `task_link` change is now a
  version, an activity, an audit row and an outbox event, like other task
  writes. Its `expectedVersion` is optional, and a link that's already there
  answers `changed: false`.
- **The history comes in pages:** the latest 30 entries by default, up to
  100, with a cursor for older ones.
- **The MCP request limit is 1 MiB,** up from 256 KiB, so a 100-task plan
  with real descriptions fits in one call.
- **The web app changed too,** because it shares the task commands:
  - a task whose status changes goes to the bottom of its new column, where
    new tasks go;
  - archiving a task releases its claim;
  - restoring the archived original of a moved task is refused (409);
  - linking a thread or repository to an archived task is refused (409), but
    a thread can still be unlinked from one;
  - a thread you can't see can't be linked (404);
  - the task timeline shows "via Claude Code" the way messages do.
- **Two migrations weren't planned.** 0045 makes the database refuse a
  parent cycle from any writer. 0046 keeps control characters and line
  separators out of label descriptions.

## Decided for Phase 3 (October 1, 2026)

1. Admins as well as owners can remove anyone's message. This widens
   decision 3 of September 30.
2. Removing a message also removes the text of its earlier revisions, so a
   leaked secret doesn't live on in the history. Who wrote each revision,
   and when, is kept.
3. Agents edit and delete only their own messages, whatever their role.
4. Agents can leave groups, but they can't add people to them.
5. Phase 3's migrations are 0047_message_edits, 0048_message_reactions and
   0049_read_mark_rewind. 0046 is Phase 2's label-description migration and
   the live database has applied it, so no Phase 3 file may reuse it. Before
   adding a migration, check `ls apps/server/migrations` for the next free
   number rather than trusting a plan; `migration_versions_are_unique_and_contiguous`
   fails on a duplicate or a gap.
6. Reactions come from a fixed list of 12 emoji (👍 👎 ❤️ 😄 🎉 😕 🚀 👀 ✅ ❌
   🙏 🤔). A call states the result (react or take away) rather than
   toggling, and creates no notification, unread count or reply request.
   A removed message's reactions are hidden and frozen; the rows are kept,
   since they hold no text. Archived conversations keep theirs, frozen.

## Built in Phase 3 (October 2, 2026)

On `claude/mcp-phase-3`, conversations at full strength. Six new tools bring
the count from 55 to 61, which `tests/integration/m6_mcp_foundation.rs`
checks:

- `message_edit`: authors change their own messages, with
  `expectedRevision` and a `clientRequestId`. Every version is kept in
  `message_revisions` and shown by `message_history`, so editing never
  hides anything; the web editor says so too.
- `message_delete`: authors remove their own messages, and people with the
  owner or admin role remove anyone's. Agents never remove someone else's,
  whatever their role. Removal leaves a tombstone (who and when) and takes
  the text of every revision with it: search, briefing, `conversation_list`
  previews, `conversation_read` and `conversation_open`, pins, task links,
  reply-request context, replays of the original send, and threads named
  after the message, which get a neutral name. Open reply requests about it
  are withdrawn and can't be retried.
- `message_history`: every revision of a message, readable wherever the
  conversation is, archived ones included.
- `message_react`: one of the 12 fixed emoji, stated rather than toggled
  (`reacted: true|false`). Reactions notify nobody and change no unread
  count; a removed message's are hidden and frozen.
- `conversation_members`: people add and remove anyone in a group (20 at
  most); agents can only leave.
- `conversation_continue` (people only): gives the group's agents 6 more
  turns and asks them about the newest message that wasn't removed.

`mark_read` takes `unread: true` (from `fromMessageId`, default the newest
message) to come back to something later. Message IDs everywhere accept a
UUID or a `#message-<id>` link.

Migrations:

- 0047_message_edits: revision, edited and removed columns on messages, and
  `message_revisions`. A message changes only through the edit and remove
  commands, an edit must record both versions, a removal must redact every
  revision first, and no message or revision row is ever deleted.
- 0048_message_reactions: one row per message, person and emoji, with the
  12 emoji enforced by the database. The trigger re-checks membership,
  `conversation_visible_to`, and refuses removed messages.
- 0049_read_mark_rewind: only the mark-unread command can move a read mark
  backwards.

The server instructions now say to fix your own messages with
`message_edit`, to take a secret down with `message_delete` and ask for it
to be rotated, and to acknowledge with `message_react` (👀, ✅) rather than
a message; group reply requests suggest reacting and then `reply_pass`.
`tests/integration/phase3_takedown.rs` is the "done when": Bob pastes a
token in #general, Mara (an agent, even with the owner role) is refused when
she tries to remove it and asks for it to be taken down, Codex connected as
the owner removes it and reacts ✅ to her request, and the token is gone
from every tool, web view and table.

Differences from the plan:

- **Admins remove messages too, not just owners** (decision 1 of October
  1), and only people. So the "done when" is met by an app connected as a
  person, not by an agent account: the agent asks and is refused if it
  tries itself.
- **Removing a message removes its old versions' text** (decision 2).
  Decision 3 of September 30 said revisions would stay in the audit log;
  they don't. Audit rows hold IDs and counts only, and older thread audit
  rows that carried a name taken from the message lose it.
- **`conversation_members` lets agents leave, nothing more** (decision 4).
  The catalog said "add or remove people and agents". It takes a group or a
  thread in one; DMs and channels are refused.
- **Reactions are a fixed list, set rather than toggled** (decision 6), and
  create no notification or reply request.
- **Editing never hides anything.** Every version is visible to everyone who
  can see the conversation, including people added later. Both the tool and
  the web editor say to remove a secret rather than edit it out.
- **Removal reaches further than "search, briefings and
  `conversation_read`".** It also renames threads named after the message,
  replaces stored replays of the send and the thread creation with the
  tombstone (a replay of the original send is now refused as a different
  payload, and no hash of the text is kept), withdraws reply requests in
  every open state, and refuses new pins, task links, decisions and threads
  from the message. `thread_start` still returns a thread already started
  from it.
- **Edits notify newly @mentioned people and agents.** In channels and
  channel threads, a newly mentioned agent gets a reply request, as a send
  would; mentions taken out aren't un-notified.
- **The web app has all of it:** edit, remove, history, reactions, "Mark
  unread from here", and group members. Its routes sit under
  `/api/v1/conversations/{id}/messages/{message_id}`, so a message is
  always checked against its conversation. Pages now mark a conversation
  seen only when they show something new, so a mark moved back elsewhere
  stays.
- **Fixes along the way:** removing an agent from a group (web or MCP)
  withdraws its reply requests there and in the group's threads; a second
  Continue no longer cancels the first one's requests; retrying a request
  is refused when its message was removed or the agent can no longer see
  the conversation; and `migration_versions_are_unique_and_contiguous`
  fails on a reused or skipped migration number.

Still open: a run's prompt is copied into the run (`agent_runs`, sponsored
execution requests) when someone starts a run from a message, and removing
the message doesn't reach that copy.

## Built (Phase 4 and 5, October 3–4, 2026)

**Phase 4** (PRs 109 and 110; migrations 0050–0051):
- `pull_request_list`, `repository_list` and `agent_list`. PR visibility follows the run's thread (`pull_request_visible_to`).
- `run_start`, `run_get` and `run_list`. A person starts Codex on their own account, capped at 5 runs through apps.
- `approval_list`, with `approval_get` and `approval_respond` open to people.

Differences from the plan:
- Approvals that need a person stay in the web app, because Codex doesn't record the step they ask about.
- Publishing a run's branch and opening its PR are still web buttons.

**Phase 5** (migration 0052):
- `task_depend`: blocks and blocked-by, with cycles refused and the path shown.
- `milestone_list` and `milestone_write`: a target date, progress and an overdue flag; `addTasks` and `removeTasks`.
- Three prompts: `standup`, `plan_project` and `review_prs`.

Differences from the plan:
- `workflow_update` isn't built. Projects still check their statuses against the default workflow, so editing statuses needs that check redesigned first.
- Pages as MCP resources aren't built.

The tool count is 71.
