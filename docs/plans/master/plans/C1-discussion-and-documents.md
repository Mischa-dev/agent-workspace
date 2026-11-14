# C1: Discussion and documents

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner's draft, 2026-11-14, against `main` at fe95a74. Every `path:line` is relative to
`/home/user/agent-workspace` unless it says otherwise. Server paths are under
`apps/server/src/` unless they start with `apps/` or `docs/`; migrations are under
`apps/server/migrations/`. Nothing was built or run.

> **Amended by the architecture review (2026-11-14).** R54 amends the `subject_kind` CHECK: it uses the shared ThingKind domain. See `LEAD-RULINGS.md`.

## 1. Goal

When this is done there is one way to talk about anything. The comments on a task, the
comments on a page (on the page or on a sentence), on a sheet cell, on a whiteboard element
and on a record field are conversations attached to that thing, so @mentions, replies,
reactions, files, unread state, notifications and waking an agent work there exactly as in
a channel. An agent mentioned in a page comment is woken, answers in that thread, and makes
the edit. Pages, sheets and whiteboards are each close to their reference product (number
formats, charts, validation, filters and cell comments in sheets; live cursors, element
comments, stickies into tasks in boards; templates, sharing, PDF and "make a page from
this" in pages). Whether anyone's edit lands directly or as a proposal is a role setting,
the proposal is one record for every kind of document with one review experience, and
every document shows who just changed what, with one tap to undo it.

## 2. Where the code is today

### 2.1 The message pipeline (keep it; everything else joins it)

- One `conversations` table, `kind IN ('channel','thread','dm','group')`
  (`0037_direct_conversations.sql:29-30`). Threads have a parent that may be a channel, DM
  or group (`0042_agent_interface.sql:102-150`, trigger `validate_conversation_parent`).
  Timelines are `conversation_entries` over `messages`; unread state is
  `conversation_read_states`.
- The one visibility rule is `conversation_visible_to(target, viewer)`
  (`0037_direct_conversations.sql:158-180`), unchanged since. It answers only "is this
  project-wide, or is the viewer on the member list of it or its parent". It does not check
  project membership; every caller pairs it with a membership join (B report finding 6).
- `append_message_command_with` (`conversations.rs:1695-2067`) is the send pipeline for
  HTTP, MCP, scheduled sends, push replies and forwards. In one transaction it: locks the
  conversation and re-checks access (`lock_message_conversation`, `:2219-2347`, which uses
  `conversation_visible_to` at `:2301` and `authorize` at `:2341`); reserves idempotency
  (`:1755`); resolves mentions and task references (`:1790`); inserts the message
  (`:1817`), its projections (`:1839`), files (`:1847`) and link previews (`:1855`);
  allocates the sequence (`:1862`); renames auto-named threads (`:1878`); writes the outbox
  `MessageCreated` (`:1888`); completes or renews a reply claim (`:1984-2005`); sends
  `message.mentioned` events to mentioned agents' connections
  (`enqueue_external_message_events`, `:2006`, body `:2351-2423`); and runs the reply rules
  (`replies::ask_for_replies`, `:2014`; rules at `replies.rs:97-160`).
- Edits, removals and reactions share `message_edits::{lock_message_for_change,
  apply_edit, apply_removal}` and `reactions::set_reaction` between HTTP and MCP. Message
  revisions are `message_revisions` (`0047_message_edits.sql:30-60`), guarded by triggers
  that need `aw.message_command` set to the message id (`0047_message_edits.sql:71,109`).
- Web: one composer (`apps/web/src/conversations/StructuredComposer.tsx`) and one
  `Timeline.tsx` are already used for channels, threads, DMs **and task comments**
  (`TaskActivity.tsx`, `CommentEditing.tsx`). That makes the UI half of "one message
  system" cheap.

### 2.2 Thread creation, six times

| # | Where | Parent check | Name | `auto_named` | Audit | Reuses a thread already started from the message |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | `conversations.rs:998-1153` `create_thread` (HTTP) | `lock_conversation_access` `:1020` | given, else `thread_name_from_text` or "New thread" `:1072-1078` | yes `:1071` | `:1121` | no |
| 2 | `mcp/talk.rs:2117-2268` (`thread_start`) | locked parent, kind check `:2117` | given or `suggested_name`, made unique `:2184-2204` | no | `:2240` | yes `:2142` |
| 3 | `mcp/replies.rs:970-1078` `open_answer_thread` | `lock_conversation_for` `:978` | `suggested_thread_name`, unique `:1021-1023` | no | `:1059` | yes `:992` |
| 4 | `work_items.rs:1532-1611` (work with a worker) | bare `EXISTS` with `conversation_visible_to`, no lock `:1532-1546` | `worker_thread_name` `:1552` | no | task activity only | no |
| 5 | `launch.rs:1751-1840` `task_thread` | `project_access` then an unlocked `SELECT` `:1769-1802` | `worker_thread_name` `:1815` | no | none | via `reusable_thread` `:1707` |
| 6 | `launch.rs:2920-2995` `thread_under` | `FOR UPDATE` without a visibility check `:2926`, then `project_access` `:2948` | `suggested_thread_name`, its own copy of `unique_thread_name` `:2953-2957, :2999` | no | none | yes `:2931` |

(`seed.rs:247` is a seventh insert for development data only.) Each copy also writes the
read state and `ConversationCreated` by hand. Consequences: only the web path sets
`auto_named`, so a thread an agent starts from a message keeps the message's words in its
name forever, and message removal has to mirror the web dialog's JavaScript to scrub them
(`conversations.rs:4882-4898` `web_suggested_thread_name`, used at `message_edits.rs:869`).

### 2.3 Task comments: a second message system

Tables: `work_item_comments` (`0017*.sql:32-50`; edits and removal columns
`20261028070000_keyboard_drag_polish.sql:66-90`; `via_oauth_grant_id`
`0042_agent_interface.sql:43-45`; `reply_to_comment_id`
`20261113080000_task_comment_replies.sql:12-18`), `work_item_comment_mentions`
(`0017:66-77`), `work_item_comment_references` (`0017:92-104`),
`work_item_comment_revisions` (`20261028070000:92-125`). They are interleaved with task
activity through `work_item_timeline_entries` (`0017:243-263`, kind `'comment'`), whose
sequence comes from `work_items.next_timeline_sequence` (`work_item_commands.rs:3035`).
Other tables point at comment ids: `notifications.comment_id`
(`20261016120000_notifications.sql:34`), `files.comment_id`
(`20261017150000_files_and_link_previews.sql:23`), and the `thing_references` trigger
(`20261113060000_thing_references.sql:170-190`, source kind `task_comment`).

Code: HTTP create `work_items.rs:2859` (insert `:2976`, mentions `:2994`, references
`:3006`, `finalize_comment` `:3016`, files `:3025`); MCP create `mcp/writes.rs:1243-1394`
(insert `:1345`); shared tail `work_item_commands.rs:2906-2936` `finalize_comment`
(timeline, `work_item.updated`, an external `work_item.changed` event for the assignee,
watcher events, audit); edit and delete `work_items/comments.rs` (808 lines, mirrors
`message_edits`); readers in `notifications.rs:880-925`, `modules/things.rs:415,432`,
`mcp/tasks.rs:2159`, `ai/summaries.rs:205`, `work_item_move.rs:2159-2175,2554` (a move copies
comments), web `apps/web/src/work-items/TaskActivity.tsx`, `CommentEditing.tsx`.

What a task comment lacks compared with a message:

| Feature | Message | Task comment |
| --- | --- | --- |
| Mentions from an agent's text | `content_from_text` (`mcp/common.rs:515`) | none: MCP builds `MessageContent::from_plain_text` (`mcp/writes.rs:1255`) and inserts no mentions or references |
| Waking a mentioned agent | `message.mentioned` event + reply request | none. Comment mentions feed people's notifications only (`notifications.rs:880-925`; "Only people get notifications", `notifications.rs:12`). An agent hears only `work_item.changed` if it is the assignee or a watcher (`external_events.rs:87-140`, `work_item_commands.rs:2656-2688`) |
| Reactions | `message_reactions` | none |
| Unread state | `conversation_read_states` | none |
| Link previews, forwarding, pins, "save for later" | yes | none |
| Search | messages are indexed (`0042_agent_interface.sql:173-177`) | not searchable (`search.rs:30-38` has no comment kind) |
| Moderation | `channels.moderate` by role | a person-only rule (`work_items/comments.rs:776-808`) |
| Concurrency | locks only the conversation | takes the task row lock (`work_items.rs:2922`, `mcp/writes.rs:1300`) |

### 2.4 Page comments and suggestions

- `project_page_comments` (`20261112004000_page_comments_and_suggestions.sql:46-75`): a
  third comment system. Plain text `body` (no mention nodes), threads by `thread_id`, an
  optional anchor `(block_id, quote)` on the first comment, `resolved_at` per thread,
  `edited_at` without revisions, `deleted_at`. No reactions, files, read state or search.
- Code: `page_docs/comments.rs:318-463` `create` (locks the page row first, through
  `discussion.rs:127-148`, and needs `PageEdit` to comment); `page_docs/discussion.rs`
  (HTTP); `mcp/page_discussion.rs` (`page_comments`, `page_comment`, `page_comment_update`).
  Web `apps/web/src/memory/discussion/PageDiscussion.tsx` and
  `memory/live/highlights.ts` (marks anchored words and suggested blocks).
- Notifications go to the page's creator and the thread's earlier authors
  (`comments.rs:426-462`), through `suggestions::notify`, which drops every recipient that
  isn't a person (`page_docs/suggestions.rs:457`). So **an agent is never woken by a page
  comment**, mentioned or not; R3's "unclear" is answered: it cannot happen today. The
  notification also stores 140 characters of the comment in `detail`
  (`comments.rs:451`), against the rule that notifications hold ids, not text
  (`notifications.rs:12-16`): a deleted comment's words survive in the inbox row.
- `project_page_suggestions` (`20261112004000:10-38`): block edits by lasting block id
  with the hash each was read at, `base_version`, status, decider, `applied_version`.
  `page_docs/suggestions.rs`: `resolve` (`:119`), `create` (`:386`), `decide` (`:516`, applies
  through the same save path). Good shape: it is the model for every kind's proposal.
- The account-kind rule: `must_suggest` (`suggestions.rs:42-44`) returns true for any agent
  writing to a page it didn't create; called at `mcp/memory_tools.rs:550` and `:1272`
  (`page_edit` also accepts `suggest: true`, so anyone can already opt in).

### 2.5 Sheets and whiteboards: no comments at all

Neither has a comment table or tool. `docs/plans/whiteboards.md` lists "Comments pinned to
elements" as not built. Collection records have none either (C report finding 4).

### 2.6 Files by kind

`files` (`20261017150000_files_and_link_previews.sql:16-59`) belongs to exactly one of a
conversation or a task (`CHECK ((conversation_id IS NULL) <> (work_item_id IS NULL))`),
with `message_id` or `comment_id` once sent. `files.rs:277-290` `Destination` has only
those two. Bytes go through `files/store.rs:20-23` `enum FileStore { Local }`, a real seam.
Pages have no image block (F report F9). Sheets have none (import and export only).
Whiteboard images bypass the store: `whiteboard_files.data BYTEA`
(`20261017070000_whiteboards.sql:72-81`; `whiteboards/store.rs:684`), up to 64 MiB a board.
Records have no file field (T1 owns that field type).

### 2.7 History by kind

| Kind | What is kept | Restore | Undo one change |
| --- | --- | --- | --- |
| Page | every version, full Markdown (`project_page_revisions`, `0042_agent_interface.sql:219-229`), plus agent session, harness and run (`20261112031500_page_revision_sessions.sql`) | yes (`memory.rs:680` `restore_version`; web History) | no |
| Sheet | `sheet_revisions` with counts only: `{"cells":n,"values":n,"tabs":bool,"operations":[...]}` (`sheets.rs:1859-1873`) | **no**: no earlier cell inputs are kept anywhere | no |
| Whiteboard | whole-scene snapshots at most every 10 minutes, 100 kept, and before mass deletes and restores (`whiteboards/store.rs:29,266`; `20261017070000:97-110`) | yes (`whiteboards/http.rs:61`, `store.rs:356`) | no |
| Task | `work_item_activity` before/after by id (C report finding 4) | no | no |
| Record | `collection_record_history` names frozen at write | no | no |
| Comment | `work_item_comment_revisions`; page comments keep none | n/a | n/a |

### 2.8 How a mention wakes an agent, by place

| Place | Mentioned person | Mentioned agent |
| --- | --- | --- |
| Channel or thread message by a person | notification | `message.mentioned` event and a reply request (`replies.rs:105-113`) |
| Message by an agent in a channel or thread | notification | event only, no reply request (`replies.rs:105`; A2 owns this rule) |
| DM or group | notification | reply request under the turn allowance (`replies.rs:125-157`) |
| Task comment from the web | notification | nothing (unless assignee or watcher, then an untyped `work_item.changed`) |
| Task comment from an agent | nothing (no mentions are made) | nothing |
| Page comment | the page creator and thread authors, if people | nothing |
| Sheet, board, record | no comments exist | |

### 2.9 Account-kind rules in this area (each becomes a role setting or a delivery mechanism)

| Rule | Where | Replaced by |
| --- | --- | --- |
| An agent's edit to a page it didn't create must be a suggestion | `page_docs/suggestions.rs:42-48`, callers `mcp/memory_tools.rs:550,1272` | K1's "suggest without edit" on pages, with an "edit own" scope (C1.8) |
| Agents may accept or reject suggestions only on pages they made | `page_docs/suggestions.rs:558-565` | K1's edit permission on the page (C1.8) |
| Suggestion and comment notices skip non-people | `page_docs/suggestions.rs:457-459` | mechanical difference kept (people get the inbox), but agents get the same occurrence as a delivered event (C1.5, C1.7; A2 delivers) |
| Only people may delete others' task comments | `work_items/comments.rs:782-784` | the conversation's moderate permission, by role (C1.4) |
| Only a person's message asks agents in channels and threads | `replies.rs:105`, `:171` | A2 (loop limits). C1 only makes comment conversations follow the thread rules |

### 2.10 Keep, and what is in the way

Keep: the send pipeline; `lock_conversation_scope`'s lock order and re-check; one write
path per document kind (`page_docs::store::save`, `sheets::apply`,
`whiteboards::store::write_elements`); the suggestion's "block id plus hash" staleness rule;
per-cell revisions; per-element version merge; `thing_references` by trigger; the shared
composer and timeline. F report F11 is right: do not merge Yjs, cells and Excalidraw into
one collaboration backend.

In the way: comment tables beside messages; a page comment taking the page's row lock; the
project row lock on every sheet write (`sheets.rs:15-18`, `work_item_commands.rs:363-374`,
`sheets/http.rs:181`, `mcp/sheet_tools.rs:112`) held across a whole-workbook recalculation
(`sheets.rs:1486`); sheet history that can't restore; board images in the database; no
events or search for sheets and boards (`events.rs:33-51`, `search.rs:30-38`); sheets not
a thing (`modules/things.rs:51-61`).

## 3. The target design

### 3.1 Words

- **Comments conversation**: a conversation of the new kind `comments`, attached to one
  thing (its *subject*). At most one per thing. It is created by the first comment.
- **Anchored thread**: a thread (the existing kind) whose parent is a comments
  conversation and which carries an *anchor*: a text range in a page, a cell or range in a
  sheet, elements in a board, a field of a record or task. Page-level threads (Notion's
  "comment on the page") are threads with no anchor. Resolving is the thread's status.
- **Proposal**: an edit to a document that waits for someone with edit permission to
  accept it. One table for every kind.
- **Change set**: one write to a document by one account, with what it touched and enough
  to undo it. The unit of "recent edits" outlines and of one-tap undo.

Why `comments` is a conversation kind and not a column on threads: a task's comments are a
flat stream with quoted replies (today's task comment UI), which is what a conversation
is; anchored discussions need their own resolve state, which is what a thread under it is.
Both already exist, so the only new things are a parent kind and a subject.

### 3.2 Data model for discussion

```sql
-- C1.3 (expand only; no data moves)
ALTER TABLE conversations
    ADD COLUMN subject_kind TEXT CHECK (subject_kind ~ '^[a-z][a-z_]{1,31}$'),
    ADD COLUMN subject_id   UUID,
    -- Only on threads under a comments conversation. Shape is per kind (3.4).
    ADD COLUMN anchor JSONB CHECK (anchor IS NULL OR (jsonb_typeof(anchor) = 'object'
                                   AND octet_length(anchor::text) <= 4096)),
    DROP CONSTRAINT conversations_kind_check,
    ADD CONSTRAINT conversations_kind_check
        CHECK (kind IN ('channel', 'thread', 'dm', 'group', 'comments')),
    DROP CONSTRAINT conversations_visibility_check,
    ADD CONSTRAINT conversations_visibility_check
        CHECK (visibility IN ('project', 'members', 'subject'));
-- kind_shape_check gains:
--   (kind = 'comments' AND parent_channel_id IS NULL AND origin_message_id IS NULL
--    AND repository_id IS NULL AND dm_key IS NULL AND visibility = 'subject'
--    AND subject_kind IS NOT NULL AND subject_id IS NOT NULL AND anchor IS NULL)
-- and every other kind requires subject_kind IS NULL AND subject_id IS NULL,
-- and anchor IS NULL unless kind = 'thread'.

CREATE UNIQUE INDEX conversations_one_comments_per_subject
    ON conversations (subject_kind, subject_id) WHERE kind = 'comments';
CREATE INDEX conversations_anchored_threads
    ON conversations (parent_channel_id, created_at) WHERE anchor IS NOT NULL;
```

`validate_conversation_parent()` is replaced to allow `parent.kind IN ('channel','dm',
'group','comments')` and to require `NEW.anchor IS NULL` unless the parent is `comments`.
`project_id` is the subject's project; a whiteboard with no project uses its workspace's
hidden workspace space (`workspace_space.rs`, `20261113203000_workspace_space.sql`), which
every workspace member belongs to. K4 later renames `project_id` to `space_id` here as
everywhere.

**Visibility.** `conversation_visible_to` stays the single gate and learns one branch. Under
R3 the function body is K1's and is composed of named helpers, each owned by one workstream;
C1 owns only `subject_visible_to` and never replaces another helper. If the split has not
landed when C1.3 merges, C1.3 does it first, without changing behaviour: today's
membership rule moves into `conversation_member_visible(target, viewer)` (K4's helper), and
`conversation_visible_to` becomes the composition below (K1's body). The SQL below shows the
composed result C1.3 leaves; after the split, C1's migrations touch only
`subject_visible_to`:

```sql
-- Composition (K1's body after the R3 split), kept exactly as K1 section 3.10 shows it:
-- C1's body, with K4's arms moved into conversation_member_visible. The 'project' and 'members' arms are
-- conversation_member_visible's (K4's); C1 adds only the 'subject' arm's helper.
CREATE OR REPLACE FUNCTION conversation_visible_to(target UUID, viewer UUID)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT EXISTS (
        SELECT 1
        FROM conversations conversation
        LEFT JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = target
          AND CASE COALESCE(parent.visibility, conversation.visibility)
                WHEN 'project' THEN true
                WHEN 'members' THEN EXISTS (
                    SELECT 1 FROM conversation_members member
                    WHERE member.conversation_id =
                          COALESCE(conversation.parent_channel_id, conversation.id)
                      AND member.actor_id = viewer)
                WHEN 'subject' THEN subject_visible_to(
                    COALESCE(parent.subject_kind, conversation.subject_kind),
                    COALESCE(parent.subject_id, conversation.subject_id),
                    viewer)
                ELSE false
              END
    )
$$;

-- The seam K1 replaces with its generated "may see" filter. Until then it
-- dispatches to today's rule for each kind, and it checks membership itself
-- (unlike the 'project' branch), so a comment can never be wider than its thing.
CREATE FUNCTION subject_visible_to(target_kind TEXT, target_id UUID, viewer UUID)
RETURNS BOOLEAN LANGUAGE sql STABLE AS $$
    SELECT CASE target_kind
        WHEN 'whiteboard' THEN whiteboard_visible_to(target_id, viewer)
        WHEN 'task'   THEN EXISTS (SELECT 1 FROM work_items i WHERE i.id = target_id
                                   AND actor_can_access_project(viewer, NULL, i.project_id))
        WHEN 'page'   THEN EXISTS (SELECT 1 FROM project_pages p WHERE p.id = target_id
                                   AND actor_can_access_project(viewer, NULL, p.project_id))
        WHEN 'sheet'  THEN EXISTS (SELECT 1 FROM sheets s WHERE s.id = target_id
                                   AND actor_can_access_project(viewer, NULL, s.project_id))
        WHEN 'record' THEN EXISTS (SELECT 1 FROM collection_records r WHERE r.id = target_id
                                   AND actor_can_access_project(viewer, NULL, r.project_id))
        ELSE false
    END
$$;
```

Because realtime (`realtime.rs:506-560`), notifications (`notifications.rs:1323`), search
(`search.rs:495-509`), files (`files.rs:216`) and link previews already call
`conversation_visible_to`, they all follow a thing's access with no other change. When K1
adds per-page sharing (C1.27), only `subject_visible_to`'s `page` arm changes. The
function's `CASE` keeps the planner from evaluating the subject arm for ordinary
conversations; C1.3 checks the timeline and sidebar query plans before and after.

### 3.3 The `discussion` module

New `apps/server/src/discussion/` (`mod.rs`, `subjects.rs`, `anchors.rs`, `ops.rs`) and,
from C1.2, `apps/server/src/conversations/threads.rs`. Public functions:

```rust
// conversations/threads.rs: the one way a thread is made (C1.2).
pub(crate) enum ThreadName {
    /// A person or agent named it.
    Given(String),
    /// Named from its origin message or first message until someone names it.
    Derived,
    /// A fixed name a caller composes (task launches): unique under the parent.
    Composed(String),
}
pub(crate) struct ThreadSpec {
    pub origin_message_id: Option<Uuid>,
    pub name: ThreadName,
    pub anchor: Option<Anchor>,          // only under a comments conversation
    pub reuse_for_origin: bool,          // return the open thread already started from the origin
    pub link_task: Option<Uuid>,         // writes the task link (K3 link store once it exists)
    pub audit: serde_json::Value,        // extra audit metadata (reply request id, launch id)
}
pub(crate) struct CreatedThread { pub id: Uuid, pub name: String, pub created: bool, pub parent_kind: String }

pub(crate) async fn create(
    tx: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,                // id, kind, via grant, requested_by
    parent_id: Uuid,
    spec: ThreadSpec,
) -> Result<CreatedThread, AppError>;
```

`create` locks the parent through `lock_conversation_for(.., Action::ThreadCreate, ..)`
(visibility re-checked after the wait), refuses a parent that is a thread or archived,
reuses an open thread from the same origin when asked, names by policy (`Derived` sets
`auto_named = true` and uses `thread_name_from_text`, so removal and edits rename it through
`refresh_auto_thread_names`), inserts, writes the creator's read state,
`ConversationCreated`, and always an audit row. The six callers in 2.2 become calls to it.

```rust
// discussion/mod.rs
pub struct ThingRef { pub kind: &'static str, pub id: Uuid }

/// What a kind of thing tells discussion. Registered per kind; K3's thing
/// registry holds it once that exists (until then a static table here).
#[async_trait]
pub trait DiscussionSubject: Send + Sync {
    fn kind(&self) -> &'static str;
    /// The project (later space) the comments live in, read with the thing locked FOR SHARE.
    async fn home(&self, tx: &mut PgConnection, id: Uuid) -> Result<Option<SubjectHome>, AppError>;
    /// The permission to comment (K1: "<kind>.comment"); checked by `post`.
    fn comment_action(&self) -> Action;
    /// Validate an anchor against the thing now, and normalise it.
    async fn check_anchor(&self, tx: &mut PgConnection, id: Uuid, anchor: &Anchor) -> Result<Anchor, Refusal>;
    /// Who hears about new comments besides mentions: owner, assignee, watchers, followers.
    async fn followers(&self, tx: &mut PgConnection, id: Uuid) -> Result<Vec<Uuid>, AppError>;
    /// Where a comment opens, e.g. /projects/{p}/work-items/{id}#comment-{m}.
    fn href(&self, home: &SubjectHome, id: Uuid, message: Uuid, thread: Option<Uuid>) -> String;
    /// Archived or trashed things refuse new comments.
    async fn accepts_comments(&self, tx: &mut PgConnection, id: Uuid) -> Result<bool, AppError>;
}

/// The thing's comments conversation, made if it doesn't exist (ON CONFLICT on
/// conversations_one_comments_per_subject, so two first comments race safely).
pub async fn open_for(tx, actor: &CommandActor, thing: ThingRef) -> Result<Uuid, AppError>;

/// Posts a comment: opens the conversation (and the anchored thread, when an
/// anchor and no thread are given), then calls the send pipeline. Every
/// mention, reply rule, file, preview, notification and wake-up is the
/// pipeline's.
pub async fn post(state: &AppState, actor: MessageCommandActor, thing: ThingRef,
                  place: CommentPlace, key: IdempotencyKey, content: serde_json::Value,
                  attachments: &[Uuid], reply_to: Option<Uuid>) -> Result<PostedComment, AppError>;

pub enum CommentPlace { Stream, Thread(Uuid), NewThread { anchor: Option<Anchor> } }
```

`post` opens and commits the conversation and thread in a short first transaction and then
calls `append_message_command_with` with `expected_kind` `comments` or `thread`. The pipeline
is unchanged except for three small hooks: `effective_kind()` returns `"thread"` for
`comments` (so the reply rules treat a comment like a thread message: mentioned agents get a
reply request, A2 decides agent-to-agent); `enqueue_external_message_events` takes the
href from the subject (`DiscussionSubject::href`) instead of the `/projects/{p}/threads/`
pattern at `conversations.rs:2397-2418`; and the outbox `MessageCreated` carries
`subject_kind/subject_id` so the thing's open page refreshes. Followers are told through a
new notification kind `thing_comment` (C2 owns the kind list and the inbox; C1 supplies the
recipients and ids, never text).

### 3.4 Anchors

```jsonc
// page: a range of words in a top-level block (today's block_id + quote)
{"kind": "text", "blockId": "b7Qx…", "quote": "the launch date"}
// sheet: a cell or a range on a tab, by tab id
{"kind": "cell",  "tabId": "…uuid…", "row": 12, "column": 3}
{"kind": "range", "tabId": "…uuid…", "from": [2, 1], "to": [9, 4]}
// whiteboard: elements, with a fallback point if they are deleted
{"kind": "elements", "elementIds": ["sticky-1", "arrow-4"], "at": {"x": 120.5, "y": -40}}
// record or task: a field
{"kind": "field", "fieldId": "…uuid…"}          // or "field": "description" for a task's built-in
```

Anchors follow the thing. Sheets: `sheets::apply` already computes row and column moves per
tab (`WorkingTab.moves`, `sheets.rs` around `:940`); C1.19 rewrites cell and range anchors in
the same transaction, and a deleted cell's thread keeps its last place with
`"orphaned": true`. Pages: block ids are lasting; a missing quote shows the thread as
"words changed". Boards: deleted elements fall back to `at`.

### 3.5 Operations (K2 defines the registry; these are C1's entries)

| Operation | Input | Permission (K1 names) | Effect |
| --- | --- | --- | --- |
| `comment_post` | `thing {kind,id}`, `text` or `content`, `threadId?`, `anchor?`, `replyToMessageId?`, `attachmentIds?`, `clientRequestId` | `<kind>.comment` on the thing | `discussion::post`; returns message, conversation and thread ids and the href |
| `comment_list` | `thing`, `include: stream|threads|all`, `status?`, `anchor?`, cursor | see the thing | the stream page and anchored threads with status and anchor |
| `comment_thread_update` | `threadId`, `status` (Open, Resolved, ...), `outcome?` | `<kind>.comment` | the existing thread status update (`thread_update`) |
| existing `message_edit`, `message_delete`, `message_react`, `mark_read`, `thread_start` | unchanged | unchanged | work on comments because comments are messages |
| `task_comment`, `task_comment_edit`, `task_comment_delete`, `page_comments`, `page_comment`, `page_comment_update` | unchanged arguments and results | as today | kept as aliases that map onto the rows above (K2's alias mechanism; until K2 lands, hand-written wrappers) |

Every new route and tool gets an isolation case in `tests/integration/` (a member of
another project, a person outside a private channel, an app limited by its agent limit).

### 3.6 Proposals

```sql
-- C1.7
CREATE TABLE proposals (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),       -- space_id after K4
    thing_kind TEXT NOT NULL CHECK (thing_kind ~ '^[a-z][a-z_]{1,31}$'),
    thing_id UUID NOT NULL,
    operation TEXT NOT NULL CHECK (char_length(operation) BETWEEN 1 AND 64),  -- K2 op name
    author_actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) DEFAULT current_via_grant(),
    requested_by_actor_id UUID REFERENCES actors(id),
    run_id UUID,                                           -- A1's run, when a run made it
    base_version BIGINT NOT NULL CHECK (base_version > 0),
    change JSONB NOT NULL CHECK (jsonb_typeof(change) = 'object'
                                 AND octet_length(change::text) <= 1048576),
    preview JSONB NOT NULL DEFAULT '{}'::jsonb CHECK (octet_length(preview::text) <= 1048576),
    note TEXT CHECK (note IS NULL OR char_length(note) <= 500),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN
        ('pending', 'accepted', 'partly_accepted', 'rejected', 'withdrawn', 'stale')),
    decided_by_actor_id UUID REFERENCES actors(id),
    decided_at TIMESTAMPTZ,
    decision_note TEXT CHECK (decision_note IS NULL OR char_length(decision_note) <= 1000),
    applied_version BIGINT,
    applied_change_set_id UUID,                            -- document_change_sets(id)
    request_id UUID,                                       -- A1's requests(id)
    thread_id UUID,                                        -- review discussion, a thread under the thing's comments
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((status = 'pending') = (decided_at IS NULL)),
    CHECK (applied_version IS NULL OR status IN ('accepted', 'partly_accepted'))
);
CREATE INDEX proposals_thing_pending ON proposals (thing_kind, thing_id) WHERE status = 'pending';
CREATE INDEX proposals_author ON proposals (author_actor_id, created_at DESC);
CREATE INDEX proposals_workspace_pending ON proposals (workspace_id, created_at DESC) WHERE status = 'pending';
```

The `change` per kind (each says what was read, so accepting can never undo a change the
author hadn't seen):

```jsonc
// page (today's StoredEdit list, unchanged)
{"edits": [{"op": "replace", "blockId": "b7Qx", "hash": "9f…", "markdown": "…", "before": "…"}]}
// sheet: cell changes with the revision each cell was read at (the existing ifRevision)
{"changes": [{"cell": {"tab": "Budget", "row": 4, "column": 2, "input": "=B3*1.2", "ifRevision": 311}}],
 "before": {"Budget!B4": "1200"}}
// whiteboard: the whiteboard_edit operations and the versions of every element they touch
{"operations": [{"op": "add", "id": "sticky-9", "type": "sticky", "label": "Ship Friday"}],
 "touched": {"frame-2": 14}, "expectedSceneSeq": 882}
// record (and task, through T1's record write)
{"fields": {"<fieldId>": {"before": "Open", "after": "Triaged"}}, "expectedVersion": 7}
```

```rust
// documents/proposals.rs
#[async_trait]
pub trait Proposable: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Check the operation's input against the thing now and turn it into the stored
    /// change (positions become ids, each item carries what it was read at) and a preview.
    async fn prepare(&self, tx: &mut PgConnection, thing: Uuid, op: &str, input: &Json)
        -> Result<Result<Prepared, Refusal>, AppError>;
    /// Apply all, or the parts named, as the author, with the accepter named in history.
    /// Refuses (Stale) when any part it applies changed since it was read.
    async fn apply(&self, tx: &mut PgConnection, thing: Uuid, change: &Json,
                   parts: Option<&[usize]>, author: Writer, accepted_by: Uuid)
        -> Result<Result<Applied, Stale>, AppError>;
}
pub struct Prepared { pub base_version: i64, pub change: Json, pub preview: Json }
pub struct Applied  { pub version: i64, pub change_set_id: Uuid }
```

**How an operation becomes a proposal.** K2's operation definition carries
`proposable: Option<&'static dyn Proposable>` (K2's `PROPOSABLE` flag; K2's `ProposalDraft`
is the input `prepare` takes). Before running a write, the operation layer asks K1's
`access::write_mode(conn, caller, kind, target)` (R2): `Ok(Grant{mode: Direct})`,
`Ok(Grant{mode: Propose})` or `Err(Denied)`. On `Propose`, or when the caller passes
`propose: true`, the layer calls `proposals::create`, which runs `prepare`, inserts the row, raises an A1 request
(`kind = 'proposal'`, subject the proposal, assignees: accounts with edit on the thing, the
thing's owner first), emits `proposal.created`, and returns the standard result with
`{"outcome": "proposed", "proposalId": "…", "href": "…"}`. So the same `sheet_update` call
becomes a proposal for an account whose role says "suggest" on sheets, with no special tool.

**Deciding.** One operation, `proposal_decide {proposalId, decision: accept|reject|withdraw,
parts?, note?}`. Accept and reject need edit on the thing (K1). Withdraw is the author's.
Accept runs `apply` inside a K2 dispatch with `Edge::Proposal{proposal_id, accepted_by}` (so
the write is checked, metered and logged like any other), in one transaction with the thing
locked, records a change set, sets `applied_version`, resolves the A1 request (idempotently:
A1's `request_answer` on a proposal request calls `proposal_decide`, and a second decide is a
no-op) and emits `proposal.decided`. This is the accept path for every kind; K2 does not
re-dispatch the stored input separately (R14: C1 owns proposals and review). A stale proposal
is marked `stale` and stays readable; the reviewer can ask the author to redo it in the
proposal's thread. `proposal_list {thing? | mine | waitingOnMe}` reads them.

**Review experience.** One panel for every kind (`apps/web/src/documents/proposals/`,
lazy): list, author, note, age, accept all, accept parts, reject with a note, and "discuss"
(opens the proposal's thread). Each kind supplies an inline preview: pages show the
existing block marks and a before/after diff (`memory/live/highlights.ts`); sheets show
proposed cells with a dashed outline and the old value on hover; boards draw proposed
elements as ghosts over the canvas; records show a field diff in the record view. C2's inbox
shows the A1 request as a card with Accept and Reject.

### 3.7 Change sets: marking what changed, and one-tap undo

```sql
-- C1.9
CREATE TABLE document_change_sets (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    project_id UUID NOT NULL REFERENCES projects(id),
    thing_kind TEXT NOT NULL,
    thing_id UUID NOT NULL,
    actor_id UUID NOT NULL REFERENCES actors(id),
    via_oauth_grant_id UUID REFERENCES mcp_oauth_grants(id) DEFAULT current_via_grant(),
    requested_by_actor_id UUID REFERENCES actors(id),
    run_id UUID,
    version_from BIGINT NOT NULL,
    version_to BIGINT NOT NULL,
    -- What it touched, by lasting id: {"blocks": [...]} | {"cells": [[tab,row,col],...]} |
    -- {"elements": [...]} | {"fields": [...]}
    touched JSONB NOT NULL CHECK (octet_length(touched::text) <= 262144),
    -- What it replaced, to undo it; NULL when too large (undo then offers restore).
    inverse JSONB CHECK (inverse IS NULL OR octet_length(inverse::text) <= 1048576),
    proposal_id UUID REFERENCES proposals(id),
    undoes_change_set_id UUID REFERENCES document_change_sets(id),
    undone_by_change_set_id UUID REFERENCES document_change_sets(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX document_change_sets_thing ON document_change_sets (thing_kind, thing_id, created_at DESC);
```

Each kind's one write path writes one row per write: `page_docs::store::save` (touched
blocks are the ones whose hash changed; inverse is their previous Markdown and position;
the browser's Yjs updates are grouped per editing session and minute, as page revisions
already are), `sheets::apply` (touched cells; inverse is the previous inputs of changed
cells, which also fixes "sheet history can't restore"), `whiteboards::store::write_elements`
and `whiteboard_edit` (touched elements; inverse is their previous JSON). Records and tasks
join through T1's record write.

Operation `change_undo {changeSetId}`: needs edit on the thing; applies the inverse for the
items not changed since (by block hash, cell revision, element version), skips and lists the
rest, writes its own change set (so undo is undoable), and marks the original
`undone_by_change_set_id`. The outbox event for the write carries the change set id, so an
open editor outlines the touched items the moment they land, labelled with the author
("Muse · 2 min · Undo"). A "Recent edits" toggle shows outlines for the last 24 hours or
since the viewer last opened the document, filterable by person or agent. Retention: change
sets older than 90 days lose `inverse` (a daily job), and rows go after a year; restore of
older versions uses snapshots.

### 3.8 Pages depth

- **Ask in a comment.** Falls out of 3.3: a page comment mentioning an agent gives it a
  reply request in that thread; it claims, reads the page and the thread's anchor
  (`comment_list` returns it), edits with `page_edit` passing `threadId`, and replies. The
  change set records `thread_id` in `touched.context`, and the thread shows a system line
  "Edited the page (v42) · View · Undo". If the agent's role says suggest, the same
  `page_edit` call makes a proposal linked to the thread instead.
- **Sharing levels and public links.** A share dialog per page: people and groups with
  view, comment, suggest or edit; "anyone in the workspace"; and a public read-only link.
  K1 owns the grants and the access function (per-thing grants, link tokens as credentials);
  C1 owns the dialog, the public page route `GET /p/{token}` (server-rendered HTML from the
  page's blocks, no app shell, no comments, `noindex` unless the owner opts in) and making
  `subject_visible_to('page', …)` call K1's per-thing rule.
- **Word and PDF export.** Word exists (`page_docs/docx.rs`, `transfer.rs:3-6`). PDF: a
  print stylesheet for "Print / Save as PDF" in the browser (immediate), and a server export
  `GET /api/v1/pages/{id}/export?format=pdf` that converts blocks to a typesetting library
  written in Rust and run in-process (Typst as a library is the leading candidate:
  Apache-2.0, no browser, no outside service). The alternative, a headless browser, needs a
  large runtime in the image and a sandbox; rejected for self-hosting weight. Embeds export
  as their title and link; sheet ranges as a table of values; boards as a rendered PNG.
- **Templates.** `project_pages.is_template BOOLEAN` and a template gallery (workspace
  templates live in the workspace space; built-in ones ship as Markdown under
  `apps/server/src/modules/templates/pages/`). "New page from template" copies the document
  and fills `{{date}}`, `{{author}}`, `{{project}}`. Operation `page_create` gains
  `fromTemplateId`. Boards share the mechanism (`whiteboards.is_template`).
- **A page tab on a channel.** K3 has no `tab` link kind and none is added: tabs are a C1
  table `conversation_tabs (workspace_id, conversation_id, thing_kind, thing_id, position,
  added_by, added_at)` (R1 names), plus a K3 `embeds` link from the conversation to the thing
  so backlinks show "tab in #channel". Shown as tabs in the channel header under C2's part
  `channel.tabs` (C2.8 owns the header and the part, and hides the Page tab until this
  lands). "Add a page" creates a page in the channel's project titled after the channel. C1
  provides the table, the operation `conversation_tab_set {conversationId, thing, position}`
  (agent tool from day one) and the embedded editor. A DM or group in the direct scope (R20)
  has no tabs, since nothing but conversations lives there.
- **Turn a selection or a message into a page.** `page_create` gains `from`:
  `{messageIds: [...]}` (in order, authors and times kept as a quote block each, mentions as
  links), `{threadId}` (the whole thread, plus its outcome as a heading), or
  `{pageId, blockIds, leaveLink: true}` (moves blocks out of a page and leaves an embed in
  their place; this edit of the source page is proposable). A K3 link `made_from` points
  back, written through C2's one idempotent "made from a message" helper (the one
  `message_make` uses), and a system line in the thread says "Turned into page …". A page
  made from a DM or group message goes to a space the person picks, never into the direct
  scope (R20); `from: {messageIds}` then requires `spaceId`. Web: the message-menu entry is
  C2's "Make…" menu (page is one of its makers), page selection menu "Move to new page".
- **Events for automations.** `page.created`, `page.updated` (version, change set id,
  touched block count), `page.archived`, `page.restored`, `comment.created` (subject kind
  `page`), `proposal.created`, `proposal.decided`. K3 owns the log and the catalogue; C1
  emits from `page_docs::store::save` and the page row writers in `memory.rs`.

### 3.9 Sheets depth

- **The row lock.** Cell writes stop taking `lock_membership` (`FOR UPDATE OF project`).
  They read the membership `FOR SHARE` (a new `share_membership` beside `lock_membership`)
  and lock only the sheet row. Creating, renaming and archiving a sheet keep the project
  lock, because they check the per-project count and the unique name. Recalculation still
  runs under the sheet's lock (per-sheet order is what makes per-cell revisions correct); a
  later step can compute outside the lock and re-check the revision, if measurements say so.
- **Number formats and styles.** `sheet_cells` gains `format TEXT` (a spreadsheet number
  format code such as `0.00%`, `$#,##0`, `yyyy-mm-dd`) and `style JSONB` (bold, italic,
  colour, fill, alignment, wrap; at most 1 KB), plus `display TEXT` computed by the engine.
  Typing `10%` or `$5` keeps the format the engine infers. Formatting a range is a change
  kind (`{"format": {"range": "B2:B40", "numberFormat": "0.0%"}}`). `.xlsx` import and export
  keep them. Formatting does not bump a cell's input revision, so it never conflicts with an
  edit of the same cell's value.
- **Validation and dropdowns.** `sheet_validations (id, tab_id, range JSONB, rule JSONB,
  strict BOOLEAN)`: list of values (a dropdown), list from a range, number or date between,
  checkbox, text length. Strict rules refuse a change with a problem naming the rule
  (agents see why). Dropdown cells render a chooser in the grid and on a phone.
- **Find and replace.** `sheet_find {sheetId, query, matchCase?, wholeCell?, inFormulas?,
  tab?}` returns matches by address; replace is an ordinary `sheet_write` with the
  `ifRevision` of each match (so it is proposable and undoable).
- **Sort and filter views.** Sort a range is a change kind applied by permuting row inputs
  inside the engine call, so formulas pointing into the range are rewritten as a row move
  would. Filter views are saved, named, per-person or shared, and change no data:
  `sheet_views (id, sheet_id, tab_id, name, owner_actor_id NULL=shared, filters JSONB,
  sort JSONB, hidden_columns JSONB)`. One shared "basic filter" per tab is a view with a
  flag.
- **Conditional formatting.** `sheet_conditional_formats (id, tab_id, range, rule, style,
  position)`: value comparisons, text contains, top/bottom n, colour scales. Evaluated in the
  grid for display and written into `.xlsx` on export.
- **Charts.** `sheet_charts (id, sheet_id, tab_id, kind bar|line|area|pie|scatter, source
  range or named range, options JSONB, placement JSONB)`, drawn by T1's `ChartView` (T1.31,
  the same one Dashboards uses; R17). A chart is embeddable in a page and is a thing (`sheet_chart`). Agents
  get `sheet_chart_write` and a server-rendered SVG for `sheet_read include: ["charts"]`.
- **Cell comments.** Anchored threads (3.4); the grid marks commented cells with a corner
  and opens the thread in a side panel; "Comment" on a selection.
- **Marking recent edits.** Change sets (3.7); the grid outlines touched cells with the
  author's colour as edits arrive, and offers Undo on hover.
- **A source for dashboards and pages.** `sheet_named_ranges (id, sheet_id, name, tab_id,
  range)`; for T1's dashboards (T1.33) C1 provides `sheets::read_range(sheet, range) ->
  Vec<Vec<Value>>` (a range or named range, rows by header, checked against the reader's
  access); there is no separate `DataSource` trait in T1; page embeds of ranges already exist and gain named ranges and
  charts.
- **Limits raised safely.** After the lock fix, a benchmark slice measures `apply` at 25k,
  50k, 100k and 200k filled cells (load, engine, write-back). The target is Claude Sheets'
  100,000 cells if the 95th percentile write stays under 500 ms on the reference host;
  otherwise the limit stays and the slice records why. `cells()` loading every cell on every
  write (`sheets.rs:295`) is the first thing to measure.
- **Search and events.** `sheets.search_text` (names, tab names and text-valued inputs, at
  most 100 KB, rewritten in `apply`) indexed like `whiteboards.text_content`; events
  `sheet.created`, `sheet.updated` (revision, change set id, cells changed), `sheet.archived`,
  `sheet.restored`.

### 3.10 Whiteboards depth

- **Live cursors, selections and follow.** An open board sends K5's `watch` for itself and
  then `awareness` frames (protocol 2, K5.9) over the live connection at most 10 times a
  second: `{type: "awareness", document: {kind: "whiteboard", id}, state: {cursor: {x, y},
  selection: [elementIds], viewport: {x, y, zoom}}}`. K5 relays it to the board's other
  watchers on every server process without storing it. "Follow" makes the viewer's viewport
  track another person's. An agent has no pointer; its presence is derived from its change
  sets in the last minute (the elements it is drawing get its outline and name), which
  extends today's "is drawing" (`apps/web/src/whiteboards/WhiteboardPage.tsx:147`).
- **Comments on elements.** Anchored threads with `{"kind":"elements"}`; a pin on the
  canvas, a list in the side panel, resolve per thread.
- **Ask an account about this selection.** Select elements, choose "Ask", pick a person or
  agent: this posts a comment anchored to the selection with the mention and, as an
  attachment, a PNG of just the selection rendered on the server (`whiteboard_render`
  gains `elementIds`). The agent is woken by the mention like any comment, reads
  `whiteboard_read {elementIds}`, and answers in the thread or edits the board. One
  mechanism, no special "ask" channel.
- **Stickies into tasks.** `whiteboard_to_tasks {boardId, elementIds, projectId,
  defaults?, replace?}`: one task per sticky (label as title, frame name as a label, colour
  mapped by an optional rule), created through the task layer's batch create, a K3
  `made_from` link back to the board, and optionally the sticky replaced by a task card
  element that links to the task.
- **A board into a page.** `page_create {from: {whiteboardId, elementIds?}}`: frames become
  headings, stickies and text become bullet lists in reading order (top to bottom, left to
  right within each frame), connectors become "A → B" lines, plus an embed of the board and a
  rendered image stored as a file of the page.
- **Templates.** As pages (3.8): `whiteboards.is_template`, built-in templates
  (retrospective, user journey, flowchart, 2x2) as scene JSON in
  `apps/server/src/modules/templates/whiteboards/`.
- **Image storage through the seam.** Board images move into `files` with
  `thing_kind = 'whiteboard'` and `external_key` = Excalidraw's file id, bytes in
  `FileStore` (K5's storage seam). The routes `/api/v1/whiteboards/{id}/files/{file_id}`
  keep working. Duplicating a board copies file rows, not bytes (bytes are content-addressed
  per workspace by sha256).
- **Events and search.** `whiteboard.created`, `whiteboard.updated` (scene seq, change set
  id, elements changed), `whiteboard.trashed`, `whiteboard.restored`; boards join K3's search
  index from the existing `search_vector` (`20261017070000_whiteboards.sql:39-42`).

### 3.11 Files on every thing

```sql
-- C1.33
ALTER TABLE files
    ADD COLUMN thing_kind TEXT CHECK (thing_kind ~ '^[a-z][a-z_]{1,31}$'),
    ADD COLUMN thing_id UUID,
    ADD COLUMN purpose TEXT NOT NULL DEFAULT 'attachment'
        CHECK (purpose IN ('attachment', 'inline', 'cover', 'render', 'import', 'export')),
    ADD COLUMN external_key TEXT CHECK (external_key IS NULL OR external_key ~ '^[A-Za-z0-9_-]{1,100}$');
-- Exactly one owner: a conversation (a message's file), or a thing.
-- The old (conversation XOR work_item) check is replaced by
--   num_nonnulls(conversation_id, thing_id) = 1, with work_item_id kept in step
--   (thing_kind = 'task', thing_id = work_item_id) until C1.6 contracts it.
CREATE UNIQUE INDEX files_thing_external ON files (thing_kind, thing_id, external_key)
    WHERE external_key IS NOT NULL AND removed_at IS NULL;
```

`files::Destination` becomes `Conversation {..} | Thing { thing: ThingRef, purpose }`.
Reading checks `file_readable_by(file, viewer)`: a message's file through
`conversation_visible_to` and the message not removed (as today); a thing's file through
`subject_visible_to`. Uses: page image blocks (a new `image` block in `page_docs/tree.rs`
and the editor schema, stored as `![alt](/api/v1/files/{id})`), board images (3.10), sheet
import originals, renders. Previews: images already get thumbnails (`files/image.rs`); add
PDF first-page and text/code previews generated by a job, and an icon for anything else.
Quota and free-space checks split per K5 (`files.rs:476` `require_room` stops reading the
local disk when the store isn't local). Where K5's storage can sign links, upload and
download go straight to object storage with short-lived signed links and the bytes never
pass through the server (R24.2); disk storage keeps today's path.

**Files that live in an outside source (R23).** C1 owns the file thing and M1 the contract,
together; K5's storage code skips rows with `source_id` set (their bytes are not ours). `files` gains `source_id UUID NULL` (the
attached file source, a connected module installed under M1's file source contract) and
`source_ref TEXT NULL` (the source's own id for the file); both are null for our own uploads,
and `num_nonnulls(storage key, source_ref) = 1`. A source file is a thing shown **by
reference**: browse the source's folders (`list`), attach it to anything, link and mention it,
preview it (`thumbnail`, `content` with a range, fetched when asked, cached only as a
preview), search it by name (and by content where the source offers it), and "Open in
<source>". Bytes are never copied unless a person chooses "Keep a copy", which makes an
ordinary upload with a `made_from` link to the source file. A source that can `put` may be
chosen as where a space's new uploads go (`space_settings.upload_target`), so new uploads
become source files. Who sees a source's files: the members of the space it's attached to,
by the space's roles, through the same `file_readable_by`; the file view says plainly that
opening it in the source follows the source's own sharing. Tokens stay in the server's
secret store (M1's connection), never in the browser or the log. The file thing, links,
comments and search treat source files and uploads the same. Operations, each with its agent
tool: `file_source_browse {sourceId, folder?, cursor?}`, `file_attach {sourceId, ref, to}`,
`file_keep_copy {fileId}`, `space_upload_target_set {spaceId, sourceId?}`. Slice C1.36.

### 3.12 History on every thing (C1's side of K3's seam)

K3 owns the interface and the history screen's frame. C1 implements it for pages, sheets,
whiteboards and comments; T1 for tasks and records.

K3's names win: the trait is K3's `history::History { list, show, restore }` with
`HistoryEntry` and `HistoryCaps`, behind the operations `history_list`, `history_show`,
`history_restore` and `activity_list` (K3.19). C1 implements `history::History` for its
kinds; the sketch below is C1's mapping onto it (`versions` is `list`, `version` is `show`),
kept for the fields C1 must supply. The web history screen is W1.21's `HistoryPanel`.

Cold data (R24.7, K3.25): for pages and sheets (and boards the same way), history versions
older than a year move into snapshots plus object-stored deltas through K3's `History` seam,
and document update logs (Yjs updates) compact; old history payloads may move to object
storage by the same job. History is never cut, and an old version may take longer to open
(`show` fetches it back).

```rust
// C1's mapping onto K3's history::History (K3's names win).
#[async_trait]
pub trait HistoryProvider: Send + Sync {
    fn kind(&self) -> &'static str;
    async fn versions(&self, tx: &mut PgConnection, id: Uuid, page: Cursor) -> Result<Page<VersionEntry>, AppError>;
    async fn version(&self, tx: &mut PgConnection, id: Uuid, version: i64) -> Result<Option<VersionView>, AppError>;
    async fn restore(&self, tx: &mut PgConnection, actor: &CommandActor, id: Uuid,
                     version: i64, expected_version: i64) -> Result<Restored, AppError>;
}
pub struct VersionEntry { pub version: i64, pub at: String, pub actor_id: Uuid, pub via: Option<String>,
                          pub requested_by: Option<Uuid>, pub run_id: Option<Uuid>,
                          pub summary: String, pub change_set_id: Option<Uuid>, pub proposal_id: Option<Uuid> }
```

- Pages: versions from `project_page_revisions`; restore exists. The revision's
  `agent_session_id/agent_harness/agent_run_id` columns (`20261112031500`) stay readable;
  new writes record `run_id` and `requested_by` (A1) and stop writing a harness name.
- Sheets: snapshots in `sheet_snapshots (sheet_id, revision, tabs JSONB, cells BYTEA
  compressed, created_at)` every 10 minutes of activity and before imports, bulk deletes and
  restores (as boards do), plus change set inverses in between; restore writes the
  snapshot's inputs back through `apply`, so formulas recompute and it is one undoable change.
- Whiteboards: existing snapshots; versions list joins change sets for finer entries.
- Comments: message revisions (view only; "restore" of a message is an edit).

### 3.13 Events and search: what C1 emits

K3 owns `events`, the catalogue and the search index. C1 calls `events::emit` from the same
transactions that enqueue today's outbox rows and registers search sources:

| Kind | Events | Search source |
| --- | --- | --- |
| comments | `comment.created`, `comment.edited`, `comment.removed`, `comment_thread.resolved` (subject kind and id, anchor kind, message id; never text) | messages already indexed; results show "comment on <thing>" |
| page | `page.created/updated/archived/restored` | existing `project_pages.search_vector` |
| sheet | `sheet.created/updated/archived/restored` | `sheets.search_text` (new) |
| whiteboard | `whiteboard.created/updated/trashed/restored` | existing `whiteboards.search_vector` |
| proposal | `proposal.created/decided` | none |
| change set | `change.undone` | none |

### 3.14 Live editing with several server processes: what C1 needs from K5

1. **Durable fan-out to every process.** `page.updated`, `sheet.updated`,
   `whiteboard.updated`, `message.created` (including comments), with the change set id,
   reach every browser subscribed to that thing on any process within a second. Today one
   process claims each outbox row and only its sockets hear it (`realtime.rs:675-765`;
   F report F6), and editors stop polling while live (`LivePage.tsx:343-347`,
   `BoardCanvas.tsx:236-240`).
2. **Watching a document.** Under K5's protocol 2 the server decides the audience; an open
   editor sends `watch {document}` for `page`, `sheet` or `whiteboard` (and `unwatch`) to
   receive `document.update` and `awareness`; access is checked at watch time and when the
   viewer's access changes (K5's `access` frame plus K1), not per event. Live document rooms
   are audiences under the live-role contract (R24.5). C1 provides the check K5 calls at watch
   time and when `api` issues the ticket: `documents::can_view(conn, viewer, DocumentRef) ->
   Result<bool>` (page, sheet, whiteboard; through `sql_filter_thing` once K1 lands, today's rule
   per kind until then), in C1.30's slice or earlier if K5.9 needs it.
3. **An ephemeral presence channel.** K5's `awareness` frames per watched document, never stored,
   relayed across processes by K5's live role (never by `NOTIFY` alone, R24.3), rate-limited per
   connection, dropped on disconnect, carrying cursor, selection, viewport and "typing in
   block X". Used by boards (3.10), the page editor (Yjs awareness: who is where) and the
   grid (selected cell per person).
4. **The storage seam** for files, board images, renders and exports, with a store-specific
   room check.
5. **Shared rate limits** for comment posting, proposals and presence per account.
6. **Leases** for C1's background jobs: the comment copy jobs, file previews, board image
   move, change-set retention, sheet snapshots.

No in-memory document rooms are needed: every write already goes to PostgreSQL under a row
lock (`page_docs/store.rs:543,719`; `sheets.rs:246-259`; `whiteboards/store.rs`). If K5's
fan-out is late, C1.10 adds a 20-second poll while live as a stopgap (F report F6's own
suggestion).

## 4. Migration path

The live server will apply every pending migration in one go on its next deploy, so no
migration assumes another pending one ran with data in its new shape, and every backfill
is idempotent (`INSERT … ON CONFLICT DO NOTHING`, `UPDATE … WHERE new IS NULL`).

**Thread creation (C1.2).** Pure code. Behaviour change: threads named from a message by
an agent are `auto_named` from now on (old rows unchanged). `web_suggested_thread_name`
stays for threads the web named before.

**Discussion (C1.3).** Expand only: new columns, kind, visibility, function bodies,
`subject_visible_to`. Nothing writes `comments` conversations yet except the new
`comment_post` for sheets, whiteboards and records (which have no old comments, so there is
nothing to migrate for them).

**Task comments (C1.4): copy, then switch, then contract.**

1. Migration `<ts>_task_comments_into_conversations.sql`, in one transaction:
   - for each task with comments, insert its `comments` conversation
     (`subject_kind = 'task'`, `visibility = 'subject'`, project the task's) with a
     deterministic id (`uuid` from the task id via `md5('comments:' || id)::uuid`) so a
     re-run finds the same row;
   - copy each comment into `messages` **with the same id**, author, `created_at`,
     content, plain text, `revision`, `edited_at`, `removed_at`, `removed_by_actor_id`,
     `via_oauth_grant_id`, and `reply_to_message_id = reply_to_comment_id`;
   - copy `work_item_comment_revisions` to `message_revisions`,
     `work_item_comment_mentions` to `message_mentions`, `work_item_comment_references` to
     `message_work_item_references`, and add `conversation_entries` in `created_at, id`
     order, setting `next_entry_sequence`;
   - `files`: set `conversation_id` and `message_id` from `comment_id` for comment files
     (task-level files stay on the task);
   - `notifications`: set `message_id` and `conversation_id` where `comment_id` is set;
   - `work_item_timeline_entries` gains `message_id UUID REFERENCES messages(id)` and kind
     `'message'`; rows of kind `comment` get `message_id = comment_id` (same ids);
   - `thing_references`: the message trigger function is replaced so a message in a
     `comments` conversation records `thing_kind/thing_id` = its subject (K3 later absorbs
     this into the link store); `task_comment` rows are re-written as `message` rows.
   The message immutability triggers (`0047_message_edits.sql:71,109,138,194`) are disabled
   for the copy statements only (`ALTER TABLE … DISABLE TRIGGER …` and re-enable in the same
   migration), so the copy doesn't need `aw.message_command` per row. Task comment volume
   on the live server is small enough for one transaction (the slice measures it on a copy
   of production data first; if it is over about 200,000 rows the copy moves to a resumable
   job keyed by comment id, with reads served from the old tables until the job reports
   done).
2. The same slice switches every reader and writer: the task timeline reads messages
   (`work_items.rs` timeline view, `mcp/tasks.rs:2159`, `mcp/history.rs`,
   `ai/summaries.rs:205`, `notifications.rs:880-925` becomes the message mention path plus
   `thing_comment` for followers), the two create paths call `discussion::post`, edit and
   delete use `message_edits`, a task move copies the comments conversation's messages to
   the new task's conversation (`work_item_move.rs:2554`), and HTTP routes and `task_comment*`
   tools keep their shapes (`WorkItemCommentView` is built from `MessageView`; `commentId`
   is the message id, which is the old comment id).
3. A trigger on `work_item_comments` refuses inserts and updates from then on ("task
   comments moved to conversations in C1.4"), so any writer the slice missed fails loudly in
   tests instead of writing to a dead table.
4. C1.6, one release later: drop `work_item_comment_*` tables, `files.comment_id`,
   `notifications.comment_id`, `work_item_timeline_entries.comment_id`, the old trigger
   functions, and the freeze triggers.

Rolling back the binary after step 1 is not supported (the old code would hit the freeze
trigger); the slice ships with the copy verified on a production snapshot.

**Page comments (C1.5).** Same pattern: one comments conversation per page with comments;
each `project_page_comments` thread becomes a thread (same id as the thread's first comment)
under it with `anchor = {"kind":"text","blockId","quote"}` (or none), `auto_named`,
`thread_status = 'Resolved'` when `resolved_at` is set (and `'Open'` otherwise); each comment
becomes a message with the same id, `content` from its plain body, `edited_at` kept (no
revisions existed), `removed_at = deleted_at`. Notifications of kind `page_comment` get
their `detail.preview` removed in the same migration if P0's new preview slice (R10; the
140-character bug at `page_docs/comments.rs:451`) has not already done it.
`page_comments`/`page_comment`/`page_comment_update` and `/api/v1/pages/{id}/comments*`
keep their shapes. Freeze trigger, then C1.6 drops the table.

**Page suggestions (C1.7).** Copy `project_page_suggestions` into `proposals` with the same
ids (`thing_kind = 'page'`, `operation = 'page_edit'`, `change = {"edits": edits}`), keep
`page_suggestions`/`page_suggestion_decide` and `/suggestions/{id}/decision` as aliases,
freeze the old table, drop it in C1.6 (or a C1.7 follow-up if C1.6 has already merged).

**Suggest as a role (C1.8).** Needs K1's permission levels. To keep today's behaviour on
the day the code rule goes, K1's role migration (K1.12, R5) gives existing agent memberships
"Member, suggests" (edit own, suggest on others; the data equivalent of `must_suggest`), and
new agents get the workspace's default role for new agents. K1.12 is the only step that
selects rows by account kind, and it deletes `must_suggest`, `is_agent` and the decide rule
in the same slice; C1.8 does not delete them again, it routes `Propose` grants into
proposals.

**Board images (C1.29).** A resumable job (K5 leases) moves each `whiteboard_files` row into
`files` and the store, keyed by `(board, file_id)`; reads try `files` first and fall back to
`whiteboard_files` until the job reports done; then a contract migration drops the `data`
column.

**Sheet cell formats (C1.14).** New nullable columns; no backfill.

## 5. Slices

Three ordered tracks. Track 1 (discussion) and track 2 (proposals and change marks) are
mostly sequential inside themselves; track 3 (depth) is grouped by document and runs in
parallel with the other tracks once its dependencies are in. Sizes: S about a day, M a few
days, L about a week.

### Track 1: one message system

**C1.1 Task comment parity now (S).** The MCP `task_comment` builds content with
`content_from_text` and task-key references, and one `insert_task_comment` in
`work_item_commands.rs` (content resolution, mentions, references, insert, `finalize_comment`,
files) is used by `work_items.rs:2859` and `mcp/writes.rs:1243`. Mentioned agents with a
connection get a `comment.mentioned` external event (same shape as `message.mentioned`,
href to the comment), for both task and page comments; A2.4 delivers it and emits it itself
until this slice lands. R10: the task-comment mention fix is done by P0.4, and the
page comment preview (`page_docs/comments.rs:451`) by P0's new preview slice after P0.35;
here only what goes beyond them, the `comment.mentioned` agent event (and the shared
`insert_task_comment` if P0.4 did not already make one).
- Changes: `mcp/writes.rs`, `work_items.rs`, `work_item_commands.rs`, `external_events.rs`.
- Depends on: P0.4 (and P0's preview slice for the last acceptance line).
- Acceptance: an agent's `task_comment` "@mara see AWD-12" notifies Mara and appears in
  AWD-12's "Referenced in"; a comment mentioning a connected agent enqueues one
  `comment.mentioned` event for it and none for an agent not in the project; a deleted page
  comment's words are in no notification row. Tests: `tests/integration/mcp_tasks.rs`
  (mention and reference rows written), `tests/integration/notifications.rs` (no preview
  text in `detail`), `tests/integration/m9_external_agents.rs` (event enqueued).

**C1.2 One thread-creation function (M).** `conversations/threads.rs::create` as in 3.3;
the six callers in 2.2 move to it; `launch.rs`'s copy of `unique_thread_name` goes.
Derived names set `auto_named`. Every path writes an audit row and locks its parent through
`lock_conversation_for`.
- Changes: `conversations.rs`, new `conversations/threads.rs`, `mcp/talk.rs`,
  `mcp/replies.rs`, `work_items.rs`, `launch.rs`.
- Depends on: nothing. Can run beside C1.1 and C1.12.
- Acceptance: each of the six entry points creates a thread with an audit row, read state
  and one `ConversationCreated`; a thread an agent starts from a message is renamed when
  that message is removed; a launch into a private channel the caller can't see is refused.
  Tests: a new `tests/integration/threads.rs` table-driven over the six entry points; a
  guard (6.1) that `INSERT INTO conversations` with `'thread'` appears only in
  `conversations/threads.rs` and `seed.rs`.

**C1.3 Discussion foundation (L).** Migration 3.2 (kind `comments`, `subject_*`, `anchor`,
visibility `subject`, `subject_visible_to`, the parent trigger). R3: if the split of
`conversation_visible_to` into helpers has not landed, do it first, without changing
behaviour (`conversation_member_visible` holds today's rule; K1's body composes the helpers);
then this migration adds only `subject_visible_to` and its arm. The `ThingComments` panel
(`{thing: ThingRef, anchor?, threadId?}`) is what W1.21's discussion panel shell hosts. Module `discussion` with `open_for`, `post`, `DiscussionSubject` for task, page,
sheet, whiteboard and record (task and page registered but their old comment paths still
used until C1.4 and C1.5), anchors (3.4) with checks per kind, the three pipeline hooks,
`thing_comment` follower notifications, operations `comment_post`, `comment_list`,
`comment_thread_update` (HTTP routes under `/api/v1/things/{kind}/{id}/comments` and MCP
tools). Web: a shared lazy `ThingComments` panel (`apps/web/src/discussion/`) built from
`StructuredComposer` and `Timeline`, registered as part `thing.comments` in the layout
registry, not yet placed on any page.
- Depends on: C1.2, P0.0 (listings filtered first). Uses K3's thing registry if merged, else
  a static table in `discussion/subjects.rs` that K3 later absorbs.
- Acceptance: a person comments on a sheet and a whiteboard through the API; mentioning an
  agent creates a reply request in that comments conversation and the agent answers with
  `reply_send`; a member of another project gets 404 for the comments, the realtime event
  and the file; a comment on a board in the workspace space is visible to every workspace
  member and to nobody else. Tests: new `tests/integration/discussion.rs` (posting,
  anchors, reply request, isolation per subject kind), an assertion in
  `tests/integration/direct_conversations.rs` that DMs and private channels still hide,
  and the plan-shape check for the timeline query.

**C1.4 Task comments become conversations (L).** Section 4's task comment steps 1 to 3.
Reactions, unread state, link previews and search now work on task comments; the task
page shows unread comments; the moderation rule (`work_items/comments.rs:782`) becomes K1's
`channels.moderate` on the comments conversation.
- Depends on: C1.3, C1.1.
- Acceptance: after the migration every old comment opens by its old id through
  `task_comment_edit`, keeps its revisions, files, replies and notifications; the task
  timeline interleaves activity and comments in the same order as before; an agent
  mentioned in a task comment is woken; an agent with the moderate permission can delete
  others' comments; inserting into `work_item_comments` fails. Tests:
  `tests/integration/awd95_comment_replies.rs` and `m4_work_items_http.rs` kept green
  unchanged (shape compatibility), a new migration test that seeds old comments with
  revisions, mentions, files and notifications and checks every one after the copy,
  `tests/integration/awd102_task_move.rs` (comments move with the task).

**C1.5 Page comments become conversations (M).** Section 4's page steps. `PageDiscussion.tsx`
uses `ThingComments` with anchors; mentions, reactions, files and replies work in page
comments; commenting no longer locks the page row.
- Depends on: C1.3. Can run beside C1.4.
- Acceptance: every old page thread appears with its anchor, status and comments;
  "@agent can you tighten this paragraph?" on a quote wakes the agent with a reply request
  in that thread. Tests: `tests/integration/awd53_discussion.rs` kept green, plus a
  migration test and an agent-woken case.

**C1.6 Retire the old comment tables (S).** Contract step: drop the frozen tables, columns
and triggers listed in section 4, and the legacy-table allowlist in guard 6.2.
- Depends on: C1.4 and C1.5 deployed for one release; C1.7's suggestion copy if merged.
- Acceptance: the schema has no `work_item_comment*`, `project_page_comments`
  (`project_page_suggestions` too once C1.7 is deployed); all tests green. Proof that no old
  feature is lost: the shape-compatibility tests named in C1.4 (`awd95_comment_replies.rs`,
  `m4_work_items_http.rs`, `awd102_task_move.rs`) and C1.5 (`awd53_discussion.rs`) stay green
  unchanged on the new tables before the drop.

### Track 2: proposals and change marks

**C1.7 The proposals record, with pages moved onto it (M).** Table 3.6, the `Proposable`
trait and its page implementation (wrapping `suggestions::resolve` and the accept path of
`suggestions::decide`), `proposal_list` and `proposal_decide`, the page suggestion copy and
aliases, `proposal.created/decided`. If A1's requests exist, a proposal raises one and its
decision resolves it; otherwise the page creator's notification is kept as today and the
request link is added when A1 lands. Agents who are owed a decision get it as a delivered
event (the `notify` skip at `suggestions.rs:457` is replaced). Web: the review panel
(`apps/web/src/documents/proposals/`, lazy) used by the page editor in place of the
suggestion list.
- Depends on: C1.3 (for the proposal's thread). Can run beside C1.4.
- Acceptance: old suggestions appear as proposals with the same ids; accepting one applies
  the edits as the author with "accepted by" in history; a stale one is refused and marked
  stale; `page_suggestion_decide` still works. Tests: `tests/integration/awd53_discussion.rs`
  (unchanged shapes) and new `tests/integration/proposals.rs`.

**C1.8 Suggest is a role setting (M).** The page write path calls K1's `access::write_mode`
(instead of `must_suggest`). K2's `PROPOSABLE` flag on `page_update` and
`page_update_blocks` (K2's names; `page_write` and `page_edit` stay as aliases), and from
C1.11 the sheet, board and record writes, routing a `Propose` grant from K1's
`access::write_mode` into `proposals::create`; `propose: true` for anyone. `must_suggest`,
`is_agent` and the agents-decide-only-their-pages rule (`suggestions.rs:42-48, 558-565`) are
deleted by K1.12 together with the "Member, suggests" role migration (R5); this slice does
not delete them again.
- Depends on: C1.7, K1.12 (suggest level, "edit own" scope, role migration and the deletion),
  K2 (operation registry with the flag). If K2 isn't ready, the routing goes in a small
  `documents::write_gate` called by the existing tools, and moves into K2 later.
- Acceptance: a person whose role says "suggest" on pages gets a proposal from the web
  editor's save; an agent whose role says "edit" edits a person's page directly; an agent
  with the migrated default role still proposes on others' pages and edits its own. Tests:
  `tests/integration/proposals.rs` (role matrix by permission, not by account kind) and the
  guard 6.4.

**C1.9 Change sets and one-tap undo (L).** Table 3.7, writes from the page, sheet and board
write paths (sheet inverses are the previous inputs of changed cells), `change_undo`,
`change_set_list {thing, since?, actor?}`, the change set id on the outbox events, the
retention job.
- Depends on: C1.12 (the sheet write path it edits). Can run beside track 1.
- Acceptance: an agent's `sheet_write` of 40 cells is undone by one call and the cells hold
  their earlier inputs; a cell changed by someone since is skipped and listed; undo of an
  undo restores the agent's values; a page block edited since is skipped. Tests: new
  `tests/integration/change_sets.rs` per kind.

**C1.10 Recent-edit outlines in the three editors (M).** The grid, the page editor and the
board outline touched items from change sets as they land and on a "Recent edits" toggle,
with the author's name and an Undo button; phone views show a "changed by" chip. Includes
the 20-second poll while live if K5's fan-out hasn't merged.
- Depends on: C1.9. Web only plus one read route.
- Acceptance: checked in a browser (the `check-in-browser` skill): an agent writes through
  MCP while a person watches the sheet; the cells outline with the agent's name within two
  seconds; Undo restores them. A unit test for the outline model in each editor.

**C1.11 Proposals for sheets, whiteboards and records (L).** `Proposable` for sheet
(`sheet_update` changes with `ifRevision`), whiteboard (`whiteboard_update_elements`
operations with touched element versions) and record (T1's `record_update_many`, wired in
T1.41); the review previews for each
kind (dashed cells, ghost elements, field diff); partial accept.
- Depends on: C1.8, C1.9, T1.41 for the record part (the sheet and board parts can merge
  first).
- Acceptance: with a suggest-only role, `sheet_write` returns `outcome: proposed`; the
  sheet shows the proposed cells; accepting three of five applies three; a board proposal
  over an element moved since is stale. Tests: `tests/integration/proposals.rs` per kind.

### Track 3: depth per document

Sheets (C1.12 first; then at most two sheet slices at a time, since they share
`sheets.rs::apply`: {C1.13, C1.14}, then {C1.15, C1.17}, then {C1.16, C1.18}; C1.19 to
C1.21 can join any pair):

**C1.12 Sheet writes stop holding the project lock (S).** `share_membership` (`FOR SHARE`)
for cell and tab changes in `sheets/http.rs` and `mcp/sheet_tools.rs`; the project lock
stays for create, rename and archive.
- Depends on: nothing. Can start on day one.
- Acceptance: while a 50,000-cell write recalculates, a task update in the same project
  commits without waiting. Test: `tests/integration/awd54_sheets.rs` with two connections
  and a lock-wait timeout.

**C1.13 Sheet history: snapshots and restore (M).** `sheet_snapshots`, the history list
(revisions with authors, via and change sets), view a past revision, restore as one
undoable change, `sheet_history` and `sheet_restore` tools, a History panel.
- Depends on: C1.9 for change-set entries (the snapshot half can merge first).
- Acceptance: restoring revision 12 of a sheet at revision 40 makes revision 41 with
  revision 12's inputs and recomputed values. Test: `awd54_sheets.rs`.

**C1.14 Number formats and cell styles (M).** Columns and engine support (3.9), formatting
toolbar, `sheet_write` format changes, `.xlsx` round trip.
- Acceptance: `10%` keeps showing as 10% and computes as 0.1; a styled `.xlsx` imports and
  exports with formats kept. Tests: `awd54_sheet_files.rs`, engine unit tests.

**C1.15 Validation and dropdowns (M).** `sheet_validations`, strict refusals with a clear
problem, dropdown chooser on wide and narrow screens, `sheet_write` validation changes.
- Acceptance: an agent writing "Maybe" into a strict Yes/No column is refused with the rule
  named. Test: `awd54_sheets.rs`.

**C1.16 Find and replace, sort, and filter views (L).** `sheet_find`, replace through
`sheet_write`, sort as a change kind, `sheet_views` with personal and shared views, the
grid's filter UI and view switcher.
- Acceptance: sorting B2:F40 by C keeps a formula that pointed at a moved row pointing at
  it; a personal filter view hides rows for its owner only. Tests: `awd54_sheets.rs`.

**C1.17 Conditional formatting (S).** `sheet_conditional_formats`, rule editor, display,
`.xlsx` export.
- Acceptance: a "less than 0 is red" rule colours the right cells after an edit.

**C1.18 Charts (M).** `sheet_charts`, chart editor in the grid, embed in pages, SVG for
agents.
- Depends on: T1.31 `ChartView` (if it isn't ready, the slice ships the bar and line charts
  as a local component that T1.31 replaces; named stopgap).
- Acceptance: a chart over a named range updates when a cell changes; it embeds in a page.

**C1.19 Cell comments in the grid (S).** Corner marks, the thread side panel, "Comment" on
a selection, anchors moved by row and column changes inside `apply`.
- Depends on: C1.3.
- Acceptance: inserting a row above a commented cell moves the comment with it. Test:
  `tests/integration/discussion.rs` (sheet anchor follows a row insert).

**C1.20 A sheet as a thing and a source (M).** `sheet` in K3's thing registry (or
`modules/things.rs::KINDS` and `thing_references` targets if K3 isn't there),
`sheet_named_ranges`, `sheets::read_range(sheet, range) -> Vec<Vec<Value>>` for T1's
dashboards (T1.33), named-range page embeds.
- Depends on: K3 (things).
- Acceptance: a page linking a sheet appears in its "Referenced in"; a dashboard chart
  reads a named range.

**C1.21 Raise sheet limits after measuring (S).** A benchmark under
`apps/server/benches/` (run on the CI host, not the Mac) and the limit change it justifies.
- Depends on: C1.12.
- Acceptance: the slice states measured numbers and either raises `MAX_CELLS` with tests at
  the new limit or keeps it with the numbers recorded in `docs/plans/sheets.md`.

Pages (all can run beside the sheet and board groups):

**C1.22 Ask in a page comment (S).** `page_edit` and `page_write` take `threadId`; the
change set records it; the thread shows "Edited the page · Undo"; the server instructions
and tool descriptions tell agents to answer in the thread that asked.
- Depends on: C1.5, C1.9.
- Acceptance: an end-to-end test where a person mentions an agent on a quote, the agent
  claims, edits and replies, and the thread shows the linked change. Test:
  `tests/integration/awd53_discussion.rs`.

**C1.23 PDF export (M).** Print stylesheet; `format=pdf` through an in-process typesetter;
embeds rendered as described in 3.8.
- Depends on: nothing (board images in PDFs use `whiteboard_render`).
- Acceptance: a page with headings, a table, a code block, a task embed and a board embed
  exports to a PDF that opens and contains the text. Test: `awd53_transfer.rs` (PDF bytes
  start with `%PDF`, text extracted contains the headings).

**C1.24 Templates for pages and boards (M).** `is_template` on both, galleries, built-in
templates, `fromTemplateId`, variables.
- Acceptance: "New page from template" in a project copies the template with `{{date}}`
  filled; an agent does the same with `page_write`.

**C1.25 A page from a message, a thread or a selection (M).** `page_create` `from` variants
for messages, threads and page blocks, the `made_from` link through C2's one idempotent
helper, the thread's system line, the page entry in C2's "Make…" message menu and the page
selection menu. From a DM or group message the person picks a space (R20).
- Depends on: K3 links (else `thing_references` plus a stored `made_from` column on the
  page as a stopgap), C2's `message_make` helper (C2.20; stopgap: write the link here and
  move it into the helper when C2's lands).
- Acceptance: "Make a page" on three selected messages makes a page quoting them in order
  with a link back; moving blocks to a new page leaves an embed and is undoable.

**C1.26 A page tab on a channel (S).** `conversation_tabs` table (3.8), `conversation_tab_set`
(operation and agent tool), the K3 `embeds` link, tabs drawn in C2's channel header under C2's
part `channel.tabs`.
- Depends on: C2.8 (channel header and the part; C2.8 does not depend on this), K3 links
  (stopgap: the table alone).
- Acceptance: adding a page tab shows the page in the channel for every member who can see
  both, and hides the tab for someone who can't see the page.

**C1.27 Page sharing levels and public links (M).** Share dialog, public page route,
`subject_visible_to('page')` delegating to K1's per-thing rule.
- Depends on: K1 (per-thing grants and link tokens).
- Acceptance: a page shared "comment only" with a guest lets them comment and not edit;
  revoking a public link makes `/p/{token}` 404 at once. Tests: an isolation case per level
  in `tests/integration/awd96_workspace_pages.rs` and a new public-link test.

**C1.28 Documents in events and search (M).** Emit the events in 3.13 from pages, sheets,
boards, comments and proposals; register sheets and boards as search sources; automation
triggers for `page.updated`, `sheet.updated`, `whiteboard.updated`, `comment.created`,
`proposal.created`.
- Depends on: K3 (event log and search index). If K3's log isn't merged, emit through
  today's `events.rs` catalogue and `outbox` as K3's adapter allows.
- Acceptance: an automation "when a comment is posted on a page in project X, ask agent Y"
  fires; search finds a word that appears only in a sheet cell and only for members.

Whiteboards (parallel with pages and sheets):

**C1.29 Board images through the storage seam (M).** The move job, the read fallback, then
the contract migration.
- Depends on: C1.33 (files on things), K5 (storage seam and leases).
- Acceptance: after the job, a board's images load from the file store and the
  `whiteboard_files.data` column is gone; duplicating a board doesn't copy bytes.

**C1.30 Live cursors, selections and follow (M).** Presence in the board through K5's
`watch` and `awareness` frames, follow mode, agents' derived presence.
- Depends on: K5.9 (protocol 2 with `awareness`).
- Acceptance: two browsers on two server processes see each other's cursor within half a
  second; following a person moves the follower's viewport. Checked in a browser.

**C1.31 Element comments and "ask about this selection" (M).** Pins, side panel, the Ask
flow with a selection render, `whiteboard_render`/`whiteboard_read` with `elementIds`.
- Depends on: C1.3.
- Acceptance: asking an agent about three stickies wakes it with a reply request whose
  thread is anchored to them and carries a PNG of the three. Test:
  `tests/integration/discussion.rs`.

**C1.32 Stickies into tasks, a board into a page (M).** `whiteboard_to_tasks`, the page
`from: {whiteboardId}` variant, web actions.
- Depends on: K3 links (stopgap as C1.25).
- Acceptance: five stickies in two frames become five tasks labelled by frame with links
  back; the board becomes a page with two headings and the stickies as bullets.

Files and history:

**C1.33 One attachment model and previews (M).** Section 3.11: thing-owned files,
`Destination::Thing`, `file_readable_by`, page image blocks, PDF and text previews,
`file_upload` taking `thing`.
- Depends on: C1.3 (`subject_visible_to`). K5 storage seam for anything beyond local disk.
- Acceptance: an image pasted into a page is stored as a file of the page and readable only
  by those who can see the page; old task and message files read as before. Tests:
  `tests/integration/files.rs` (or the existing file tests' home), isolation per owner kind.

**C1.34 History on every document through K3's seam (M).** K3's `history::History` for page,
sheet, whiteboard and comments; W1.21's `HistoryPanel` used by all three editors; `run_id`
and `requested_by` on new page revisions; the compaction job for old update logs, snapshots
and history payloads (R24.7: compact or move to object storage, never cut).
- Depends on: K3.19 (the trait and operations), C1.13, W1.21.
- Acceptance: the History panel lists who, through which app, on whose request, and the
  change set for each version of a page, a sheet and a board, and restores each; a version
  whose payload was moved by the compaction job still opens and restores.

**C1.35 The document-kind contract test (S).** Guard 6.5 and the matrix integration test.
- Depends on: C1.11, C1.28, C1.34.
- Acceptance: the test fails when a new document kind lacks any of the five services.

**C1.36 Files from an outside source, by reference (M; R23).** Section 3.11's second part:
`files.source_id`, `files.source_ref`, the file thing for source files, browse (`list`),
attach, link, preview (`thumbnail`, ranged `content`, fetched when asked), search by name
(content where the source can), "Open in <source>", "Keep a copy", and a space's upload
target set to a source that can `put`. Operations with agent tools from day one:
`file_source_browse`, `file_attach`, `file_keep_copy`, `space_upload_target_set`. The file
view says opening in the source follows the source's own sharing. No source is named in the
core; a fake source in tests implements M1's contract.
- Depends on: C1.33, M1.18 (the file source contract), K5 secret store.
- Acceptance: with a fake source attached to a space, a member browses it, attaches a file to
  a task and a page, previews it and finds it by name; a non-member gets 404 for the file and
  its preview (isolation case over HTTP and MCP); "Keep a copy" makes an upload with a
  `made_from` link; with the source as upload target, a new upload calls `put` and is stored
  as a source file; no token appears in any response, log line or event.

**C1.37 Move a page between spaces (S/M).** `page_move {pageId, toSpaceId, expectedVersion}`
(operation and agent tool): the page keeps its id, history, comments conversation (its
`project_id`/`space_id` follows), files and links; embeds and tabs pointing at it keep working;
viewers who can no longer see it lose it at once (access is the new space's). Needs edit on the
page and create in the target space. This is how a draft in a personal space is published (K4).
- Depends on: C1.3, C1.33, K4 spaces (K4.9).
- Acceptance: moving a page from a personal space to a team space keeps its versions, comments
  and files; a member of the old space who isn't in the new one gets 404 on the page, its
  comments and its files (isolation case over HTTP and MCP); a link and an embed elsewhere still
  open it for those who can see it.

## 6. Guard rails

1. **One thread maker.** A unit test in `conversations/threads.rs` scans `src/**/*.rs` for
   `INSERT INTO conversations`; only `conversations/threads.rs`, the channel and direct
   creators, `discussion/mod.rs` (comments conversations) and `seed.rs` may contain it.
2. **No second comment system.** A test in `discussion/mod.rs` scans
   `apps/server/migrations/*.sql` for `CREATE TABLE` names matching
   `comment|annotation|remark|note_thread|reply` and fails unless allowlisted (the legacy
   tables until C1.6, then nothing); and scans `src/**/*.rs` for `INSERT INTO messages`
   outside the send pipeline (`conversations/send` after the B report's split, or
   `conversations.rs`) and the run-message writers P0 or A1 are moving (allowlisted with
   their slice ids, so the list can only shrink).
3. **One visibility gate.** A test asserts every SQL string in `src/` that selects from
   `conversations` or `messages` also calls `conversation_visible_to` (or is on a short
   allowlist of admin and migration paths), extending B report finding 6's proposed guard,
   and that `subject_visible_to` is called only by `conversation_visible_to` and
   `file_readable_by`.
4. **No account kind in document permissions.** A test scans `page_docs/`, `sheets*`,
   `whiteboards*`, `discussion/`, `documents/` and `files*` for `ActorKind::`,
   `actor.kind`, `kind = 'human'`, `'external_agent'` and `is_agent(`, allowlisting only
   delivery code (who gets an inbox row versus an event). K1's broader guard may replace it.
5. **Every document kind gets every service.** `documents::KINDS` (page, sheet, whiteboard,
   and record and task registered by T1) must each have a `DiscussionSubject`, a
   `Proposable`, a `HistoryProvider`, events in K3's catalogue and a search source; a unit
   test checks the registrations, and `tests/integration/document_contract.rs` runs, per
   kind: comment with a mention wakes an agent; a suggest-only role makes a proposal;
   accept applies; a change set undoes; a version restores; the event is in the log; search
   finds it; an outsider gets 404 on each.
6. Keep the existing guards: `agent_parity` (every new write has a tool), the module claims
   tests in `modules/mod.rs`, the layout registry test for new parts.
7. **Triggers on hot tables (R16).** C1 adds these and removes them as listed; none writes a
   table another plan's trigger writes:

   | Table | Trigger | Added | Removed |
   | --- | --- | --- | --- |
   | `work_item_comments` | freeze: refuse insert and update | C1.4 | C1.6 (table dropped) |
   | `project_page_comments` | freeze | C1.5 | C1.6 |
   | `project_page_suggestions` | freeze | C1.7 | C1.6 (or C1.7's follow-up) |
   | `messages` | the `thing_references` trigger function replaced so a message in a `comments` conversation records its subject (writes `thing_references` only) | C1.4 | when K3's link store absorbs `thing_references` (K3's slice) |
   | `conversations` | parent trigger: `anchor` only under a `comments` parent | C1.3 | kept (a check, writes nothing) |
   | `0047` message immutability triggers | disabled for the copy statements only, re-enabled in the same migration | C1.4 | same migration |
8. **Operation names are K2's.** New and changed operations use K2's canonical names
   (`page_create`, `page_update`, `page_update_blocks`, `sheet_create`, `sheet_update`,
   `record_create_many`, `record_update_many`, `whiteboard_update_elements`,
   `conversation_mark_read`); older names in this plan (`page_write`, `page_edit`,
   `sheet_write`, `whiteboard_edit`) mean those operations and stay as K2 aliases.

## 7. Risks and open questions

**Risks.**

1. **Copying comments into messages changes many readers at once (C1.4).** About ten
   server readers and the task page. *Notice:* the shape-compatibility tests in
   `awd95_comment_replies.rs` and `m4_work_items_http.rs`, the migration test with seeded
   old data, and the freeze trigger turning any missed writer into a test failure.
   *Fallback:* the copy keeps ids, so if a reader is found broken after deploy it can be
   pointed at messages by id; old tables stay for one release for comparison.
2. **`conversation_visible_to` gets slower.** It is in every conversation query. *Notice:*
   C1.3 compares `EXPLAIN ANALYZE` of the sidebar, timeline and search queries on a
   production-sized copy before and after. *Fallback:* keep the subject arm out of the hot
   function and filter `comments` conversations out of the sidebar and channel lists
   (they never belong there anyway), so only subject-scoped reads pay for it.
3. **Comment conversations leak into chat lists.** Sidebars, unread counts, "my
   conversations", `conversation_list`, briefing and search must show comments as
   "comment on <thing>", never as a channel. *Notice:* a test in C1.3 that lists every
   conversation surface for an account with only task comments. *Fallback:* each list
   filters `kind <> 'comments'` until its owner (C2) adds a proper "comments" view.
4. **Behaviour change when the account-kind rule goes (K1.12, routed by C1.8).** If K1's
   role migration is missing or wrong, agents start editing people's pages directly.
   *Notice:* C1.8's test with the migrated default role. *Fallback:* C1.8 depends on K1.12.
5. **Sheet undo and history size.** Inverses for a 10,000-cell paste are big. Capped at
   1 MB with restore as the fallback; snapshots bound restore cost.
6. **Presence load across processes.** Ten `awareness` updates a second per viewer through
   K5's live role is fine for tens of boards; at thousands it depends on the live role's
   implementation (R24.5). *Notice:* K5's metrics. *Fallback:* lower the rate, or an edge live
   role behind K5's contract.
7. **Anchors drift.** Quotes that no longer match and cells deleted. Shown as "words
   changed" or "cell removed" with the thread kept; never silently dropped.

**Questions only the owner can answer** (with the recommendation):

1. Should today's "agents only suggest on pages they didn't create" survive as the default
   role for existing agents? *Recommend yes* for existing workspaces (no surprise), and new
   workspaces' starting agent role gets "edit" like a new member (END-STATE section 10,
   call 3).
2. Should a comment need its own permission ("comment") separate from edit? Today a task
   comment needs "edit tasks" and a page comment needs "edit pages". *Recommend yes*, as
   END-STATE's role levels already say (see, comment, suggest, edit, manage), with
   existing roles given comment wherever they had edit.
3. Public page links on hosted workspaces: allowed by default, or off until an admin turns
   them on? *Recommend off by default*, on per workspace, because a hosted free tier
   attracts abuse.
4. PDF export: is an in-process typesetting library in the server acceptable (adds a
   sizeable dependency), or should PDF be the browser's print only? *Recommend* the library,
   because agents need PDF without a browser.

## 8. Touch points

**C1 needs:**

- **K1:** permission levels per kind (see, comment, suggest, edit, manage) and an "own"
  scope ("edit own, suggest on others"); `access::write_mode(conn, caller, kind, target) ->
  Result<Result<Grant, Denied>>` with `Grant.mode: Direct | Propose` (R2); `sql_filter_thing`
  for `subject_visible_to` to delegate to; the R3 split of `conversation_visible_to` (K1 owns
  the body); per-thing grants and public link tokens (C1.27); K1.12's role migration and
  deletion of `must_suggest`/`is_agent` (R5); `channels.moderate` applying to any
  conversation, including comments.
- **K2:** the operation registry with a `proposable` flag and aliases for old tool names;
  every C1 write is an operation (until K2 lands C1 writes tools by hand and lists them in
  `agent_parity`).
- **K3:** the thing registry (C1 adds `sheet`, `sheet_chart`, and registers
  `DiscussionSubject`, `Proposable`, `history::History` per kind); the link store with kinds
  `made_from` and `embeds` (no `tab` kind: tabs are C1's `conversation_tabs` table); the event
  log and catalogue entries in 3.13; search sources; the history trait and operations (K3.19,
  3.12). Also: in the link store, a message in a `comments` conversation
  counts as a reference *from its subject*.
- **K4:** `project_id` to `space_id` on `conversations`, `proposals`,
  `document_change_sets`, `files`; the workspace space becomes the workspace's own space.
  C1 uses `workspace_space` meanwhile for workspace boards.
- **K5:** the six items in 3.14; protocol 2's `watch`, `awareness` and `document.update`
  (K5.9); signed upload and download links (R24.2); cold storage for compacted history
  (R24.7).
- **A1:** `requests` with kind `proposal` (raise, resolve, assignees); `request_answer` on a
  proposal request calls `proposal_decide`; run ids on change sets and proposals.
- **M1:** the file source contract (M1.18) for C1.36.
- **A2:** delivery of `comment.mentioned`, reply requests in comments conversations, and
  proposal and change events to agents; the rule for agent-authored mentions.
- **C2:** notification kind `thing_comment` and the inbox rendering of comments and
  proposal requests; the channel header and part `channel.tabs` (C2.8); the one idempotent
  `made_from` helper and the "Make…" menu (C2.20); conversation lists that show comments in
  their own section, not as channels.
- **T1:** `ChartView` (T1.31); record and task writes as proposable operations with a
  `Proposable` and `history::History` (T1.41); the file field type uses C1's thing-owned
  files.
- **W1:** the discussion panel shell and `HistoryPanel` (W1.21); the shell's lazy routes for
  the review panel and public page; generated types.
- **P0:** P0.0 (listings), P0.4 (task comment mentions) and the new preview slice after
  P0.35 (R10); C1.1 keeps only what goes beyond them.

**C1 provides:**

- `conversations::threads::create` (to everyone who makes threads, A1's runs included).
- `discussion::post` and `comment_*` operations (to T1 for record comments, to A1 for run
  discussion if wanted, to M1 so built and connected modules get comments by registering a
  `DiscussionSubject`).
- `proposals` and `Proposable` (to T1 for tasks and records, to M1 for module things).
- `document_change_sets` and `change_undo` (to K3's audit view and to A1's run page, which
  can list "what this run changed").
- Thing-owned files, source files by reference (C1.36) and `file_readable_by` (to T1's file
  field, to M1).
- `sheets::read_range` and named ranges (to T1.33).
- `conversation_tabs` and `conversation_tab_set` (to C2.8's header).
- `documents::can_view` (to K5's watch and ticket checks).
- Moving a page between spaces, `page_move` (to K4: publishing a draft from a personal space;
  C1.37).

## 9. Changes to the end state

1. **Discussion, more precisely.** Replace the "Discussion" bullet with: "**Discussion**:
   one message system. A channel, a thread, a DM and the comments on any thing are all
   conversations. A thing's comments are one conversation attached to it, created by the
   first comment; comments on a part of it (words in a page, a cell, board elements, a
   record field) are threads in that conversation anchored to the part. Mentions,
   reactions, replies, files, unread state, notifications and waking an agent work the same
   everywhere, and who may see a thing's comments is who may see the thing."
2. **History needs a unit smaller than a version.** Add to "History": "Every write to a
   document is a change set: who, through which app, at whose request, what it touched. A
   change set can be undone on its own while what it touched hasn't changed since, and the
   app outlines recent change sets as they land." This is what makes test-for-done item 4
   ("one tap to undo it") buildable for sheets, which today keep no earlier inputs at all.
3. **"Suggest instead of edit" needs an ownership scope.** Today's rule is "agents edit
   their own pages and suggest on others'". To express it as a role, roles need "edit own"
   as well as "edit". Add to the Role bullet: "…what they may do: see, comment, suggest,
   edit (all, or only things they made), manage."
4. **Section 8, Pages row:** "What is strong today" should include Word import and export
   (`page_docs/docx.rs`); the gap is PDF only.
5. **Section 8, Sheets row:** add "history that can restore" to the gaps: sheets record
   that a change happened but not what it replaced.

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| C1.1 | Task comment parity now (beyond P0.4) | S | P0.4 | C1.2, C1.12, all of track 3 not needing C1.3 |
| C1.2 | One thread-creation function | M | none | C1.1, C1.12 |
| C1.3 | Discussion foundation (R3 split first if not landed) | L | C1.2, P0.0 (K3 things if merged) | C1.12, C1.23, C1.24 |
| C1.4 | Task comments become conversations | L | C1.1, C1.3 | C1.5, C1.7, C1.9 |
| C1.5 | Page comments become conversations | M | C1.3 | C1.4, C1.7, C1.9 |
| C1.6 | Retire the old comment tables | S | C1.4, C1.5 deployed one release (C1.7 for suggestions) | anything |
| C1.7 | Proposals record, pages moved onto it | M | C1.3 (A1 requests if merged) | C1.4, C1.5, C1.9 |
| C1.8 | Suggest is a role setting (routing only) | M | C1.7, K1.12, K2 | track 3 |
| C1.9 | Change sets and one-tap undo | L | C1.12 | track 1, C1.7 |
| C1.10 | Recent-edit outlines in the editors | M | C1.9 | C1.11, track 3 |
| C1.11 | Proposals for sheets, boards, records | L | C1.8, C1.9 (T1.41 for records) | C1.10, pages and boards groups |
| C1.12 | Sheet writes stop holding the project lock | S | none | everything |
| C1.13 | Sheet history: snapshots and restore | M | C1.12 (C1.9 for change entries) | C1.14 |
| C1.14 | Number formats and cell styles | M | C1.12 | C1.13 |
| C1.15 | Validation and dropdowns | M | C1.12 | C1.17 |
| C1.16 | Find and replace, sort, filter views | L | C1.12 | C1.18 |
| C1.17 | Conditional formatting | S | C1.14 | C1.15 |
| C1.18 | Charts | M | C1.14, T1.31 `ChartView` (stopgap: local bar and line) | C1.16 |
| C1.19 | Cell comments in the grid | S | C1.3 | any sheet slice |
| C1.20 | A sheet as a thing and a source (`sheets::read_range`) | M | K3 things | any sheet slice |
| C1.21 | Raise sheet limits after measuring | S | C1.12 | any sheet slice |
| C1.22 | Ask in a page comment | S | C1.5, C1.9 | sheets and boards groups |
| C1.23 | PDF export | M | none | everything |
| C1.24 | Templates for pages and boards | M | none | everything |
| C1.25 | A page from a message, thread or selection | M | K3 links, C2.20 (stopgaps allowed) | sheets and boards groups |
| C1.26 | A page tab on a channel (`conversation_tabs`) | S | C2.8 (K3 links; stopgap: table alone) | sheets and boards groups |
| C1.27 | Page sharing levels and public links | M | K1 per-thing grants, C1.3 | sheets and boards groups |
| C1.28 | Documents in events and search | M | K3 events and search | sheets and boards groups |
| C1.29 | Board images through the storage seam | M | C1.33, K5 storage | pages and sheets groups |
| C1.30 | Live cursors, selections and follow | M | K5.9 (`watch`, `awareness`) | pages and sheets groups |
| C1.31 | Element comments and ask about a selection | M | C1.3 | C1.30, C1.32 |
| C1.32 | Stickies into tasks, a board into a page | M | K3 links (stopgap allowed) | C1.30, C1.31 |
| C1.33 | One attachment model and previews | M | C1.3 (K5 storage for non-local) | track 2, sheets group |
| C1.34 | History on every document via K3's seam; compaction | M | K3.19, C1.13, W1.21 | pages and boards groups |
| C1.35 | Document-kind contract test | S | C1.11, C1.28, C1.34 | anything |
| C1.36 | Files from an outside source, by reference | M | C1.33, M1.18, K5 secret store | sheets, pages, boards groups |
| C1.37 | Move a page between spaces | S/M | C1.3, C1.33, K4.9 | sheets and boards groups |

## Review changes

- 3.2 and C1.3: R3 wording; C1 owns only `subject_visible_to`; C1.3 does the helper split first if it has not landed; C1.3 depends on P0.0.
- 3.6: K1's `access::write_mode` returning `Grant{mode: Direct|Propose}` or `Denied` replaces `access::decide`/`SuggestOnly` (R2); K2's `ProposalDraft` feeds `prepare`.
- 3.6: accept is `proposal_decide` applying through `Proposable::apply` inside a K2 dispatch with `Edge::Proposal`, resolving A1's request idempotently (R14; K2's separate re-dispatch is not used).
- 4 and C1.8: K1.12 deletes `must_suggest` and `is_agent` with the "Member, suggests" migration (R5); C1.8 only routes and depends on K1.12.
- C1.1: the task-comment mention is P0.4's and the 140-character preview is P0's new slice (R10); C1.1 keeps the `comment.mentioned` agent event; C1.5's preview removal only if P0 hasn't.
- 3.8 and C1.26: no `tab` link kind in K3; a C1 `conversation_tabs` table plus a K3 `embeds` link; C2 owns part `channel.tabs`; C1.26 depends on C2.8, not the reverse.
- 3.8 and C1.25: `made_from` written through C2's one idempotent helper (C2.20); the message-menu entry is C2's "Make…"; from a DM, a page goes to a space the person picks (R20).
- 3.9, C1.18, C1.20: charts drawn by T1.31's `ChartView`; C1 provides `sheets::read_range` and named ranges instead of a `DataSource` trait.
- 3.10, 3.14, C1.30: presence and cursors use K5 protocol 2's `watch` and `awareness`; no `NOTIFY`-only relay (R24.3, R24.5).
- 3.11: signed links for bytes where storage can sign (R24.2); new "Files that live in an outside source" and new slice C1.36: `files.source_id`, `files.source_ref`, by-reference browse, attach, preview, search by name, open in source, keep a copy, upload target (R23).
- 3.12 and C1.34: K3's `history::History` trait and operations (K3.19), W1.21's `HistoryPanel`; cold history compacts or moves to object storage, never cut (R24.7).
- C1.4 moderation maps to `channels.moderate`; C1.6 names the shape tests that prove the old comment features before the drop.
- Guard 7: R16 trigger table; guard 8: operations follow K2's canonical names (aliases kept). C1.8 and C1.11 use them.
- C1.11 records part depends on T1.41; touch points and slice table follow all of the above.
- 3.14 provides `documents::can_view` for K5's watch and ticket checks (K5's assumption had no home).
- New slice C1.37, moving a page between spaces, which K4 assumes C1 provides (K4.9) and no plan held.

### Second pass

- 3.2: the composed `conversation_visible_to` is kept exactly as K1 §3.10 shows it (kernel review; R27 also confirms the tabs table).
- C1.8: the page write path calls `access::write_mode`; it does not delete `must_suggest` (K1.12 does).
- 3.12 and C1.34: history older than a year moves into snapshots plus object-stored deltas through K3's `History` seam, and update logs compact (K3.25).
- 3.11: K5's storage code skips rows with `source_id`; C1 and M1 own the file source contract together.
- C1.1: `comment.mentioned` for task and page comments; A2.4 delivers it (agents and hosting review).
