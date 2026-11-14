# K3: Things, links and events

> **Build order (R44).** This plan's slices are built inside the large builds listed in
> `slices.json` and `BUILD-ORDER.md`, not one pull request each. Where the text below says
> one session or one pull request per slice, or names a wave, read the slice's build.
> R45 to R50 also apply: approval is by permission for any kind of account, and a new seam
> ships with a real user.

Planner's draft, 2026-11-14, against `main` at fe95a74 (pull request #251, tasks as records part
four, is not in this checkout). Paths are relative to `/home/user/agent-workspace` unless they
start with `/`. "Inferred" marks a claim not read directly in code.

## 1. Goal

When K3 is done, every kind of thing (a message, task, page, sheet, whiteboard, record, file,
run, decision, conversation, space) has one row in one registry, links between any two things
live in one table and show on both ends to whoever may see both, and every change writes one
event into one ordered log in the same transaction. Live updates, notifications, webhooks,
automations, the search index, agent delivery (A2), the audit trail and activity feeds each
read that log with their own cursor and never lose an event. A person or an agent notices that
a page, sheet or whiteboard edit can fire a rule or a webhook and be found in search, that
"made from this message" and a task's link fields show as links, and that an agent polling
or streaming events from one cursor sees exactly what a browser and a webhook see.

## 2. Where the code is today

### 2.1 Every place a reference between two things is stored

| # | Store | What it holds | Written by | Read as a link by `things::links`? |
| --- | --- | --- | --- | --- |
| 1 | `work_items.parent_work_item_id` | Subtask of task (`part_of`) | task commands (81 references in 17 files) | yes, `apps/server/src/modules/things.rs:383-388` |
| 2 | `work_item_dependencies` (`migrations/0052_task_dependencies_and_milestones.sql:7`) | task blocks task, same project | `work_item_planning.rs:630` | yes, `things.rs:390-394` |
| 3 | `work_item_relations` (`migrations/20261016100000_tasks_at_scale.sql:123`) | `relates_to` (stored once either way, unique on the pair) and `duplicates`, may cross projects | `work_item_planning.rs:921` | yes, `things.rs:396-400` |
| 4 | `work_item_links` (`migrations/0042_agent_interface.sql:302`) | task to `message`, `page`, `decision`, `conversation` | `work_items.rs:2447` (task made from a message), `mcp/tasks.rs:4063` (`task_link`) | page, decision, conversation only; **the `message` target (made from) is never read** (`things.rs:402-406`) |
| 5 | `work_item_conversation_links` (`migrations/0018_m4_work_item_links.sql:1`) | task to thread (a trigger insists on threads, `:15-35`); overlaps #4's `conversation` | `work_items.rs:1579`, `work_item_commands.rs:1609`, `launch.rs:1836` | yes, `things.rs:408-409` |
| 6 | `whiteboard_links` (`migrations/20261017070000_whiteboards.sql:87`) | board to `project`, `work_item`, `page` | `whiteboards.rs:786` | yes, `things.rs:410-412, 454-463, 479-484` |
| 7 | `message_work_item_references` (`migrations/0017_m4_work_item_collaboration.sql:82`) | task chips in messages (`WorkItemRef` nodes, `message_content.rs:68-82`) | `conversations.rs:2198` | yes, `things.rs:419-423` |
| 8 | `work_item_comment_references` (`0017:95`) | task chips in task comments | `work_items.rs:3006`, `work_items/comments.rs:204` (web only; MCP comments write none, `C-tasks.md` finding 6) | yes, `things.rs:413-418, 431-435` |
| 9 | `thing_references` (`migrations/20261113060000_thing_references.sql:15`, widened by `20261113193000_record_references.sql:10-13`) | "Referenced in": app URLs found in message text, page bodies, task descriptions, task comments, by regex in SQL (`thing_reference_targets`, `20261113193000:17-55`) | triggers on `messages`, `project_pages`, `work_items`, `work_item_comments` (`20261113060000:94-187`) | yes, for task, page, whiteboard, conversation, record |
| 10 | Collection link fields in `collection_records.field_values` (JSONB keyed by field id) | record to task, page or record | `collections/records.rs:706` `write` | yes, `things.rs:488-505, 515-527` |
| 11 | Task link fields in `work_items.field_values` | task to anything a link field targets | task field commands | **no**: `RECORDS_LINKING_SQL` requires `definition.collection_id IS NOT NULL` (`things.rs:522`) and the task arm never reads task field values |
| 12 | `github_pull_request_work_item_links`, `_conversation_links`, `_workspace_links` (`migrations/0023_m5_github_pull_requests.sql:122,143,159`) | pull request to task, conversation, run | `github.rs:3434`, `mcp/writes.rs:1593` | no (read by `events.rs:638-709` and the code module) |
| 13 | "Made from" columns: `conversations.origin_message_id` (`0005_m2_conversations.sql:13`), `project_decisions.source_message_id` / `source_conversation_id` (`0042:256-257`), `launches.source_message_id` (`20261016220100_launches.sql:11`), `whiteboards.duplicated_from_id` (`20261017070000:31`) | thread, decision, run, board made from a message or a board | each kind's create path | no |
| 14 | Embeds: a line `[label](/path "embed")` in a page body (`page_blocks.rs`, `page_docs/markdown.rs`) | page embeds a task list, sheet, collection, board | page saves | only as a plain reference through #9, and only for paths the regex knows; sheets are not in the regex at all |
| 15 | Account mentions: `message_mentions` (`0005:363`), `work_item_comment_mentions` (`0017:68`) | a message or comment names a person or agent | `conversations.rs:2185`, `work_items.rs:2994`, `work_items/comments.rs:192` | no (they drive notifications and reply rules, not backlinks) |
| 16 | `files.conversation_id` / `files.work_item_id` (`20261017150000_files_and_link_previews.sql:16`) | a file attached to a conversation or task | `files.rs:277-290` | no |

Not links between things, and out of scope: `task_import_links` (an import's mapping),
`bug_report_links` (form tokens), `agent_setup_links`, `link_previews`.

What has the right shape and must be kept: **one reader** (`things::links`, `things.rs:535-623`)
that checks the other end's visibility through the kind resolvers (`conversation_visible_to`,
`whiteboard_visible_to`, project lists) and the other end's own modules (`things.rs:562-596`),
with a cap (`MAX_LINKS`, `:29`); the web's one "Referenced in" component used by tasks, pages,
boards and records (`F-content-modules.md` F2); trigger-maintained references that catch every
write path; the `LinkKind` vocabulary (`modules/contract.rs:60-110`).

What is in the way: sixteen stores, a `links_sql` arm per kind (`things.rs:379-509`), a regex
over the web app's URL shapes inside a migration (so a route change silently stops backlinks;
the workspace-in-the-URL change in W1 would break every one of them), sheets missing from
`KINDS` (`things.rs:52-62`) and from every CHECK, and three holes users can see (#4 `message`,
#11, #14 for sheets).

### 2.2 Every event and notification mechanism

| Mechanism | Where | Vocabulary | Kinds covered | Durable? |
| --- | --- | --- | --- | --- |
| Outbox | `outbox_events` (`migrations/0005_m2_conversations.sql:519-572`), `outbox.rs:736` `enqueue`, claimed by `outbox.rs:911` `claim_ready` | 18 types, `outbox.rs:292-312` (`conversation.message.created`, `work_item.updated`, `page.updated`, `whiteboard.updated`, `sheet.updated`, `collection.updated`, ...) | conversations, messages, reactions, read state, runs, tasks, memberships, notifications, boards, pages, page discussion, collections, sheets | yes; `event_cursor` is an identity column (`0005:523`); no actor column; published rows deleted after 30 days (`retention.rs:356`) |
| Realtime | `realtime.rs:675-760` publisher claims a batch, broadcasts to an in-process `tokio::broadcast` (`:131-166`), then in one acknowledgement runs `notify_people` and `queue_event_route` | outbox names; the web switches on them (`apps/web/src/realtime/useRealtimeConnection.ts:323-327`) | as the outbox | broadcast is per process (`G-backbone-data.md` F1). **Notifications and routing failures are logged and the event still marked published** (`realtime.rs:724-726, 770-844`; `G` F2) |
| Webhooks and automations | `events.rs:151` `queue_route` (from the publisher), `events.rs:229` `route`, `expand` (`:394-620`) | 11 names, `events.rs:34-52` | messages, runs, tasks, records, pull requests (derived from runs and tasks) | via `background_jobs` (`events.route` job unique per outbox event) |
| `Subject` | `events.rs:70-93` | a struct of optional per-kind ids (`work_item_id`, `record_id`, `collection_id` ...) | grows a field per kind | |
| Outgoing webhooks | `webhooks.rs:895` `fan_out`, delivery rows `webhook_deliveries`, access rechecked at send (`webhooks.rs:998-1052`), payload rendered live (`:1468-1632`) | same 11 | same | yes, job queue |
| Automations | `automations.rs:1482` `on_event`; triggers are a closed enum (`:120-191`, `TRIGGER_KINDS` `:193-203`); `kinds_for` maps event names to triggers by hand (`:1405-1417`); dedupe on `automation_runs (rule_id, dedupe_key)` (`20261017040100_automations.sql:75`) | | tasks, runs, PR checks failed, messages, records, schedules | yes |
| Notifications | `notifications.rs:400` `fan_out`, called by the publisher; dedupe keys per occurrence (`notifications.rs:1-23`) | | messages (mentions), message removals, tasks, runs (`fan_out` match arms) | rows yes; the fan-out step is the lossy one above |
| Agent external events | `external_agent_events` (migration 0034) written inline by `external_events.rs:36` `enqueue_external_event` with a per-connection counter (`:47-58`) from 15 call sites (`conversations.rs:2403`, `work_item_commands.rs:947,2677,2834,2883,2917`, `work_item_workflow.rs:830`, `domain.rs:340`, `later.rs:1027`, `replies.rs:536`, `questions.rs:382`, `external_agents.rs:447,1118,1544`, `external_execution.rs:429,762`) | a third vocabulary: `message.mentioned`, `work_item.changed`, `work_item.assigned`, `agent_run.terminal`, `reminder.due`, `reply.requested`, `external_execution.*`, `connection.created` | | yes, 30 days (`external_events.rs:10`); read by `external_event_poll` (`mcp.rs:3659`, "kept for existing connectors") |
| Reply wake-ups | `replies.rs:516` `wake` writes an external event and an `agent_webhook_deliveries` row (`0038_reply_requests.sql:73`); a hand-written loop sends them (`replies.rs:1251-1270, 1315`) | `reply.requested`, `reply.followup`, `reply.cancelled` (`replies.rs:497-503`); old body `inbox.available` (`replies.rs:1446`) | reply requests only | yes |
| Audit | `audit_events` (`migrations/0004_m1_identity_projects.sql:151-178`), written in 26 files, read by `audit_log.rs` | `<thing>.<verb>` mostly (`work_item.created`, `work_item.mcp_updated`) plus `denied` and `failed` outcomes | whatever each path remembers; thread creation over MCP writes none (`B-conversations.md` finding 4) | yes |
| Module manifests | `modules/*.json` `events.emits` | | only tasks, collections, messages, runs, code emit; **pages, sheets, whiteboards emit nothing and offer no triggers** | |
| Ephemeral | `RealtimeHub::publish` (`realtime.rs:146`), `message.delta` from `provider.rs:785` | | streaming text | no, and should stay so |

Writes that emit no event at all (`G-backbone-data.md` F5, spot-checked in `memory.rs`):
decisions, conversation pins, milestones, labels, saved views, files, person questions (only an
agent event), field definitions.

Who acted is not on the outbox row. It is in each kind's history row, and the app a person
acted through is a transaction setting, `aw.via_grant`, read by `current_via_grant()`
(`migrations/0042_agent_interface.sql:28-34`, set by `domain.rs:130-142`). Nothing records
"requested by".

### 2.3 Search today

`search.rs` runs one SQL query per kind (`SearchKind`, `search.rs:30-78`), each against a
generated `tsvector` column on the kind's own table: messages (`0042:175`), tasks (`0042:186`),
pages (`0042:204`), decisions (`0042:262`), files (`20261017150000:43`), records
(`20261113082000_collection_record_search.sql:8`). Threads compute `to_tsvector` on the fly
with no index (`search.rs:262-270`). Access: projects pre-filtered by the caller, then
`conversation_visible_to`, `projects_showing_tasks`, and module switches per kind
(`search.rs:128-170`). Missing kinds: **whiteboards** (they have a `simple`-config
`search_vector`, `20261017070000_whiteboards.sql:39-52`, used only by the board list's filter,
`whiteboards.rs:173`), **sheets** (no text column at all), task comments, page comments,
runs. `SearchHit.project_id` is a non-null `Uuid` (`search.rs:96`), so a workspace board could
not be returned as is.

### 2.4 History today

| Kind | Store | Who / via | Show a version | Restore |
| --- | --- | --- | --- | --- |
| Page | `project_page_revisions` (`0042:219`), live sessions merge (`20261112031500`) | actor, via | yes (`memory.rs:470` `get_revision`) | yes (`memory.rs:680-691`, web `MemoryPages.tsx:389`) |
| Whiteboard | `whiteboard_revisions`, whole scene snapshots (`20261017070000:104`) | actor, via | yes | yes (`whiteboards/store.rs:356` `restore_revision`) |
| Sheet | `sheet_revisions` summary of changed inputs (`20261112193000_sheets.sql:123`) | actor, via | no: written (`sheets.rs:528`), never read | no |
| Record | `collection_record_history` field before/after (`20261111081500_collections.sql:165`) | actor, via | change list (`collections/records.rs:401`) | no |
| Task | `work_item_activity` JSON payload (`0015:113`) and `work_item_timeline_entries` | actor, via | change list (`work_item_history.rs`, `mcp/history.rs`) | no |
| Message | `message_revisions` (`0047_message_edits.sql:30`) | | earlier versions visible | no |
| Task comment | `work_item_comment_revisions` (`20261028070000:91`) | | yes | no |

Five shapes, three readers for task history alone (`C-tasks.md` finding 4).

### 2.5 Ids and addresses today

- Every row id is a UUID made in Rust (UUIDv7, `G-backbone-data.md` §1); a few seeded rows use
  `gen_random_uuid()`. Ids are unique across kinds in practice, which the registry relies on.
- People-facing keys: tasks are `<project_key>-<number>` (`things.rs:127`) with
  `work_item_key_aliases` so a moved task's old key still resolves (`work_item_commands.rs:1227`,
  `mcp.rs:5688`); records show `Untitled #<number>` (`things.rs:207`); sheets have a unique
  name per project so agents can name them (`20261112193000:52-54`); channels are `#name`.
