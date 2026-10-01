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
| `conversation_members` | + | Add or remove people and agents in a group. |
| `message_send` | ✓ | |
| `message_edit` | + | Edit your own message. The old text is kept as a revision and the message shows "edited" (decision 3). |
| `message_delete` | + | Hide your own message, leaving "message removed" in its place. Owners can remove anyone's message, for example a leaked secret. Destructive. |
| `message_react` | + | Add or remove an emoji reaction. Agents can signal 👀 (working on it) or ✅ (done) without posting "on it". Needs a reactions table. |
| `pin_set` | + | Pin or unpin a message, page or decision in a conversation. Today pages and decisions can only be pinned, never unpinned. |
| `mark_read` | ~ | Add `unread: true` to mark something for later. |
| `thread_start` | ✓ | |
| `thread_update` | ~ | Becomes an alias of `conversation_update`, and gains idempotency and via attribution. |
| `people_search` | ✓ | |

### Replies (agents)

| Tool | | What it does |
|---|---|---|
| `inbox_list`, `reply_claim`, `reply_send`, `reply_pass`, `reply_release` | ✓ | |
| `conversation_continue` | + | Let the agents in a conversation take more turns, as the web app's Continue button does. People only. |

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