- Addresses: each manifest's thing `href` template (`modules/tasks.json:82`,
  `modules/pages.json:43`, `modules/whiteboards.json:32`, `modules/collections.json:28` with
  `?record={id}`, `modules/messages.json:55`), filled by `things.rs:85-97`. Decisions and
  milestones have no per-thing address (`pages.json:59`, `tasks.json:95`).
- Mention syntax: `@handle` becomes a `Mention { actorId }` node (`message_content.rs:68-82`,
  resolved in `mcp/common.rs:549`), a task chip is a `WorkItemRef { workItemId }` node, and
  any other thing is referenced by pasting its URL.

## 3. The target design

### 3.1 The thing registry

**Decision: `things` is a real table that every kind writes one row to, kept current by
triggers on each kind's own table.** A view over the per-kind tables would avoid a second copy,
but it cannot be the target of a foreign key (links and events need one), cannot carry one
index for K1's access filter or for search, needs a migration per new kind, and makes "open id
X" a union over every kind. The cost of the table is one narrow insert per new thing and one
update when an enveloped column changes, inside a transaction that already writes the kind's
row, its history and an event. Triggers rather than Rust calls because they catch every write
path (imports, automations, MCP, triggers of their own) the way `thing_references` does today,
and because a projection kept by a trigger is the "invariant" half of triggers the health check
says to keep (`G` F6).

```sql
-- migrations/<ts>_thing_registry.sql
CREATE TABLE thing_kinds (
    kind TEXT PRIMARY KEY CHECK (kind ~ '^[a-z][a-z0-9_]{1,39}$'),
    module TEXT NOT NULL,             -- the manifest id that makes it
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Seeded by the migration with today's kinds; the server upserts the
-- registered kinds at startup (M1's data-made modules add rows the same way).

CREATE TABLE things (
    id UUID PRIMARY KEY,                       -- the kind row's own id, unchanged
    kind TEXT NOT NULL REFERENCES thing_kinds(kind),
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    space_id UUID REFERENCES projects(id),     -- K4 renames; NULL = workspace level
    parent_id UUID REFERENCES things(id) DEFERRABLE INITIALLY DEFERRED,
                                               -- containment: message->conversation,
                                               -- thread->channel, record->collection,
                                               -- subtask->task, file->what it's attached to
    access_parent_id UUID REFERENCES things(id) DEFERRABLE INITIALLY DEFERRED,
                                               -- whose visibility governs this one:
                                               -- message->its conversation, decision->its
                                               -- source conversation, file->its holder
    title TEXT CHECK (title IS NULL OR char_length(title) <= 300),
                                               -- NULL for kinds whose title is text that
                                               -- removal must scrub (messages, comments)
    status TEXT CHECK (status IS NULL OR status ~ '^[a-z][a-z0-9_]{0,39}$'),
                                               -- a stable key, not a display name: a task's
                                               -- semantic_category, a thread's status, a run's state
    owner_account_id UUID REFERENCES actors(id),   -- assignee for tasks, agent for runs,
                                                   -- creator otherwise
    created_by_account_id UUID REFERENCES actors(id),
    updated_by_account_id UUID REFERENCES actors(id),
    version BIGINT NOT NULL DEFAULT 1,         -- the kind's own version or revision
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    archived_at TIMESTAMPTZ,
    deleted_at TIMESTAMPTZ,                    -- tombstone: the row stays so old events and
                                               -- links can still be access-checked
    CHECK (id <> parent_id), CHECK (id <> access_parent_id)
);
CREATE INDEX things_space_kind ON things (space_id, kind, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX things_workspace_kind ON things (workspace_id, kind, updated_at DESC) WHERE deleted_at IS NULL;
CREATE INDEX things_parent ON things (parent_id) WHERE parent_id IS NOT NULL;
CREATE INDEX things_access_parent ON things (access_parent_id) WHERE access_parent_id IS NOT NULL;
CREATE INDEX things_owner ON things (owner_account_id) WHERE deleted_at IS NULL;
```

Notes on the shape:

- **Same id.** A thing's id is its kind row's id. Nothing is re-keyed; every URL and payload
  stays valid. Tasks as records (T1) keeps the task's id on its record; the registry has one
  row of kind `task` for both (see the trigger rule in §4.1).
- **No text that must vanish.** Titles of messages and comments stay `NULL` and are resolved
  live, so the message removal scrub (`message_edits.rs:654` `apply_removal`) gains nothing to
  scrub here. Removal sets `deleted_at` through the trigger.
- **`space_id`** is today's project id. Workspace boards (`whiteboards.project_id IS NULL`) get
  `NULL` until K4 gives every workspace its own space; DMs and groups keep the hidden direct
  project's id until K4 moves them out, when K4's slice rewrites the column.
- **`parent_id` replaces links for containment.** "Subtask of", "in this collection", "message
  in this conversation" and "file on this task" are the thing's parent, not rows in `links`.
  The link reader shows `part_of` from `parent_id` both ways. This keeps the hot tree columns
  (`parent_work_item_id`, 81 uses) as they are and mirrors them, instead of moving tree queries
  onto a generic table.
- **Kinds in the registry** at the end: `space` (today `project`), `conversation`, `channel`,
  `thread`, `message`, `task`, `milestone`, `page`, `decision`, `sheet`, `whiteboard`,
  `collection`, `record`, `file`, `run`, `pull_request`, `repository`, `automation`. A DM or
  group is kind `conversation`. Task comments and page comments are not things: C1 turns them
  into conversations, which are.

The Rust side is one registration per kind, the seam M1 later fills from manifests:

```rust
// apps/server/src/things/mod.rs  (moved out of modules/things.rs; old path re-exports)
pub struct ThingKindSpec {
    pub kind: &'static str,
    pub module: &'static str,
    /// Display fields the envelope doesn't hold: a task's key and status name,
    /// a page's excerpt. SQL over `things` joined to the kind's table.
    pub resolver: &'static str,
    pub events: Support<&'static [&'static str]>,      // event types it emits
    pub search: Support<crate::search::SearchSource>,
    pub history: Support<crate::history::HistoryCaps>,
    pub export: Support<&'static str>,                  // H1's exporter id
    pub links: Support<LinkCaps>,                       // may be a link end; may hold text refs
}
pub enum Support<T> { Yes(T), Not(&'static str /* why, in a sentence */) }

pub fn registry() -> &'static [&'static ThingKindSpec];   // each module's `pub const KIND`
pub async fn get(conn: &mut PgConnection, viewer: &Viewer<'_>, id: Uuid) -> Result<Option<Thing>, AppError>;
pub async fn get_many(conn: &mut PgConnection, viewer: &Viewer<'_>, ids: &[Uuid]) -> Result<Vec<Thing>, AppError>;
/// The ids among `ids` the viewer may see now. Until K1's SQL filter lands this
/// wraps today's per-kind rules (the resolvers in things.rs); K1 replaces the body
/// (K1.20, through `access::sql_filter_thing`). This is a named stopgap, not a second
/// access rule.
pub async fn visible_ids(conn: &mut PgConnection, viewer: &Viewer<'_>, ids: &[Uuid]) -> Result<Vec<Uuid>, AppError>;

/// A kind's name. Open-ended (M1's built and connected modules add kinds at runtime), so a
/// newtype over a string with constants for the core kinds (`ThingKind::TASK`, ...).
/// K1's `access::Target` and K2's `ops::Target` use this type. It lives in
/// `things/kind.rs`; whichever of K1.2, K2.2 or K3.10 merges first creates that file.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct ThingKind(Cow<'static, str>);

/// Where a thing lives, without access checks: for K2's dispatcher (step 4, "locate the
/// target"), which then asks K1's `access`. Reads only `things` (one primary-key lookup);
/// `None` for an unknown id. The caller compares `kind` with the kind it expected.
pub struct Located { pub id: Uuid, pub kind: ThingKind, pub workspace_id: Uuid,
                     pub space_id: Option<Uuid>, pub access_thing_id: Option<Uuid>,
                     pub version: i64, pub deleted: bool }
pub async fn locate(conn: &mut PgConnection, id: Uuid) -> Result<Option<Located>, AppError>;

/// The trait M1's registry holds per kind (`KindRegistration::provider`). Core kinds
/// implement it through their `ThingKindSpec`; M1's built and connected kinds implement it
/// over `things` rows they write with `upsert`.
pub trait KindProvider: Send + Sync {
    fn spec(&self) -> &ThingKindSpec;
    /// Display fields beyond the envelope (key, status name, excerpt) for these ids.
    fn resolve<'a>(&'a self, conn: &'a mut PgConnection, ids: &'a [Uuid])
        -> BoxFuture<'a, Result<Vec<ThingDisplay>, AppError>>;
    /// The kind's own fields for the `fields` map of the `Thing` contract.
    fn fields<'a>(&'a self, conn: &'a mut PgConnection, ids: &'a [Uuid])
        -> BoxFuture<'a, Result<HashMap<Uuid, serde_json::Value>, AppError>>;
}
/// For kinds with no table of their own (M1's connected modules, "thing_upsert"): writes
/// the `things` row directly, in the caller's transaction. Core kinds never call it (their
/// triggers keep `things` current).
pub async fn upsert(tx: &mut Transaction<'_, Postgres>, row: ThingRow) -> Result<(), AppError>;
```

`get` no longer needs the kind: `GET /api/v1/things/{kind}/{id}` keeps its path and also
accepts `GET /api/v1/things/{id}`; `thing_open` accepts an id alone. The `Thing` contract
(`modules/contract.rs:283-302`) gains `spaceId`, `parentId`, `version`, `updatedAt` and the
`fields` map that `F-content-modules.md` F3 found missing.

**Addresses.** A thing's address stays its manifest `href` (W1 adds the workspace prefix). A
new short form `thing:<uuid>` is accepted wherever a thing is named (tools, link writes, the
composer), so agents never build URLs. People-facing keys stay per kind: tasks keep
`KEY-12` and aliases; `things::resolve_key(workspace, "AWD-12")` is the one function that turns
a key into an id, and a kind may register a key resolver (records could get `<collection>-12`
later; not designed here).

### 3.2 The link store

```sql
-- migrations/<ts>_links.sql
CREATE TABLE links (
    id UUID PRIMARY KEY,
    workspace_id UUID NOT NULL REFERENCES organizations(id),
    from_thing_id UUID NOT NULL REFERENCES things(id) ON DELETE CASCADE,
    to_thing_id   UUID NOT NULL REFERENCES things(id) ON DELETE CASCADE,
    kind TEXT NOT NULL CHECK (kind IN (
        'blocks', 'about', 'made_from', 'relates_to', 'duplicates', 'mentions', 'embeds'
    )),
    -- Why it exists. 'explicit': someone linked them. 'text': found in the text of
    -- source_*. 'field': a link field's value. 'derived': mirrored from a kind's own
    -- column (made from, pull request links) during and after migration.
    origin TEXT NOT NULL CHECK (origin IN ('explicit', 'text', 'field', 'derived')),
    -- For 'text': the message, page, task description or comment holding the URL.
    -- Not a foreign key: task comments aren't things until C1.
    source_kind TEXT, source_id UUID,
    -- For 'field': the field definition.
    field_id UUID,
    created_by_account_id UUID REFERENCES actors(id),
    via_credential_id UUID,             -- K1's account_credentials(id); before K1.22 the
                                        -- mcp_oauth_grants(id), mapped by K1's acting_via view
    requested_by_account_id UUID REFERENCES actors(id),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (from_thing_id <> to_thing_id),
    CHECK ((origin = 'text') = (source_id IS NOT NULL)),
    CHECK ((origin = 'field') = (field_id IS NOT NULL))
);
-- One explicit or derived link of a kind between two things.
CREATE UNIQUE INDEX links_explicit_unique ON links (from_thing_id, kind, to_thing_id)
    WHERE origin IN ('explicit', 'derived');
-- Symmetric kinds once per pair, whichever way it was made (as work_item_relations does).
CREATE UNIQUE INDEX links_symmetric_unique
    ON links (kind, LEAST(from_thing_id, to_thing_id), GREATEST(from_thing_id, to_thing_id))
    WHERE kind = 'relates_to' AND origin = 'explicit';
CREATE UNIQUE INDEX links_one_original ON links (from_thing_id)
    WHERE kind = 'duplicates';
-- One row per text source and target, one per field value.
CREATE UNIQUE INDEX links_text_unique ON links (source_kind, source_id, kind, to_thing_id)
    WHERE origin = 'text';
CREATE UNIQUE INDEX links_field_unique ON links (from_thing_id, field_id, to_thing_id)
    WHERE origin = 'field';
CREATE INDEX links_from ON links (from_thing_id, kind);
CREATE INDEX links_to ON links (to_thing_id, kind);
CREATE INDEX links_source ON links (source_kind, source_id) WHERE source_id IS NOT NULL;
```

Kinds and how each old store maps:

| Link kind | Reads (from / to) | From today's stores |
| --- | --- | --- |
| `part_of` | not stored; from `things.parent_id` | #1, records in collections, threads in channels |
| `blocks` | "Blocks" / "Blocked by" | #2 |
| `relates_to` | "Related" both ways | #3 `relates_to` |
| `duplicates` | "Duplicates" / "Duplicated by" | #3 `duplicates` |
| `about` | "About" / "Discussed in" | #4 page, decision, conversation; #5; #6; #12 |
| `made_from` | "Made from" / "Led to" | #4 `message`; #13 |
| `mentions` | "Mentions" / "Referenced in" | #7, #8, #9 (non-embed URLs) |
| `embeds` | "Embeds" / "Embedded in" | #14 |
| link fields | "<field name>" / "Linked from <field name>" | #10, #11, as `origin = 'field'` with the kind from the field's config (default `relates_to`) |

**Confirmed for other plans** (the content review's questions, and ruling R27). A milestone is
a thing kind (`milestone`, K3.11) whose `parent_id` may be an initiative record (T1.26):
`parent_id` takes any thing in the same workspace, the kind's spec says which parent kinds it
accepts, and `milestone` accepts `record`. `embeds` may link a conversation to a thing: a
channel tab (C1.26) is a C1 table row plus an `embeds` link from the conversation to the
thing, and the closed list of link kinds above does not grow. A1 records a run's result as a
`made_from` link from the result to the run (no `produced` kind). The history slice C1.34 and
T1.41 depend on is K3.19 (`history_list`, `history_show`, `history_restore`); `activity_list`
is K3.20's.

**Direction.** A link always reads `from` → `to` as written; the reader returns both directions
and labels each with `LinkKind::words()` (`contract.rs:94-108`). For a text link, `from` is the
thing that holds the text (the message itself once messages are things; the task for a task
comment until C1).

**Account mentions are not links.** #15 names an account, not a thing, and drives
notifications and wake-ups. They stay in their projection tables, which C1 folds into one
mention table when comments become messages. (See §9.)

**Who created it.** `created_by_account_id`, `via_credential_id`, `requested_by_account_id`
from the operation context (§3.3.2). Text and field links carry the author of the text or the value.

**Reading backlinks with both ends' access respected.**

```rust
// apps/server/src/links.rs
pub struct LinkView { pub kind: LinkKind, pub direction: LinkDirection, pub label: String,
                      pub origin: LinkOrigin, pub other: Thing, pub via: Option<Thing> }
/// A thing's links both ways. The caller has checked the viewer sees `thing`.
/// One query: links where from = id or to = id, plus parent/children from
/// `things`; the other ends go through `things::visible_ids` (K1's filter later)
/// and module switches for the other end's own space; capped at MAX_LINKS per
/// kind with a `more` count.
pub async fn for_thing(conn: &mut PgConnection, viewer: &Viewer<'_>, thing: &Thing)
    -> Result<Vec<LinkView>, AppError>;
```

Rules: a link shows only if the viewer can see both ends now. For a text link whose source is
a message, the other end shown is the message's conversation (`via` = the message), so
"Referenced in #design" opens at the message; if the viewer can't see the conversation the
link doesn't exist for them (today's behaviour, `20261113060000:10-13`). A link's existence is
never revealed by counts either: `more` counts only visible rows.

**Writes.**

```rust
pub struct NewLink { pub from: Uuid, pub to: Uuid, pub kind: LinkKind }
/// Operation `link_create` (K2): needs "edit" on `from` and "see" on `to`;
/// refuses a cycle for `blocks` (today's check in work_item_planning.rs moves here);
/// emits `link.created` on the `from` thing.
pub async fn create(tx: &mut Transaction<'_, Postgres>, ctx: &OpContext, link: NewLink) -> Result<LinkOutcome, AppError>;
pub async fn delete(tx: &mut Transaction<'_, Postgres>, ctx: &OpContext, link_id: Uuid) -> Result<(), AppError>;
/// Replace one text source's links (called by the trigger function or by Rust).
pub async fn replace_text_links(tx, source_kind: &str, source_id: Uuid, from: Uuid, body: &str) -> Result<(), AppError>;
/// Replace one field's links on a thing (collections::records::write, T1's task fields).
pub async fn sync_field(tx, from: Uuid, field_id: Uuid, kind: LinkKind, targets: &[Uuid]) -> Result<(), AppError>;
```

**Finding links in text without per-route regexes.** Because every thing now has a registry
row, the extractor looks for UUIDs inside URL-shaped text and asks `things` what they are:

```sql
CREATE FUNCTION text_link_targets(p_workspace UUID, body TEXT)
RETURNS TABLE (target_id UUID, kind TEXT, embed BOOLEAN) LANGUAGE sql STABLE AS $$
  WITH urls AS (       -- each URL (absolute or starting with '/'), and whether it's an embed
    SELECT m[1] AS url, m[2] IS NOT NULL AS embed
    FROM regexp_matches(COALESCE(body, ''),
      '((?:https?://[^\s)"]+)?/[^\s)"]*[0-9a-fA-F]{8}-[0-9a-fA-F-]{27}[^\s)"]*)(\s+"embed")?', 'g') m
    LIMIT 400),
  last_id AS (         -- the deepest id in a URL is the thing (…/collections/C?record=R -> R)
    SELECT (regexp_match(url, '.*([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})'))[1]::uuid AS id, embed
    FROM urls)
  SELECT DISTINCT thing.id, CASE WHEN last_id.embed THEN 'embeds' ELSE 'mentions' END, last_id.embed
  FROM last_id JOIN things thing ON thing.id = last_id.id
  WHERE thing.workspace_id = p_workspace AND thing.deleted_at IS NULL
  LIMIT 100
$$;
```

This removes the URL regex from migrations (`F` F2, `G` F6): a route rename, W1's
`/w/<workspace>/` prefix or a new kind needs no migration, and sheets, runs and files become
linkable the day they have registry rows. Only ids in the same workspace count. A URL to
another server that happens to contain one of our ids would count; the workspace check makes
that harmless. Today's `thing_reference_targets` maps `/messages/{parent}?thread={id}` to the
thread; "deepest id wins" gives the same answer.

New text sources join the same trigger pattern: whiteboard `text_content`, record title and
long-text values (from the record write path), sheet cell inputs (from `sheets::apply`, capped),
page comments and suggestions (until C1 makes them messages).

### 3.3 The event log

#### 3.3.1 Tables

The log has two tables: writers insert into a small staging table inside their own
transaction; a **sequencer** moves committed rows into the append-only log, giving each a
`position` in the order it moves them. This is how readers never skip an event: positions are
assigned only to rows that are already committed, by one transaction at a time, so a reader at
`position > n` can never later see a row with a smaller position appear. The classic
alternative, an identity column at insert time, leaves gaps that fill late (transaction A takes
5, B takes 6 and commits first, a reader moves to 6, A commits 5 and is skipped). The other
known fix, ordering by transaction id below `pg_snapshot_xmin`, breaks after a dump and
restore into a fresh cluster (transaction ids restart lower than stored ones), and self-hosters
restore from dumps; a counter row is dumped and restored with the data.

```sql
-- migrations/<ts>_event_log.sql
CREATE TABLE event_staging (
    id UUID PRIMARY KEY,                -- the event id, kept into the log
    staged_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    ordinal INTEGER NOT NULL,           -- order within its transaction (events::emit counts)
    body JSONB NOT NULL                 -- the row below, minus position
);
CREATE INDEX event_staging_order ON event_staging (staged_at, ordinal);

CREATE TABLE event_position_counter (singleton BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
                                     next_position BIGINT NOT NULL);
INSERT INTO event_position_counter VALUES (true, 1);

CREATE TABLE events (
    position BIGINT NOT NULL,
    id UUID NOT NULL,
    workspace_id UUID NOT NULL,
    space_id UUID,
    thing_id UUID,                      -- what it's about (NULL for workspace-level events)
    thing_kind TEXT,
    access_thing_id UUID,               -- whose visibility decides who may read it
    thing_version BIGINT,               -- the thing's version after this change (W1, A2)
    type TEXT NOT NULL CHECK (type ~ '^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)+$'),
    class TEXT NOT NULL CHECK (class IN ('fact', 'signal')),
    changes TEXT[] NOT NULL DEFAULT '{}' -- keys of what changed ("status", "title", a field id);
        CHECK (cardinality(changes) <= 64), --   never values or text (W1's precise updates)
    -- Who may hear it live and through delivery (K5 routes on these columns with no query;
    -- A2 filters per connection). Exactly one audience per event:
    --   workspace:    every member of workspace_id (workspace.updated, workspace.member.*)
    --   space:        readers of space_id
    --   conversation: readers of the root conversation; restricted = members only
    --   account:      audience_account_id only (read state, its notifications, a wake-up)
    --   thing:        whoever may see access_thing_id (else thing_id) now; checked per event
    audience TEXT NOT NULL CHECK (audience IN ('workspace', 'space', 'conversation', 'account', 'thing')),
    conversation_id UUID,               -- the root conversation, for audience 'conversation'
    conversation_restricted BOOLEAN,    -- true: a DM, group, private channel or a thread in one
    audience_account_id UUID,           -- set for events only one account may read
    actor_account_id UUID,              -- who did it (NULL = the system)
    via_credential_id UUID,             -- the credential (and so the app) it came through: K1's
                                        -- account_credentials(id); the OAuth grant id until K1.22
    requested_by_account_id UUID,       -- who asked for it, when someone else acted
    run_id UUID,                        -- the run it happened in, when an agent was working
    request_id TEXT CHECK (request_id IS NULL OR char_length(request_id) <= 128),
    legacy_type TEXT,                   -- the outbox name, for the web until W1 moves
    payload JSONB NOT NULL CHECK (jsonb_typeof(payload) = 'object' AND octet_length(payload::text) <= 8192),
    created_at TIMESTAMPTZ NOT NULL,    -- when the change committed's transaction staged it
    CHECK ((audience = 'space') <= (space_id IS NOT NULL)),
    CHECK ((audience = 'conversation') = (conversation_id IS NOT NULL AND conversation_restricted IS NOT NULL)),
    CHECK ((audience = 'account') = (audience_account_id IS NOT NULL)),
    CHECK ((audience = 'thing') <= (COALESCE(access_thing_id, thing_id) IS NOT NULL)),
    PRIMARY KEY (position)
) PARTITION BY RANGE (position);
CREATE TABLE events_p0 PARTITION OF events FOR VALUES FROM (1) TO (10000001);
CREATE TABLE events_default PARTITION OF events DEFAULT;
CREATE INDEX events_workspace ON events (workspace_id, position);
CREATE INDEX events_space ON events (space_id, position) WHERE space_id IS NOT NULL;
CREATE INDEX events_thing ON events (thing_id, position) WHERE thing_id IS NOT NULL;
CREATE INDEX events_audience ON events (audience_account_id, position) WHERE audience_account_id IS NOT NULL;
CREATE INDEX events_id ON events (id);
CREATE INDEX events_actor ON events (actor_account_id, position) WHERE actor_account_id IS NOT NULL;
CREATE INDEX events_conversation ON events (conversation_id, position) WHERE conversation_id IS NOT NULL;
-- An immutability trigger refuses UPDATE on events (only retention deletes).
-- events_workspace (workspace_id, position) is the index A2's per-connection cursor scans use.

-- The wake (K5 designs it; created here so the first slice that stages events has it).
-- Channel names carry the schema so parallel test schemas don't wake each other, and stay
-- under PostgreSQL's 63-byte limit.
CREATE FUNCTION realtime_channel(suffix TEXT) RETURNS TEXT LANGUAGE sql STABLE AS $$
    SELECT 'aw_' || left(md5(current_schema()), 12) || '_' || suffix
$$;
CREATE FUNCTION wake_event_log() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    PERFORM pg_notify(realtime_channel('log'), TG_TABLE_NAME);   -- delivered at commit
    RETURN NULL;
END $$;
CREATE TRIGGER event_staging_wake AFTER INSERT ON event_staging
    FOR EACH STATEMENT EXECUTE FUNCTION wake_event_log();
```

**One design with K5 (ruling R8).** This section is the only definition of the log's tables,
envelope and sequencer; K5's plan points here. K5 owns the wake listener, the per-instance
tail, fan-out and the switch of sockets onto the log (K5.4, K5.5). K5's earlier design
(stamping `outbox_events.position` under an `event_log_heads` row) is dropped: the outbox
is mirrored into this log from K3.2, so the tail reads `events` from its first day.

**Sequencer** (`apps/server/src/events/sequencer.rs`). One transaction:

```sql
WITH batch AS (
  SELECT id, staged_at, ordinal, body FROM event_staging
  ORDER BY staged_at, ordinal LIMIT 1000 FOR UPDATE SKIP LOCKED),
counter AS (
  UPDATE event_position_counter SET next_position = next_position + (SELECT count(*) FROM batch)
  RETURNING next_position - (SELECT count(*) FROM batch) AS base),
moved AS (
  INSERT INTO events (position, id, …)
  SELECT counter.base + row_number() OVER (ORDER BY batch.staged_at, batch.ordinal) - 1, batch.id, …
  FROM batch, counter RETURNING position)
DELETE FROM event_staging USING batch WHERE event_staging.id = batch.id;
SELECT pg_notify(realtime_channel('events'), max_position::text);
```

The counter row's lock serializes sequencer transactions across processes, so any process may
run it and no lease is needed: a cheap pre-check (`SELECT 1 FROM event_staging LIMIT 1`) skips
empty rounds, and the transaction sets `SET LOCAL lock_timeout = '2s'` so a contending process
gives up and tries again on its next round. The `event_staging_wake` trigger (above) wakes it at
commit, so writers need no notify call of their own; it also polls every 250 ms. Ruling R24
rule 3: NOTIFY is only an optimisation; a process whose connection can't LISTEN (a transaction
pooler) runs the same loop on the poll alone, and the tail (K5) polls `events` position the
same way. Nothing here takes a session advisory lock or a session setting. Events from one transaction keep their order
(`ordinal`) and are contiguous unless a batch boundary splits them, which is harmless. Cost:
one staging insert and delete per event plus one log insert; the log is append-only, which
keeps it vacuum-friendly and lets an immutability trigger hold.

**Partitions and retention.** Range partitions of 10 million positions; the `events.partitions`
job (on `background_jobs`) creates the next one at 80 % and moves anything that landed in the
default partition. Retention is per class: `signal` rows (page saves while typing, board scene
counters, read marks, `work_item.updated` placement) are deleted after 7 days; `fact` rows are
kept for the workspace's event retention (default 400 days, a plan setting H1 may change); a
partition whose newest row is older than the longest retention is detached and dropped.
History tables keep "full history" (END-STATE §6) independently of the log.

**Cold data (ruling R24 rule 7).** Before a partition is dropped, the `events.archive` job
writes its `fact` rows to object storage through K5's storage seam as compressed, position-
ordered files (one per partition, with an index of `(workspace_id, first, last position)`),
and records them in `event_archives (partition, workspace_ids, object_key, first_position,
last_position, rows)`. The audit trail and activity feeds read the archive when a page asks
for positions older than the oldest live partition, so they get slower, never shorter; a
workspace export includes its archived rows. History payloads follow the same rule through
the `History` seam (§3.4): a kind may move versions older than a year into a snapshot plus
object-stored deltas, and `list`/`show` read them back; no adapter may delete a version. C1
does this for pages and sheets, and C1's document update logs compact into snapshots the
same way. Signal rows are not archived (they are not history).

#### 3.3.2 Writing an event in the change's transaction

```rust
// apps/server/src/events/mod.rs
pub struct EventContext {            // set once per operation; K2's runner sets it
    pub actor: Option<Uuid>, pub via_credential: Option<Uuid>,
    pub requested_by: Option<Uuid>, pub run: Option<Uuid>, pub request_id: Option<String>,
}
/// Stores the context in transaction-local settings (`SET LOCAL` / `set_config(..., true)`
/// only, ruling R24 rule 3: aw.actor, aw.via_grant — the existing one —, aw.requested_by,
/// aw.run, aw.request_id), so shared command code and SQL triggers read it without new
/// arguments.
pub async fn set_context(tx: &mut Transaction<'_, Postgres>, ctx: &EventContext) -> Result<(), AppError>;

/// Who may hear an event (ruling R8). The columns of `events` follow from it.
pub enum Audience {
    Workspace,                                   // every member who can see the workspace
    Space,                                       // members of `space_id`
    Conversation { root: Uuid, restricted: bool }, // a conversation's readers; restricted = private
    Account(Uuid),                               // one account (notifications, reply.requested)
    Thing,                                       // whoever can see `access_thing` (else `thing`)
}

pub struct NewEvent<'a> {
    pub ty: &'static EventType,          // from the catalogue, never a free string
    pub workspace_id: Uuid, pub space_id: Option<Uuid>,
    pub thing: Option<(Uuid, &'a str)>, pub access_thing: Option<Uuid>,
    pub version: Option<i64>,            // the thing's new version, when the event changes one
    pub changes: &'a [&'a str],          // field keys changed; at most 64
    pub audience: Audience,
    pub payload: serde_json::Value,      // validated against ty's schema and size
}
impl<'a> NewEvent<'a> {
    /// The common case: a fact about one thing, heard by whoever can see it.
    pub fn thing(ty: &'static EventType, kind: &'a str, id: Uuid) -> Self;
}
pub async fn emit(tx: &mut Transaction<'_, Postgres>, event: NewEvent<'_>) -> Result<Uuid, AppError>;

/// Unfiltered read by position, for the live tail only (K5.4). Never exposed to a caller:
/// every reader that answers an account uses `read` (§3.3.4), which re-checks access.
pub async fn tail(conn: &mut PgConnection, after: i64, limit: i64) -> Result<Vec<LoggedEvent>, AppError>;
```

K2's runner calls `set_context` at the start of each operation's transaction and `emit` at its
event step; code that is not yet an operation calls both itself. `LoggedEvent` (a row of
`events` with the envelope fields above) is this module's type; K5 and A2 use it as is.

**Direct scope and connections across workspaces (rulings R19, R20).** DMs and groups and what
hangs off them carry the direct scope's id (K4's reserved `organizations` row, `kind =
'direct'`) as `workspace_id`, so their events go through this one log, envelope and sequencer
with no second path; their audience is always `Conversation { root, restricted: true }` (or
`Account` for a participant's notification), never `Workspace` or `Space`, and a CHECK refuses
`audience IN ('workspace','space')` when `workspace_id` is the direct scope's id. `read`
answers a direct-scope event only to a participant (`conversation_member_visible`); no role,
admin or owner passes. A connection that reaches several workspaces (R19) reads one merged
stream: `position` is global to the server, so one cursor walks every workspace it reaches
plus the direct scope, filtered by `workspace_id = ANY(reached)` and access; every event names
its workspace. A2's subscription stores the set it reaches (or "every workspace the account is
in") and whether it includes DMs.

`emit` reads the context settings, validates the payload against the type's schema (in debug
and test builds) and inserts one `event_staging` row. During the transition, `outbox::enqueue`
(`outbox.rs:736`) also calls `emit` with the mapped type (`legacy_type` = the outbox name), so
every outbox writer is in the log from the first slice without touching 40 call sites.

**Payloads carry ids, keys of what changed, and small non-text diffs** (a status key before
and after, an assignee id, a field id). Never message, page or cell text, and never secrets,
because the log is kept for months and a removed message's text must vanish everywhere
(`outbox.rs:116-119` states the same rule). Consumers that need content read it live with the
reader's access, as webhooks already do (`webhooks.rs:1468-1632`).

#### 3.3.3 The typed catalogue and registration per module

```rust
pub struct EventType {
    pub name: &'static str,             // "page.updated"
    pub module: &'static str,           // must list it in its manifest's events.emits
    pub thing_kind: Option<&'static str>,
    pub class: EventClass,              // Fact | Signal
    pub summary: &'static str,          // "A page changed" (webhook and builder copy)
    pub audited: bool,                  // shows in the audit trail
    pub automatable: bool,              // can be a rule's trigger
    pub payload: fn() -> schemars::Schema,
    pub legacy_names: &'static [&'static str],  // outbox / external-event aliases
}
// Each module: `pub const EVENTS: &[EventType] = &[...]`; events::catalog() concatenates them.
```

Starting catalogue (facts unless marked *signal*; audience events marked *aud*):

| Module | Events |
| --- | --- |
| messages / channels | `conversation.created`, `conversation.updated`, `conversation.archived`, `conversation.member.added`, `conversation.member.removed`, `conversation.visibility.changed`, `message.created` (payload: conversation, sequence, author, mentioned account ids, thread id), `message.edited`, `message.removed`, `message.reactions_changed`, `conversation.read` *signal aud*, `thread.status_changed` |
| replies (A2 consumes) | `reply.requested` *aud*, `reply.followup` *aud*, `reply.cancelled` *aud*, `reply.claimed`, `reply.answered` |
| tasks | `task.created`, `task.updated` (changed field keys, status before/after keys), `task.assigned` *aud for the new assignee*, `task.archived`, `task.restored`, `task.moved`, `task.comment_added`, `task.claimed`, `task.released`, `task.placement_changed` *signal* (today's `work_item.updated` placement), `milestone.created/updated/archived`, `label.created/updated/deleted`, `view.saved` |
| pages | `page.created`, `page.updated` (one per stored revision, not per keystroke), `page.content_saved` *signal* (each live save), `page.archived`, `page.restored`, `page.version_restored`, `page.commented`, `page.suggestion_made`, `page.suggestion_decided`, `decision.recorded`, `decision.superseded` |
| sheets | `sheet.created`, `sheet.updated` (revision, tab ids, changed range count), `sheet.archived`, `sheet.restored`, `sheet.imported` |
| whiteboards | `whiteboard.created`, `whiteboard.updated` (one per saved revision), `whiteboard.scene_changed` *signal*, `whiteboard.deleted`, `whiteboard.restored`, `whiteboard.version_restored`, `whiteboard.duplicated` |
| collections | `collection.created`, `collection.updated`, `record.created`, `record.updated`, `record.archived`, `record.restored`, `field.created/updated/archived` |
| runs and hosts (A1 owns them) | `run.updated`, `run.finished`, `run.start` *aud* (to the host that runs it), `run.cancel` *aud*, plus A1's `request.opened` (one per addressed account, *aud*; was `request.created`), `request.answered`, `host.notice`, `host.command` *aud* (to the host's app account) |
| comments (C1) | `comment.mentioned` *aud* (one per mentioned account) |
| access (K1 emits) | `role.created`, `role.updated`, `role.deleted`, `account.disabled`, `account.enabled`, `agent.created`, `group.created`, `group.updated`, `group.member.added`, `group.member.removed`, `credential.created`, `credential.revoked` |
| code | `pull_request.checks_failed`, `pull_request.checks_passed`, `pull_request.merged`, `pull_request.closed`, `pull_request.linked` |
| workspace | `workspace.updated`, `workspace.member.added`, `workspace.member.removed`, `workspace.member.role_changed` (K1 emits these), `space.created`, `space.updated`, `space.archived`, `space.restored`, `space.member.added`, `space.member.removed`, `space.member.role_changed` (together today's `project.membership.updated`), `file.attached`, `file.removed`, `notification.changed` *signal aud*, `later.due` *aud* (today `reminder.due`), `link.created`, `link.deleted` |

Legacy names map 1:1 for the web's realtime envelope (`conversation.message.created`,
`work_item.updated`, ...) and for `external_event_poll` (`work_item.changed`,
`message.mentioned`, `agent_run.terminal`) until W1 and A2 retire them.

#### 3.3.4 Consumers and cursors

```sql
CREATE TABLE event_consumers (
    name TEXT PRIMARY KEY,              -- 'notifications', 'router', 'search', 'activity', …
    position BIGINT NOT NULL,           -- last position fully handled
    lease_owner TEXT, lease_expires_at TIMESTAMPTZ,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE event_consumer_failures (  -- parked events: the consumer moved past them
    consumer TEXT NOT NULL, event_position BIGINT NOT NULL, attempts INTEGER NOT NULL,
    last_error TEXT NOT NULL CHECK (char_length(last_error) <= 500),
    next_attempt_at TIMESTAMPTZ NOT NULL, resolved_at TIMESTAMPTZ,
    PRIMARY KEY (consumer, event_position)
);
```

```rust
#[async_trait]
pub trait Consumer: Send + Sync {
    const NAME: &'static str;
    /// The types it wants (None = all); the reader filters in SQL.
    fn types(&self) -> Option<&'static [&'static str]>;
    /// Handle a batch inside `tx`. Durable effects (rows, jobs) go in `tx`; the
    /// runtime advances the cursor in the same transaction, so effects and cursor
    /// commit together: at least once, and exactly once for database effects.
    async fn handle(&self, tx: &mut Transaction<'_, Postgres>, batch: &[LoggedEvent]) -> Result<(), ConsumerError>;
}
pub async fn run_consumer<C: Consumer>(state: AppState, consumer: C, shutdown: watch::Receiver<bool>);
pub async fn read(conn: &mut PgConnection, filter: &EventFilter, after: i64, limit: i64) -> Result<Vec<LoggedEvent>, AppError>;
```

The runtime takes the consumer's lease (one runner per consumer at a time, any process; until
K5.2 lands, each batch transaction instead locks its own `event_consumers` row with `SELECT ...
FOR UPDATE SKIP LOCKED`, which is pooler-safe, and never a session advisory lock, ruling R24
rule 3), reads
`position > cursor` for up to 500 rows, calls `handle`, and advances. If `handle` fails, the
transaction rolls back and the batch is retried with backoff; after three batch failures it
retries event by event, and an event that fails five times is written to
`event_consumer_failures` and passed, so one bad event never stops a consumer for long and is
never forgotten (a sweep retries parked events; operators see them on the status page). Outside
effects (webhook POSTs, pushes, emails) are never done in `handle`; it enqueues a job keyed by
the event id, as notifications and routing already dedupe (`notifications.rs:1-10`,
`events.rs:221`). This fixes `G` F2 by construction.

| Consumer | Owner | Cursor | What it does |
| --- | --- | --- | --- |
| `notifications` | K3 moves it, C2 owns the rules | global row | today's `notifications::fan_out`, unchanged rules |
| `router` | K3 | global row | today's `events::route`: webhooks `fan_out` and automations `on_event`, now for every automatable type |
| `search` | K3 | global row | re-indexes the thing an event names (§3.5) |
| `activity` | K3 | none: activity feeds read the log directly | |
| `audit` | K3 | none: the audit trail reads the log directly | |
| realtime fan-out | K5 | per process, in memory, from the `realtime_channel('events')` NOTIFY or a poll of the newest position | every `live` process tails the log with `events::tail` and offers events to its sockets; K5 owns subscriptions per socket |
| agent delivery | A2 | one per connection, in A2's `event_subscriptions` | A2 reads `events::read` with the subscription's filter and the agent's access, then webhook / SSE / poll |
| webhooks per subscription | A2 may fold in | today per-delivery rows | unchanged in K3 |

Global consumers process all workspaces in one stream. If one workspace's work ever slows the
rest, a consumer can be sharded by `hashtext(workspace_id) % n` into `name#0..n-1` rows without
changing the API (not needed now; noted for K5 and H1).

Who may read an event (A2, activity, audit, the stream): the reader sees an event if it is not
an audience event for someone else, and `things::visible_ids` passes for `access_thing_id`
(falling back to `thing_id`, then to space membership for space-level events), checked at read
time with current access. The thing's tombstone row keeps this working after deletion.

#### 3.3.5 Relation to today's outbox

The log **becomes** the outbox; it doesn't sit beside it for good. Steps (§4.3): the outbox
mirrors into the log; consumers move one at a time from outbox claims to log cursors;
realtime (with K5) reads the log; then `outbox::enqueue` writes only the log and
`outbox_events` is dropped. `OutboxEventPayload` stays as the typed payload builder for the
legacy types, so call sites don't change until they are rewritten by their owners.

### 3.4 History as a seam

Every kind keeps its own history table (the merge rules differ for good reasons, `F` F11);
K3 defines the common reading and restoring interface, the guard, and the event link. C1 fills
depth (sheet snapshots, record restore, task field revert).

```rust
// apps/server/src/history.rs
pub struct HistoryCaps { pub list: bool, pub show_version: bool, pub restore: bool, pub diff: bool }
pub struct HistoryEntry {
    pub thing_id: Uuid, pub version: i64, pub at: DateTime<Utc>,
    pub actor: Option<AccountRef>, pub via_credential: Option<AppRef>, pub requested_by: Option<AccountRef>,
    pub summary: String,                 // "Changed status to Done", "Edited 3 cells"
    pub changed: Vec<String>,            // field keys
    pub event_id: Option<Uuid>,          // the log entry written with it, when known
}
#[async_trait]
pub trait History {
    async fn list(&self, conn: &mut PgConnection, thing: &Thing, before: Option<i64>, limit: i64) -> Result<Vec<HistoryEntry>, AppError>;
    async fn show(&self, conn: &mut PgConnection, thing: &Thing, version: i64) -> Result<Option<serde_json::Value>, AppError>;
    /// A new version equal to `version`; refuses when `expected_version` is stale.
    async fn restore(&self, tx: &mut Transaction<'_, Postgres>, ctx: &OpContext, thing: &Thing,
                     version: i64, expected_version: i64) -> Result<i64, AppError>;
}
```

Operations (K2 generates route and tool): `history_list` (see the thing), `history_show` (see
the thing), `history_restore` (edit the thing; propose when the role only suggests, which K1
and C1 decide). Adapters at the start:

| Kind | list | show | restore | Source |
| --- | --- | --- | --- | --- |
| page | yes | yes | yes | `memory.rs:406-500, 680` |
| whiteboard | yes | yes | yes | `whiteboards/store.rs:247, 356` |
| record | yes | no | Not("C1 adds revert per field") | `collections/records.rs:401` |
| task | yes | no | Not("C1 adds field revert; history converter is T1's") | `work_item_history.rs` |
| sheet | yes | no | Not("sheet_revisions keeps only changed inputs; C1 adds snapshots") | `sheet_revisions` |
| message | yes | yes | Not("an edit is the restore") | `message_revisions` |
| conversation, decision, file, run, collection | Not(reason) until their owners add one | | | |

### 3.5 Search over every kind

**Decision: one index table filled from the log.** The alternative, a per-kind SQL arm per
search kind (today's `search.rs`), is synchronous and already works, but it needs a new arm,
column and access rule per kind, ranks across seven queries by hand, and gives meaning-based
search nowhere to plug in. The index costs seconds of lag after a write and a second copy of
searchable text, which is why removal and loss of access are handled synchronously (below).

```sql
CREATE TABLE search_documents (
    thing_id UUID PRIMARY KEY REFERENCES things(id) ON DELETE CASCADE,
    workspace_id UUID NOT NULL, space_id UUID, kind TEXT NOT NULL,
    access_thing_id UUID,                -- copied from things for the filter
    title TEXT NOT NULL DEFAULT '' CHECK (char_length(title) <= 300),
    body TEXT NOT NULL DEFAULT '' CHECK (char_length(body) <= 100000),
    language TEXT NOT NULL DEFAULT 'english',   -- regconfig name; 'simple' for boards' labels
    vector TSVECTOR NOT NULL,            -- setweight(title,'A') || setweight(body,'B')
    author_account_id UUID, updated_at TIMESTAMPTZ NOT NULL, source_version BIGINT NOT NULL
);
CREATE INDEX search_documents_vector ON search_documents USING GIN (vector);
CREATE INDEX search_documents_scope ON search_documents (workspace_id, kind, updated_at DESC);
-- Later, beside it, not in it: search_embeddings (thing_id, model, chunk, embedding vector)
```

Each kind registers a `SearchSource`:

```rust
pub struct SearchSource {
    /// SQL returning (title, body, language, author, updated_at, version) for $1 ids.
    pub document_sql: &'static str,
    pub weight: f32,                     // today's ranking: decisions 1.6+, pages 1.6, tasks 1.3, …
    pub reindex_on: &'static [&'static str],  // event types that change its document
}
```

The `search` consumer reads events whose type is in any source's `reindex_on`, reloads those
documents, and upserts them (skipping when `source_version` is not newer). Sources for sheets
(`sheet_cells.input` of text cells, plus tab names, capped) and whiteboards (`text_content`)
come with K3.6. Messages are indexed one document per message with `access_thing_id` = the
conversation.

Privacy rules:

- Removing a message, deleting or archiving a thing deletes its search document **in the same
  transaction**: a trigger on `things` deletes the row when `deleted_at` or `archived_at` is set
  (archived things are not searched today, `search.rs:298,324`). The message removal path's
  scrub list (`message_edits.rs:1-14`) gains `search_documents`.
- Every query applies the access filter at read time (K1's SQL filter on `things`, joined by
  `thing_id`; until it lands, the same wrapper `things::visible_ids` over the top 200 matches,
  plus today's pre-filters by project and module). Losing access needs no reindex.

The query keeps `search::search` and the `search` tool's shape, adds `kinds` for every
registered kind, a `spaceId` filter, and makes `SearchHit.projectId` optional with a
`workspaceId` (`search.rs:96`). Meaning-based search plugs in as a second ranker over
`search_embeddings` behind the same filter, owned by C2 with a model the workspace chooses.

### 3.6 Bringing pages, sheets and whiteboards in

- **Events**: `page.*`, `sheet.*`, `whiteboard.*` emitted from the single write path of each
  (`page_docs/store.rs:719` `save` and `memory.rs` page updates; `sheets.rs:863` `apply` and
  `sheets/http.rs`; `whiteboards/store.rs:95` `write_elements`, `:247` `save_revision`, `:356`
  `restore_revision`). `page.updated` and `whiteboard.updated` fire when a new revision row is
  stored, not on every merged live save, so rules and webhooks fire per meaningful edit; the
  per-save signal stays for realtime.
- **Manifests** list them in `events.emits` and add triggers.
- **Automations**: three triggers `page_changed`, `sheet_changed`, `whiteboard_changed`
  (optional page, sheet or board id; optional "created only"), and actions stay the four
  existing ones. The triggers become generic "event type + filter" when K2 makes actions
  operations; K3 does not redesign the builder.
- **Webhooks**: data rendered live like tasks (`webhooks.rs:1468`): `{page: {id, title, url,
  version, editedBy}}`, `{sheet: {id, name, url, revision}}`, `{whiteboard: {id, title, url,
  version}}`; no body text. `webhooks::visible_to` checks the thing through
  `things::visible_ids` instead of a growing list of per-kind columns (`webhooks.rs:998-1052`).
- **Search**: K3.6 (§3.5).
- **Links**: sheets become things and link targets in K3.10; board text and sheet cells become
  text sources in K3.13.

### 3.7 Activity feeds and the audit trail

- `activity_list` operation: `{ thingId? | spaceId? | accountId?, types?, before?, limit }`,
  reads `events` (facts only) through the access rule, returns `{event, actor, via,
  requestedBy, thing}` with titles read live. Web: an "Activity" tab piece any thing can show
  (W1 draws it; registered in the layout registry as `thing.activity`).
- Audit trail: `audit_log.rs` reads `events` where the type is `audited` **together with**
  `audit_events` for what is not a change to a thing: sign-ins, denied and failed attempts,
  credential and settings changes. The UI shows one list ordered by time. New code stops
  writing `audit_events` for changes that emit an audited event; the 26 existing writers move
  as their owners touch them (a guard counts them down).

## 4. Migration path

### 4.1 Registry

1. Migration creates `thing_kinds` (seeded), `things`, and an `AFTER INSERT OR UPDATE OF <envelope
   columns> OR DELETE` trigger per kind table: `work_items`, `project_pages`,
   `project_decisions`, `whiteboards`, `sheets`, `collections`, `collection_records`,
   `conversations`, `projects`; later `messages`, `files`, `runs` and `requests` (A1), `project_milestones`,
   `github_pull_requests`, `repositories`, `automation_rules`. Each upserts its row by id.
   Tasks as records: the `collection_records` trigger inserts with `ON CONFLICT (id) DO
   NOTHING`; the `work_items` trigger inserts with `ON CONFLICT (id) DO UPDATE SET kind =
   'task', …`, so a task's record never shows as kind `record`, whichever row is written first.
   T1 confirms the marker once #251's successors land.
2. Backfill runs as a `things.backfill` job per kind in id order, 5,000 rows per batch, `INSERT
   … ON CONFLICT DO NOTHING`, remembering its last id in the job's payload so it resumes after
   a restart. The triggers exist first, so rows written during the backfill are already there.
   A `things_backfill_done(kind)` row marks completion; readers of a kind switch only when it's
   done (checked at startup, cached).
3. Readers move: `things::get`/`resolve` read the envelope from `things` and join the kind's
   table only for display fields.
4. Nothing to delete: the registry is additive.

### 4.2 Links

1. **Copy.** Migration creates `links` and backfills from stores #2 to #14 (SQL `INSERT …
   SELECT … ON CONFLICT DO NOTHING`, one statement per store; the stores are small, so it runs
   in the migration). Rows whose ends have no `things` row yet are skipped and picked up by
   a `links.backfill` job after the registry backfill completes.
2. **Dual-write by triggers.** `AFTER INSERT OR DELETE` triggers on each old store mirror into
   `links`, and the `thing_references` trigger functions call the new `text_link_targets` and
   write both tables. Every write path keeps working unchanged. These triggers merge in any
   order with other branches because they touch only their own table.
3. **Switch reads.** `things::links` → `links::for_thing`, one query. An integration test
   builds every kind of link through today's tools and asserts the old union and the new
   reader return the same set (plus the three holes now filled). The old `links_sql` stays one
   release behind a constant for comparison, then goes.
4. **Switch writes.** `task_link`, `task_relate`, `task_depend`, `whiteboard_link`, the web
   routes and `work_items.rs:2447` call `links::create`/`delete`; the mirror triggers are
   reversed (links → old table) for the stores that still have readers (dependencies: 27
   references in 11 files; relations: 10 in 5).
5. **Move the remaining readers** (`ai/ready.rs`, `usage/ready.rs`, `work_items/views.rs`,
   `work_item_planning.rs`, `launch.rs`, `mcp/tasks.rs`, `events.rs` pull request lookups) to
   `links`.
6. **Contract** (K3.23): drop `work_item_links`, `work_item_conversation_links`,
   `whiteboard_links`, `thing_references`, `message_work_item_references`,
   `work_item_comment_references`, `work_item_dependencies`, `work_item_relations` after one
   release with no readers (a grep test proves it). The pull request link tables stay as the
   code module's detail rows (they carry provenance) and keep mirroring into `links`. The
   "made from" columns stay as the kind's own facts and keep mirroring.

### 4.3 Events

1. **Log in parallel.** K3.2 adds the tables and makes `outbox::enqueue` also `emit`. Nothing
   reads the log yet; the outbox keeps working.
2. **Sequencer and cursors** (K3.3). Consumers start at the position of the first mirrored
   event; nothing is processed twice because each consumer's own dedupe keys
   (`events.route:<id>`, notification dedupe keys) are by event id and the mirrored event keeps
   the outbox `event_id` as its `id`.
3. **Router** moves (K3.4): the publisher stops calling `queue_event_route`; the `router`
   consumer starts at the log position matching the last routed outbox event (found by the
   first unrouted outbox event's id). Then write-time domain events (K3.7) replace `expand`.
4. **Notifications** move (K3.9) the same way.
5. **Realtime** moves with K5: each process tails the log. Then the outbox publisher does
   nothing; `outbox::enqueue` stops inserting into `outbox_events` (K3.22); the table is kept
   read-only for 30 days and dropped.
6. **External agent events** (A2): A2 serves `external_event_poll` and wake-ups from the log
   and A2's subscriptions, then stops writing `external_agent_events`. K3 emits every event A2
   needs (`reply.*`, `task.assigned`, `later.due`, `message.created` with mentions) from K3.7
   and K3.8 onward.

### 4.4 Search

1. K3.6 adds boards and sheets to today's `search.rs` (a `text_content` column on `sheets`
   maintained by `apply`). Visible at once.
2. K3.17 creates `search_documents` and the consumer, backfills from each source by job.
3. K3.18 switches `search::search` to the index once every registered source's backfill is
   done (feature flag `search.index` per server, default on after one release); the per-table
   `search_vector` columns stay (they cost little and serve kind lists' own filters, like the
   board list) and are not dropped by K3.

## 5. Slices

Order puts the log, the missing kinds' events and the lost-event fix first, because A2 (agent
delivery), A1 (requests and runs as events) and C2 (inbox) build on them. Things and links run
in a second lane in parallel.

**K3.1 Outbox side effects retry instead of being lost** (S). Done by P0.1; here only what goes
beyond it (nothing, unless P0.1's test lacks the forced-failure case below, which K3.1 then
adds). If `notify_people` or
`queue_event_route` fails, the publisher records a failure (`outbox::record_failure`) instead
of marking the event published (`realtime.rs:724-726, 770-844`). Re-broadcast on retry is safe
(clients dedupe on `event_id`); both helpers already dedupe by key. Changes `realtime.rs`.
Depends: P0.1 (if P0.1 has fixed `G` F2, as planned, skip). Accept: `tests/integration/m2_outbox_publisher.rs`
gains a case where notification fan-out fails once (a forced error through a test hook) and
asserts the event is retried and the notification row exists after the second attempt.

**K3.2 The event log and the catalogue** (M). Tables `event_staging`, `event_position_counter`,
`events` (partitioned), immutability trigger; `apps/server/src/events/` becomes a folder:
`mod.rs` (`emit`, `EventContext`, `set_context`), `catalog.rs` (`EventType`, per-module
`EVENTS` consts with the starting catalogue for existing types and legacy names);
`outbox::enqueue` mirrors every outbox event (same id) via `emit`; `set_context` called in the
MCP tool wrapper and the browser write extractor where a transaction is begun by shared helpers
(at least `conversations::append_message_command_with`, `work_item_commands::finalize_change`,
`collections::records::write`, `page_docs::store::save`, `sheets::apply`,
`whiteboards::store::write_elements`). Depends: none. Accept: unit test that every catalogue
name is dotted, unique, and listed by its module's manifest (`modules/mod.rs` guard extended);
integration test `tests/integration/event_log.rs`: sending a message writes one staged event
with the actor and via app set; a rolled-back transaction leaves none.

**K3.3 Sequencer, cursors and the consumer runtime** (M). `events/sequencer.rs` (loop, NOTIFY
as a wake with a 250 ms poll that works alone, no lease, no session advisory lock: §3.3.1),
consumers locking their own `event_consumers` row per batch until K5.2's `consumer:<name>`
leases replace it, `event_consumers`, `event_consumer_failures`,
`events/consumer.rs` (`Consumer`, `run_consumer`, parking), `events::read`, partition and
retention job (`events.partitions`, `events.retention` on `background_jobs`), metrics (staging
backlog age, consumer lag), status-page lines, `events::tail`. If K5.4 starts first, K5.4 builds
the sequencer to this schema and K3.3 is done for that part. Depends: K3.2. Accept, in
`tests/integration/event_log.rs`: (a) two transactions that stage in one order and commit in
the other are read in commit order by a cursor reader that never skips (the test holds the
first open while the second commits and the sequencer runs; then commits the first and checks
the reader sees it after the second); (b) a consumer whose handler fails three times retries,
then parks one event and moves on; the sweep retries it; (c) two sequencers in two pools never
assign the same position; (d) retention deletes signals after 7 days and keeps facts; (e) with
LISTEN disabled (the pooler case), the sequencer and a reader still see a new event within
one poll interval.

**K3.4 Webhooks and automations read the log** (M). The `router` consumer replaces
`events::queue_route` and the `events.route` job; `Event` gains `thing: {kind, id}` beside the
old `Subject` fields (kept for webhook payload compatibility one release); `expand` reads
`events` instead of `outbox_events`. Webhook deliveries add Standard Webhooks headers
(`webhook-id` = event id, `webhook-timestamp`, `webhook-signature: v1,`) beside the existing
`X-Workspace-*` ones (A2 owns rotation and Ed25519). Depends: K3.3. Accept: the existing
webhook and automation integration tests pass unchanged (`tests/integration/` webhook and
automation suites); a new case kills the router mid-batch and asserts no delivery is lost or
duplicated (`webhook_deliveries` unique per subscription and event key).

**K3.5 Pages, sheets and whiteboards in events, automations and webhooks** (M). Emits the
`page.*`, `sheet.*`, `whiteboard.*` facts from their write paths (§3.6); manifests list them;
triggers `page_changed`, `sheet_changed`, `whiteboard_changed` in `automations.rs`
(`Trigger`, `TRIGGER_KINDS`, `kinds_for`) with their module in the manifest; webhook data for
the three; `webhooks::visible_to` checks the thing through `things::visible_ids` for these
kinds (and K1's filter later). Workspace boards (no project) route with their workspace;
`automation_rules` stay project-scoped, so workspace boards can't trigger a rule until K4
gives the workspace a space (stated in the builder). Depends: K3.4. Can run beside K3.6, K3.10.
Accept: `tests/integration/content_events.rs`: an agent edits a page through `page_write`; a
rule "when this page changes, post a message" fires once per revision (two quick saves merged
into one revision fire once); a webhook subscriber gets `page.updated` with title and url and
no body; a person without access to the project gets nothing; a sheet `sheet_write` and a
board `whiteboard_edit` do the same; Pages off for the workspace stops them (`events::shut_out`).

**K3.6 Boards and sheets in search** (S). `SearchKind::{Whiteboards, Sheets}` with SQL over
`whiteboards.search_vector` and a new `sheets.text_content` (`GENERATED` is impossible from
child rows, so `sheets::apply` rewrites it, capped at 20,000 characters, plus a backfill
statement in the migration); `SearchHit.project_id` becomes optional for workspace boards
(web and MCP types updated); manifests list the kinds. Depends: none. Can run beside anything.
Accept: `tests/integration/search_kinds.rs`: a board's sticky text and a sheet's cell text are
found by `search` and the `search` tool; a workspace board shows to a workspace member and not
to someone outside; a project sheet doesn't show to a non-member or with Sheets off.

**K3.7 Write-time domain events for tasks, records, messages, runs and pull requests** (M).
`finalize_change`/`finalize_changes` (`work_item_commands.rs:2764, 2806`) emit `task.*`
(including `task.assigned` with audience); the five writes that bypass them
(`C-tasks.md` finding 1) emit too; `collections::records::write` emits `record.*`;
`append_message_command_with` emits `message.created` with mentions; edits, removals and
reactions emit; `domain::transition_run` emits `run.updated`/`run.finished`; the GitHub sync
emits `pull_request.*` keyed by head sha. The router consumer then routes these directly and
`expand` is deleted. Depends: K3.4. Accept: webhook and automation suites unchanged; a new test
that a task created through every entry point (web, MCP, import, automation, recurring) emits
exactly one `task.created` with the right actor and via app.

**K3.8 Events for the writes that have none** (M). Decisions, conversation pins, milestones,
labels, saved views, files, person questions, field definitions, thread status, task comments
and page comments emit their catalogue events (`G` F5 list). Depends: K3.2. Can run beside
K3.4 to K3.7. Accept: the op-emits guard (K3.21) has no remaining exemptions for these writers.

**K3.9 Notifications read the log** (S). A `notifications` consumer runs `notifications::fan_out`
(unchanged rules) on log events; the publisher stops calling `notify_people`. Depends: K3.3.
Accept: `tests/integration/notifications.rs` passes unchanged; a forced failure retries.

**K3.10 The thing registry** (L, split at the kinds). Migration with `thing_kinds`, `things`,
triggers for `work_items`, `project_pages`, `project_decisions`, `whiteboards`, `sheets`,
`collections`, `collection_records`, `conversations`, `projects`; `things.backfill` job;
`apps/server/src/things/` with `ThingKindSpec` registrations (filled with `Support::Not("K3.x")`
where not built yet), `get`, `get_many`, `visible_ids`; `GET /api/v1/things/{id}`; `thing_open`
by id; sheets join `KINDS`, the manifest's `things`, and become openable; `Thing` gains the new
fields. Depends: none. Can run beside the event lane. Accept: `tests/integration/thing_registry.rs`:
creating, renaming, archiving and deleting each kind through its tool keeps the row in step
(title, status key, version, archived/deleted); a backfill interrupted halfway resumes and ends
with one row per kind row; a task and its record share one `task` row in either write order.

**K3.11 More kinds in the registry** (M). Triggers and backfill for `messages` (batched; title
NULL; removal sets `deleted_at`), `files` (parent = conversation or task),
`project_milestones`, `github_pull_requests`, `repositories`, `automation_rules`, and kinds
`run` on A1's `runs` and `request` on A1's `requests`. There is no trigger on `agent_runs`: a
legacy write must not reach `things` by two routes, and A1.1 has no dependencies, so it merges
first. Depends: K3.10; A1.1 (the `runs` and `requests` tables). Accept: same test file, the added kinds; message removal leaves a tombstone with no text
anywhere in `things`.

**K3.12 The link store, copied and mirrored** (M). `links` table, backfill from stores #2 to
#14, mirror triggers on each old store, `links.backfill` job for late ends. No reader changes.
Depends: K3.10 (K3.11 for message ends). Accept: `tests/integration/links_store.rs`: after
creating a link through each old tool, `links` has the matching row with the right kind,
direction and author; deleting it removes it.

**K3.13 Links from text, without route regexes** (M). `text_link_targets`, `embeds` kind, the
`thing_references` trigger functions switched to it (and writing `links`), new sources
(whiteboard text, record title and long text, sheet cells, page comments and suggestions),
MCP task comments resolve task chips and URLs like the web (`C-tasks.md` finding 6, the
references half). Depends: K3.12. Accept: `tests/integration/awd58_references.rs` passes
unchanged; new cases: a sheet linked from a page shows "Referenced in"; an embed reads as
"Embedded in"; a URL with a `/w/<slug>/` prefix still counts; a URL to a thing in another
workspace doesn't.

**K3.14 One link reader** (M). `links::for_thing` replaces `links_sql` and
`RECORDS_LINKING_SQL`; `part_of` from `things.parent_id`; "made from" shows; task link fields
written through `links::sync_field` from T1's field write path (with T1) and collection link
fields from `collections::records::write`. Depends: K3.12, K3.13; T1.18 (the task link fields,
soft: collection fields go first). Accept: a parity test
(`tests/integration/links_parity.rs`) builds one of every link and asserts the new reader
returns the old reader's set plus the three filled holes; the "Referenced in" web component
test fixtures are unchanged in shape.

**K3.15 Links as operations** (M). `link_create`/`link_delete` operations (K2 generates the
route and the `thing_link`/`thing_unlink` tools; until K2 lands, hand-written route and tool with
an isolation case each); `task_link`, `task_relate`, `task_depend`, `whiteboard_link` and the
web routes call them; `link.created`/`link.deleted` events; mirror triggers reversed for
dependencies and relations. Depends: K3.14; K2.3 (soft: the generator; hand-written until then). Accept: agent parity guard lists the new
tools; isolation cases in `tests/integration/` for both; a cycle of `blocks` is refused with the
same error as today.

**K3.16 Dependency and relation readers move to links** (M). The 11 files that read
`work_item_dependencies` and the 5 that read `work_item_relations` read `links`. Depends: K3.15.
Accept: ready lists, the dependency graph tool and blocked filters return the same results in
their existing tests (`mcp_planning.rs`, `mcp_tasks.rs`).

**K3.17 The search index** (M). `search_documents`, `SearchSource` per kind, the `search`
consumer, backfill job, synchronous delete on removal/archive/delete (trigger on `things`),
scrub list updated. Depends: K3.3, K3.11. Accept: `tests/integration/search_index.rs`: a page
edit is searchable within one consumer pass; a removed message is gone from the index in the
same transaction as the removal; a page moved to an archived project is not returned.

**K3.18 Search reads the index** (M). `search::search` over `search_documents` with
`things::visible_ids` (K1's filter when ready), kinds for every registered source, `spaceId`
and `workspaceId` filters, one ranking. Depends: K3.17; K1.20 (soft: `access::sql_filter_thing`; `visible_ids` until then). Accept: the
existing search tests (`search.rs` tests and the MCP `search` cases) pass unchanged; a private
channel's message never returns to a non-member (an isolation case per kind).

**K3.19 History as a seam** (M). `history.rs` trait and registry; adapters for pages,
boards, records, tasks, sheets, messages; `history_list`, `history_show`, `history_restore`
operations and tools; `task_open include: ["history"]` reads through the seam (same output).
Depends: K3.10. Can run beside everything after K3.10. Accept: `tests/integration/history_seam.rs`:
restoring a page or a board through `history_restore` makes a new version with the restorer as
actor and emits `page.version_restored`/`whiteboard.version_restored`; restore on a sheet
returns the "not supported" error with the reason.

**K3.20 Activity feeds and the audit trail from the log** (M). `activity_list` operation and
tool; `thing.activity` registered in the layout registry; `audit_log.rs` reads audited events
and `audit_events` together; guard counting `INSERT INTO audit_events` sites. Depends: K3.7;
K1.25 (soft: K1's audit list reads through this).
Accept: an owner sees an agent's task change in the audit list with actor, app and requested
by; a member without access to a private channel sees none of its events in any feed.

**K3.21 Guard rails** (S, partly landed earlier). See §6. Depends: K3.10, K3.2.

**K3.24 The direct scope and merged streams in the log** (S). The CHECK that a direct-scope
event's audience is a conversation or an account; `read` answers direct-scope events only to
participants; `EventFilter` takes a set of workspaces (or "every workspace the account is in")
plus "include DMs", over the one global position (§3.3.2, rulings R19, R20). Depends: K3.3,
K4.23 (the direct scope). Accept, in `tests/integration/event_log.rs`: a DM's events reach its
two participants and no workspace owner or admin of either participant's workspaces; one
cursor over a credential that reaches two workspaces returns both workspaces' events in
position order, each naming its workspace, and nothing from a third workspace.

**K3.25 Cold log and history** (M). The `events.archive` job and `event_archives`; audit and
activity read archived ranges; workspace export includes them; the `History` seam's archived
versions for pages (C1 does sheets and document update logs) (§3.3.1 "Cold data", ruling R24
rule 7). Depends: K3.20, K5.11 (the storage seam). Accept: `tests/integration/event_archive.rs`:
a partition archived and dropped still answers an audit query for its positions (slower, same
rows); a page version older than the cut-over opens through `history_show`; no version count
goes down.

**K3.22 The outbox retires** (S). After K5's realtime reads the log: `outbox::enqueue` writes only
the log; publisher loop removed; `outbox_events` read-only, dropped by a later migration after
30 days. Depends: K3.4, K3.9, K5.5 (the live tail reads `events`). Accept: `m2_outbox_publisher.rs` replaced by
log tests; the web's realtime test fixtures pass with envelopes built from events
(`legacy_type` as `type`, `position` as `eventCursor`).

**K3.23 Contract the old link stores** (S). Drops listed in §4.2 step 6 after one release with
no readers. Depends: K3.16, A2.28 (for `external_agent_events`, if A2 asks K3 to drop it).
Accept: a test that no Rust file names a dropped table.

## 6. Guard rails

1. **Every registered kind has every shared service or says why not.**
   `things::tests::every_kind_has_shared_services`: for each `ThingKindSpec`, each of events,
   search, history, export and links is `Support::Yes` or `Support::Not(reason)` with a
   non-empty reason; `Not("K3.x")` placeholders are allowed only while the slice is open and
   the test lists them so the count can only go down (a constant `MAX_PENDING` lowered by each
   slice). Also: every kind in `thing_kinds` (read in an integration test) has a spec; every
   `events: Yes(names)` name is in the catalogue; every `search: Yes` kind has a
   `document_sql` whose bind count matches (the `declared_params` pattern, `things.rs:265`).
2. **Every write emits an event.** In test builds `events::emit` increments a transaction-local
   counter; the K2 operation runner (or, until then, a wrapper in the parity scenario
   `tests/integration/awd91_agent_parity.rs`) asserts that every committed write operation
   emitted at least one event of a type its module declares. Exemptions are a listed constant
   with reasons.
3. **Payloads carry no text.** A unit test walks each `EventType`'s payload schema and fails on
   any string property not in an allowlist of key-like names (`type`, `status`, field keys,
   `changed`), so message or page text can't slip in.
4. **No new link stores.** A test scans migrations newer than the links migration for `CREATE
   TABLE` names ending in `_links`, `_references` or `_relations` and for columns ending in
   `_from_id` without a matching mirror trigger; it fails with "use `links`".
5. **No new outbox or audit writers.** After K3.22, a grep test refuses `outbox::enqueue` and
   `OutboxEventPayload::` outside `events/legacy.rs`; after K3.20, `INSERT INTO audit_events` is
   allowed only for non-thing actions (a counted allowlist).
6. **Manifests and the catalogue agree.** Extend `modules/mod.rs` guards: every name in
   `events.emits` exists in the catalogue with the same module, and every catalogue entry is
   listed by its module.
7. **Readers use the access wrapper.** A test refuses SQL reading `events`, `links` or
   `search_documents` outside `events/`, `links.rs`, `search/` (the only places that apply the
   filter).

## 7. Risks and open questions

| Risk | How we'd notice | Fallback |
| --- | --- | --- |
| Trigger cost on hot tables (messages, work items, records) | p95 of message send and task update in the performance checks (`docs/operations/performance.md`) | triggers fire only on envelope columns; if still costly, move the message trigger's work into `append_message_command_with` |
| The sequencer stalls (bug, long lock) and every consumer waits | staging backlog age metric and status-page line; alert at 30 s | any process can take over the lease; a manual `events sequence` CLI subcommand drains once |
| A long-running writer transaction delays its own events (not others') | consumer lag per type | statement and idle-in-transaction timeouts already bound transactions |
| Event volume from live page and board saves | rows per day by type | they are `signal` class with 7-day retention; if still heavy, coalesce per page per second in `emit` |
| Old and new link stores drift during dual-write | the parity test (K3.14) and a nightly `links.verify` job comparing counts per store | the old stores stay authoritative until K3.15, so drift only affects the new reader, which is behind the parity test |
| Search index copies text that must vanish | the removal test in K3.17 | synchronous delete by trigger; the index can be rebuilt from sources at any time |
| Tasks as records (T1, #251's successors) changes `work_items` and `collection_records` while K3 adds triggers | merge conflicts in migrations; registry rows of kind `record` for tasks | the trigger rule in §4.1; T1 and K3 agree the marker column before K3.10 |
| Global consumers let one workspace slow others | per-workspace lag metric | shard by workspace hash (§3.3.4) |
| `visible_ids` wrapper drifts from K1's filter | K1's isolation suite | K1 replaces its body in one place; K3 readers don't change |

Questions only the owner can answer:

1. **How long is the event log kept?** It feeds the audit trail and activity feeds. Recommended:
   facts 400 days by default on every plan (so a year of audit is free), signals 7 days, and
   history tables kept forever as END-STATE promises.
2. **Should pasting a URL that contains a thing's id count as a reference even when it points
   at another host?** Recommended: yes within the same workspace (it's harmless and survives a
   domain change), no across workspaces.
3. **Should agents see other people's activity in a feed by default?** Recommended: yes, by the
   same access rule as people; A2's ambient mode decides what is pushed, not what is readable.

## 8. Touch points

**Needs from:**

- **K1**: the SQL access filter over `things` (inputs: `workspace_id`, `space_id`, `kind`,
  `access_parent_id`, the thing's own rule for conversations and boards), to replace the body of
  `things::visible_ids`; permission names for `link_create` (edit on `from`, see on `to`),
  `history_restore` (edit, or propose), `activity_list` and audit (see). The rename of
  `actors` to accounts (K3's new columns say `account` and reference `actors(id)` until then).
- **K2**: the operation runner calling `events::set_context` and asserting "emits an event";
  `OpContext` with actor, via app, requested by, run, request id; generated routes and tools
  for `link_create`, `link_delete`, `history_*`, `activity_list`.
- **K4**: `spaces` (K3's `space_id` columns point at `projects` until the rename); a space for
  every workspace so workspace boards get a `space_id` and can trigger rules; moving DMs out
  of the direct project rewrites `things.space_id` for conversations and messages.
- **K5**: `consumer:<name>` leases (K5.2; consumers lock their own cursor row per batch until
  then; the sequencer needs no lease); the wake listener and the `live` role tailing `events`
  with `events::tail`, NOTIFY as a wake and a poll as the fallback (K5.4, K5.5; K3.22 waits for
  K5.5); the storage seam for archived log partitions (K5.11, K3.25).
- **K4**: the direct scope's reserved id (K4.23) for the CHECK in K3.24.
- **T1**: the task-record marker for the registry trigger; task link fields written through
  `links::sync_field`; the task history converter (K3's seam only reads).
- **C1**: history depth (sheet snapshots, record and task revert); comments becoming
  conversations (then text links from comments have a thing as source and account mentions
  get one table); files attached to any thing (`things.parent_id`).
- **A1**: emits `run.*` and `request.*` through `events::emit`; runs are things.
- **M1**: reads `ThingKindSpec` and `EventType` registrations from module manifests (data-made
  modules insert `thing_kinds` rows and catalogue entries); K3 keeps the registration shape
  M1 can fill.
- **H1**: the per-kind `export` hook names; event retention as a plan value.
- **P0**: `G` F2 is P0.1's (K3.1 is done by it).

**Provides to:**

- **A2**: `events::read(filter, after, limit)` with access re-checked at read time; positions
  usable as `Last-Event-ID`; the catalogue with `reply.*`, `task.assigned`, `message.created`
  with mentions, `later.due`; audience events; legacy names for `external_event_poll`.
- **C2**: `search_documents` and its filter for the search experience; the `notifications`
  consumer with unchanged rules; activity feeds for the inbox.
- **W1**: the `thing.activity` and links pieces' data; `GET /api/v1/things/{id}`; envelopes
  with `legacy_type` until the client moves to catalogue names.
- **Every module owner**: `events::emit`, `links::create/sync_field/replace_text_links`,
  `ThingKindSpec`, `History`, `SearchSource`.

## 9. Changes to the end state

1. **Containment is the thing's parent, not a link.** Proposed wording in §3 "Things":
   "Every thing has an id, a kind, a title, a status, an owner, a space, a parent where it
   lives inside another thing, and a version." And under "Link": "one thing pointing at
   another, with a kind (blocks, about, made from, relates to, duplicates, mentions, embeds).
   Being part of something is the thing's parent and reads as a link both ways."
2. **Mentions of people are not links.** Proposed: "Mentions of things, link fields, 'made from
   this message' and embeds all create links. Mentions of people are recorded with the
   message and drive notifications and wake-ups."
3. **Not everything that moves is an event.** Proposed addition under "Event": "Streaming text
   and typing indicators are live signals outside the log; everything that changes stored data
   is in it."
4. **The audit trail is the log plus a security record.** Proposed under §5 "The operation
   layer": "the entry in the audit trail (the event it emits; sign-ins and refused attempts
   are kept beside it)."
5. **History is kept per kind; restore where possible.** END-STATE says every thing "can show
   or restore earlier versions"; sheets, records and tasks can't restore today. Proposed:
   "Every thing keeps who changed what; documents and records can show or restore earlier
   versions; other kinds show their changes."
6. **Event log retention is a plan value, history isn't.** Add to §6 Hosted, "What is never
   charged": "Message and document history is never cut; the event log behind activity and
   audit is kept for a year on every plan."

## Triggers (ruling R16)

| Trigger | On table | Writes | Added by | Removed by |
| --- | --- | --- | --- | --- |
| things sync (one function per kind) | `work_items`, `project_pages`, `project_decisions`, `whiteboards`, `sheets`, `collections`, `collection_records`, `conversations`, `projects`; then `messages`, `files`, `runs` and `requests` (A1), `project_milestones`, `github_pull_requests`, `repositories`, `automation_rules` | `things` | K3.10, K3.11 | permanent (the registry is kept by its kinds; M1's data-made kinds write `things` through `things::upsert` instead) |
| link mirrors | old link stores #2 to #14 (§2.1) | `links` | K3.12 (dependencies and relations reversed in K3.15) | K3.23, except the pull-request link tables and "made from" columns, which keep mirroring (§4.2 step 6) |
| `thing_references` trigger functions (rewritten) | `project_pages`, `project_decisions`, `messages`, task text | `thing_references`, then `links` | K3.13 | K3.23 (drops `thing_references`) |
| search delete | `things` | `search_documents` | K3.17 | permanent |
| events immutability | `events` | refuses UPDATE | K3.2 | permanent |
| `event_staging_wake` | `event_staging` | `pg_notify` only | K3.2 | permanent (a wake, no rows) |

## Slice table

| Id | Name | Size | Depends on | Can run beside |
| --- | --- | --- | --- | --- |
| K3.1 | Outbox side effects retry instead of being lost (done by P0.1) | S | P0.1 | all |
| K3.2 | The event log and the catalogue | M | — | K3.1, K3.6, K3.10 |
| K3.3 | Sequencer, cursors and the consumer runtime | M | K3.2 | K3.6, K3.8, K3.10 |
| K3.4 | Webhooks and automations read the log | M | K3.3 | K3.6, K3.8, K3.9, K3.10 |
| K3.5 | Pages, sheets and whiteboards in events, automations and webhooks | M | K3.4 | K3.6, K3.7, K3.10 |
| K3.6 | Boards and sheets in search | S | — | all |
| K3.7 | Write-time domain events for tasks, records, messages, runs, PRs | M | K3.4 | K3.5, K3.8, K3.10 |
| K3.8 | Events for the writes that have none | M | K3.2 | K3.3 to K3.7, K3.10 |
| K3.9 | Notifications read the log | S | K3.3 | K3.4 to K3.8 |
| K3.10 | The thing registry | L | T1.3 (soft, task-record marker) | event lane |
| K3.11 | More kinds in the registry | M | K3.10, A1.1 | event lane, K3.19 |
| K3.12 | The link store, copied and mirrored | M | K3.10, K3.11 | event lane, K3.19 |
| K3.13 | Links from text, without route regexes | M | K3.12 | event lane, K3.19 |
| K3.14 | One link reader | M | K3.12, K3.13, T1.18 (soft) | K3.17, K3.19 |
| K3.15 | Links as operations | M | K3.14, K2.3 (soft) | K3.17 to K3.20 |
| K3.16 | Dependency and relation readers move to links | M | K3.15 | K3.17 to K3.20 |
| K3.17 | The search index | M | K3.3, K3.11 | K3.14 to K3.16, K3.19 |
| K3.18 | Search reads the index | M | K3.17, K1.20 (soft) | K3.15, K3.16, K3.19, K3.20 |
| K3.19 | History as a seam | M | K3.10 | K3.11 onward |
| K3.20 | Activity feeds and the audit trail from the log | M | K3.7, K1.25 (soft) | K3.14 onward |
| K3.21 | Guard rails | S | K3.2, K3.10 | all |
| K3.22 | The outbox retires | S | K3.4, K3.9, K5.5 | K3.23 |
| K3.23 | Contract the old link stores | S | K3.16, A2.28 (for its table) | K3.22 |
| K3.24 | The direct scope and merged streams in the log | S | K3.3, K4.23 | K3.4 onward |
| K3.25 | Cold log and history | M | K3.20, K5.11 | K3.21 onward |

## Review changes

- §3.1: noted `visible_ids` is a stopgap replaced by K1.20; added `ThingKind`, `Located`/`things::locate`, `KindProvider` and `things::upsert`, the interfaces K1, K2 and M1 call (interface check).
- §3.2 `links.via_app_id` renamed `via_credential_id` (K1's `account_credentials`), so K1 and K3 name the same thing.
- §3.3.1 `events` gained `thing_version`, `changes`, `audience`, `conversation_id`, `conversation_restricted`, with CHECKs: the one envelope of ruling R8.
- §3.3.1 added `realtime_channel`, `wake_event_log` and the `event_staging_wake` trigger, and a paragraph that this is the only log design and K5 owns the wake, tail and fan-out (R8).
- Sequencer: notifies `realtime_channel('events')`; no lease (the counter row serializes); wake by trigger; poll alone works (R8, R24 rule 3).
- §3.3.2: `via_app` became `via_credential`; `Audience` enum; `NewEvent` gained `version`, `changes`, `NewEvent::thing`; `events::tail` for K5; `set_context` uses `SET LOCAL` only (R8, R24 rule 3).
- §3.3.2: direct-scope events and merged multi-workspace streams under one global position (R19, R20).
- §3.3.1: cold data archived to object storage and read back; history never cut (R24 rule 7).
- §3.3.3 catalogue names aligned with K1 and K4 (`*.member.added/removed/role_changed`, `space.*`, `workspace.*`, K1's access events, `conversation.visibility.changed`).
- §3.3.4: consumers lock their own cursor row until K5.2's leases; never a session advisory lock (R24 rule 3).
- K3.1 marked done by P0.1 (R10).
- K3.3: no advisory lease; K5.4 may build the sequencer to this schema; pooler acceptance case.
- K3.11: run trigger on A1's `runs`.
- Depends added: K3.10 T1.3, K3.11 A1.1, K3.14 T1.18, K3.15 K2.3, K3.18 K1.20, K3.20 K1.25, K3.22 K5.5, K3.23 A2.28 (named slices, soft where the text has a stopgap).
- New slices K3.24 (direct scope and merged streams, R19, R20) and K3.25 (cold log and history, R24 rule 7).
- §8 touch points for K5, K4 and P0 updated to the above.
- Added the trigger table (R16).
- Content review questions answered in §3.2: milestone parent may be an initiative record; `embeds` may link a conversation to a thing (R27); history is K3.19 and `activity_list` K3.20; A1 uses `made_from`.

### Second pass (agents and hosting review, R32 to R42)

- §3.3.3: `request.created` renamed `request.opened` (one per addressed account); added A1's `run.start`, `run.cancel`, `host.notice`, `host.command` and C1's `comment.mentioned`.
- K3.11: thing sync for `run` on `runs` and for `request` on `requests`, no `agent_runs` fallback; A1.1 is now a hard dependency (it has none of its own). Trigger table updated.
