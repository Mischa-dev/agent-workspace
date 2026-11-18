//! Full-text search across messages, thread outcomes, tasks, pages,
//! decisions, collection records, whiteboards, sheets and file names. Results only ever include what the viewer can
//! see: messages, threads and message files go through
//! `conversation_visible_to`, and a decision from a private conversation
//! stays as private as it.
//!
//! Settled knowledge ranks first: decisions, then pages, thread outcomes and
//! tasks, then raw messages.

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    addresses::{Addresses, Place, Target},
    auth::AuthenticatedActor,
    error::{ApiQuery, AppError},
    modules::state::ProjectModules,
};

pub mod grammar;

pub const MAX_QUERY_CHARACTERS: usize = 200;

pub fn router() -> Router<AppState> {
    Router::new().route("/api/v1/search", get(search_http))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchKind {
    Messages,
    Threads,
    Tasks,
    Pages,
    Decisions,
    /// A collection's records (AWD-110).
    Records,
    /// Boards by title and the text on them (K3.6).
    Whiteboards,
    /// Sheets by name, tab names and text cells (K3.6).
    Sheets,
    Files,
}

impl SearchKind {
    pub const ALL: [Self; 9] = [
        Self::Decisions,
        Self::Pages,
        Self::Threads,
        Self::Tasks,
        Self::Records,
        Self::Whiteboards,
        Self::Sheets,
        Self::Messages,
        Self::Files,
    ];

    /// The module whose manifest lists this kind.
    pub fn module(self) -> Option<&'static str> {
        crate::modules::manifest::of_search(self.name()).map(|manifest| manifest.id.as_str())
    }

    /// The kind's name, as search results and module manifests use it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Messages => "messages",
            Self::Threads => "threads",
            Self::Tasks => "tasks",
            Self::Pages => "pages",
            Self::Decisions => "decisions",
            Self::Records => "records",
            Self::Whiteboards => "whiteboards",
            Self::Sheets => "sheets",
            Self::Files => "files",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "message" | "messages" => Some(Self::Messages),
            "thread" | "threads" => Some(Self::Threads),
            "task" | "tasks" => Some(Self::Tasks),
            "page" | "pages" => Some(Self::Pages),
            "decision" | "decisions" => Some(Self::Decisions),
            "record" | "records" => Some(Self::Records),
            "whiteboard" | "whiteboards" | "board" | "boards" => Some(Self::Whiteboards),
            "sheet" | "sheets" => Some(Self::Sheets),
            "file" | "files" => Some(Self::Files),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SearchFilter {
    /// Only these conversations and their threads.
    pub conversation_ids: Option<Vec<Uuid>>,
    /// Only what these accounts wrote (assigned, for tasks; last changed, for
    /// pages, records and sheets).
    pub author_ids: Option<Vec<Uuid>>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
    /// The grammar's `has:` and `is:` filters, as `has:file`, `is:dm`.
    pub flags: Vec<String>,
    /// The caller's credential reaches one project only: workspace boards,
    /// which are in no project, are out of its reach.
    pub confined: bool,
}

impl SearchFilter {
    /// Whether anything besides the words narrows the search.
    pub fn narrows(&self) -> bool {
        self.conversation_ids.is_some()
            || self.author_ids.is_some()
            || self.since.is_some()
            || self.until.is_some()
            || !self.flags.is_empty()
    }

    /// Of `kinds`, those this filter can find anything in. Only messages
    /// carry `has:`, `is:unread` and `is:saved`; `is:thread` and `is:dm`
    /// also keep threads and files sent there. With no words, only
    /// conversations' messages, threads and files are listed.
    fn kinds_for(&self, text: &str, kinds: &[SearchKind]) -> Vec<SearchKind> {
        let messages_only = self
            .flags
            .iter()
            .any(|flag| flag.starts_with("has:") || flag == "is:unread" || flag == "is:saved");
        let in_conversations = text.is_empty()
            || self
                .flags
                .iter()
                .any(|flag| flag == "is:thread" || flag == "is:dm");
        kinds
            .iter()
            .copied()
            .filter(|kind| !messages_only || *kind == SearchKind::Messages)
            .filter(|kind| {
                !in_conversations
                    || matches!(
                        kind,
                        SearchKind::Messages | SearchKind::Threads | SearchKind::Files
                    )
            })
            .collect()
    }
}

/// A query in the search grammar, made into what [`search`] takes: its
/// words, kinds and filter. `None` when a filter names nothing the viewer
/// can see (an `in:#channel` they can't read, a `from:` no one has), so
/// nothing can match.
pub struct Resolved {
    pub text: String,
    pub kinds: Vec<SearchKind>,
    pub filter: SearchFilter,
}

/// Turns parsed grammar into a search: `from:` and `in:` are looked up
/// among the spaces searched, `in:` only among conversations the viewer
/// can read. `kinds` (from a separate argument) and `kind:` both narrow.
pub async fn resolve(
    pool: &PgPool,
    viewer: Uuid,
    projects: &[Uuid],
    parsed: &grammar::ParsedQuery,
    kinds: &[SearchKind],
    mut filter: SearchFilter,
) -> Result<Option<Resolved>, AppError> {
    let mut chosen = kinds.to_vec();
    if !parsed.kinds.is_empty() {
        let named = parsed
            .kinds
            .iter()
            .filter_map(|kind| SearchKind::parse(kind))
            .collect::<Vec<_>>();
        chosen = if chosen.is_empty() {
            named
        } else {
            chosen
                .into_iter()
                .filter(|kind| named.contains(kind))
                .collect()
        };
        if chosen.is_empty() {
            return Ok(None);
        }
    }
    if !parsed.from.is_empty() {
        let handles = parsed
            .from
            .iter()
            .filter(|handle| *handle != "me")
            .cloned()
            .collect::<Vec<_>>();
        let mut authors = sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT actor_id FROM project_memberships \
             WHERE project_id = ANY($1) AND enabled AND lower(handle) = ANY($2)",
        )
        .bind(projects)
        .bind(&handles)
        .fetch_all(crate::db::scoped(pool))
        .await?;
        if parsed.from.iter().any(|handle| handle == "me") {
            authors.push(viewer);
        }
        let Some(authors) = narrow(filter.author_ids.take(), authors) else {
            return Ok(None);
        };
        filter.author_ids = Some(authors);
    }
    if !(parsed.in_channels.is_empty() && parsed.in_people.is_empty()) {
        let places = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT conversation.id
            FROM conversations conversation
            WHERE conversation.project_id = ANY($2)
              AND conversation.archived_at IS NULL
              AND conversation_visible_to(conversation.id, $1)
              AND (
                  (conversation.kind = 'channel'
                   AND (lower(conversation.name) = ANY($3)
                        OR lower(replace(conversation.name, ' ', '-')) = ANY($3)))
                  OR (conversation.kind = 'dm' AND EXISTS (
                      SELECT 1
                      FROM conversation_members them
                      JOIN project_memberships person
                        ON person.project_id = them.project_id AND person.actor_id = them.actor_id
                      WHERE them.conversation_id = conversation.id
                        AND them.actor_id <> $1
                        AND lower(person.handle) = ANY($4)))
              )
            "#,
        )
        .bind(viewer)
        .bind(projects)
        .bind(&parsed.in_channels)
        .bind(&parsed.in_people)
        .fetch_all(crate::db::scoped(pool))
        .await?;
        let Some(places) = narrow(filter.conversation_ids.take(), places) else {
            return Ok(None);
        };
        filter.conversation_ids = Some(places);
    }
    let day = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|day| chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
            .and_then(|day| day.and_hms_opt(0, 0, 0))
            .map(|time| time.and_utc())
    };
    if let Some(after) = day(&parsed.after) {
        filter.since = Some(filter.since.map_or(after, |since| since.max(after)));
    }
    if let Some(before) = day(&parsed.before) {
        let until = before - chrono::Duration::microseconds(1);
        filter.until = Some(filter.until.map_or(until, |current| current.min(until)));
    }
    filter
        .flags
        .extend(parsed.has.iter().map(|has| format!("has:{has}")));
    filter
        .flags
        .extend(parsed.is.iter().map(|is| format!("is:{is}")));
    Ok(Some(Resolved {
        text: parsed.text.clone(),
        kinds: chosen,
        filter,
    }))
}

/// `found` within `already`, when both narrow; `None` when nothing is left.
fn narrow(already: Option<Vec<Uuid>>, mut found: Vec<Uuid>) -> Option<Vec<Uuid>> {
    if let Some(already) = already {
        found.retain(|id| already.contains(id));
    }
    found.sort_unstable();
    found.dedup();
    (!found.is_empty()).then_some(found)
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub kind: String,
    pub id: Uuid,
    pub title: String,
    /// A thread's or task's status, or "superseded" for a replaced decision.
    pub status: Option<String>,
    pub snippet: String,
    /// None for a workspace's own board, which is in no project.
    pub project_id: Option<Uuid>,
    pub workspace_id: Uuid,
    pub project_name: String,
    pub conversation_id: Option<Uuid>,
    pub conversation_name: Option<String>,
    pub author: Option<String>,
    pub at: String,
    /// The path in the web app.
    #[sqlx(default)]
    pub path: String,
    #[serde(skip)]
    pub rank: f32,
    /// Where the hit opens, as the query found it: [`SearchHit::target`].
    #[serde(skip)]
    pub link_kind: String,
    #[serde(skip)]
    pub link_id: Uuid,
    #[serde(skip)]
    pub link_child: Option<Uuid>,
}

impl SearchHit {
    /// Where the hit opens, and the anchor in it (`#message-<id>`).
    fn target(&self) -> (Target, Option<String>) {
        let message = self.link_child.map(|id| format!("#message-{id}"));
        if self.link_kind == "whiteboard" {
            return (Target::Whiteboard(self.link_id), None);
        }
        // Every other kind lives in a space; only a workspace's own board
        // has none.
        let space = self.project_id.unwrap_or_default();
        match self.link_kind.as_str() {
            "direct" => (
                Target::Direct {
                    id: self.link_id,
                    thread: None,
                },
                message,
            ),
            "thread" => (Target::space(space, Place::Thread(self.link_id)), message),
            "channel" => (Target::space(space, Place::Channel(self.link_id)), message),
            "page" => (Target::page(space, self.link_id), None),
            "sheet" => (Target::space(space, Place::Sheet(self.link_id)), None),
            "decision" => (
                Target::space(space, Place::Decisions),
                Some(format!("#{}", self.link_id)),
            ),
            "record" => match self.link_child {
                Some(record) => (
                    Target::space(
                        space,
                        Place::Record {
                            collection: self.link_id,
                            record,
                        },
                    ),
                    None,
                ),
                None => (Target::space(space, Place::Collection(self.link_id)), None),
            },
            _ => (Target::task(space, self.link_id), None),
        }
    }

    fn link(&mut self, addresses: Addresses) {
        let (target, anchor) = self.target();
        self.path = format!(
            "{}{}",
            addresses.in_workspace(self.workspace_id, &target),
            anchor.unwrap_or_default()
        );
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn search(
    pool: &PgPool,
    addresses: Addresses,
    viewer: Uuid,
    projects: &[Uuid],
    text: &str,
    kinds: &[SearchKind],
    filter: &SearchFilter,
    limit: i64,
) -> Result<Vec<SearchHit>, AppError> {
    let text = text.trim();
    if (text.is_empty() && !filter.narrows()) || text.chars().count() > MAX_QUERY_CHARACTERS {
        return Err(AppError::Validation("query"));
    }
    let limit = limit.clamp(1, 100);
    let kinds = filter.kinds_for(
        text,
        if kinds.is_empty() {
            &SearchKind::ALL[..]
        } else {
            kinds
        },
    );
    let kinds = kinds.as_slice();
    if kinds.is_empty() {
        return Ok(Vec::new());
    }
    // AWD-75: each kind searches only where its module is on.
    let modules = {
        let mut connection = crate::db::scoped_tx(pool).await?;
        ProjectModules::load(&mut connection, viewer, projects).await?
    };
    // Tasks, and files attached to tasks, come only from projects where the
    // viewer (and an app's limit) still sees tasks, and Tasks is on.
    let task_projects = if kinds.contains(&SearchKind::Tasks) || kinds.contains(&SearchKind::Files)
    {
        modules.with(
            &projects_showing_tasks(pool, viewer, projects).await?,
            "tasks",
        )
    } else {
        Vec::new()
    };
    // Files sent in a conversation come only from projects whose conversations
    // show: DMs with Messages on, a project's channels with Channels on.
    let conversation_projects = if kinds.contains(&SearchKind::Files) {
        modules.with_conversations(projects)
    } else {
        Vec::new()
    };
    // Workspace boards (in no project) come from the workspaces of the
    // projects searched, where Whiteboards is on for the viewer; the board's
    // own rule (`whiteboard_visible_to`) decides each one.
    let board_module = SearchKind::Whiteboards.module();
    let board_workspaces = if let (true, false, Some(board_module)) = (
        kinds.contains(&SearchKind::Whiteboards),
        filter.confined,
        board_module,
    ) {
        let mut connection = crate::db::scoped_tx(pool).await?;
        let on = crate::modules::state::by_workspace(&mut connection, viewer)
            .await?
            .into_iter()
            .filter(|(_, set)| set.contains(board_module))
            .map(|(workspace, _)| workspace)
            .collect::<Vec<_>>();
        sqlx::query_scalar::<_, Uuid>(
            "SELECT DISTINCT organization_id FROM projects \
             WHERE id = ANY($1) AND organization_id = ANY($2)",
        )
        .bind(projects)
        .bind(&on)
        .fetch_all(&mut *connection)
        .await?
    } else {
        Vec::new()
    };
    let mut hits = Vec::new();
    for kind in kinds {
        let projects = match kind {
            SearchKind::Messages | SearchKind::Threads => modules.with_conversations(projects),
            SearchKind::Tasks => task_projects.clone(),
            SearchKind::Pages | SearchKind::Decisions => modules.with(projects, "pages"),
            SearchKind::Records => modules.with(projects, "collections"),
            // Kinds since K3.6 follow the module whose manifest lists them.
            SearchKind::Whiteboards | SearchKind::Sheets => kind
                .module()
                .map(|module| modules.with(projects, module))
                .unwrap_or_default(),
            // Files hang off messages and tasks.
            SearchKind::Files => modules.keep(projects, |set, direct| {
                set.allows_conversations_in(if direct { "direct" } else { "standard" })
                    || (!direct && set.contains("tasks"))
            }),
        };
        if projects.is_empty()
            && !(*kind == SearchKind::Whiteboards && !board_workspaces.is_empty())
        {
            continue;
        }
        let projects = projects.as_slice();
        let sql = match kind {
            SearchKind::Messages => MESSAGES_SQL,
            SearchKind::Threads => THREADS_SQL,
            SearchKind::Tasks => TASKS_SQL,
            SearchKind::Pages => PAGES_SQL,
            SearchKind::Decisions => DECISIONS_SQL,
            SearchKind::Records => RECORDS_SQL,
            SearchKind::Whiteboards => WHITEBOARDS_SQL,
            SearchKind::Sheets => SHEETS_SQL,
            SearchKind::Files => FILES_SQL,
        };
        let mut query = sqlx::query_as::<_, SearchHit>(sql)
            .bind(viewer)
            .bind(projects)
            .bind(text)
            .bind(filter.conversation_ids.as_deref())
            .bind(filter.author_ids.as_deref())
            .bind(filter.since)
            .bind(filter.until)
            .bind(limit);
        if matches!(kind, SearchKind::Messages | SearchKind::Threads) {
            query = query.bind(&filter.flags[..]);
        }
        if *kind == SearchKind::Files {
            query = query
                .bind(&task_projects[..])
                .bind(&conversation_projects[..])
                .bind(&filter.flags[..]);
        }
        if *kind == SearchKind::Whiteboards {
            query = query.bind(&board_workspaces[..]);
        }
        let rows = query.fetch_all(crate::db::scoped(pool)).await?;
        hits.extend(rows.into_iter().map(|mut hit| {
            hit.link(addresses);
            hit
        }));
    }
    hits.sort_by(|left, right| {
        right
            .rank
            .partial_cmp(&left.rank)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| right.at.cmp(&left.at))
    });
    hits.truncate(usize::try_from(limit).unwrap_or(100));
    Ok(hits)
}

// Every query takes the same parameters:
// $1 viewer, $2 projects, $3 text ('' when only filters narrow it, for the
// kinds that allow that), $4 conversations, $5 authors, $6 since, $7 until,
// $8 limit. Unused ones are compared against NULL so the planner
// drops them. The files query also takes $9, the projects where the viewer
// still sees tasks (`projects_showing_tasks`), for files attached to tasks,
// and $10, the projects whose conversations show with the modules on for
// the viewer, for files sent in a conversation.

const MESSAGES_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('english', $3) AS q)
    SELECT 'message' AS kind, message.id,
           COALESCE(author.display_name, 'Someone') || ' in ' ||
               CASE WHEN conversation.kind = 'dm' THEN 'a DM' ELSE conversation.name END AS title,
           NULL::text AS status,
           CASE WHEN $3 = '' THEN left(message.plain_text, 300)
                ELSE ts_headline('english', message.plain_text, query.q,
                                 'MaxWords=40, MinWords=12, StartSel=**, StopSel=**') END AS snippet,
           message.project_id, project.organization_id AS workspace_id,
           CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END AS project_name,
           conversation.id AS conversation_id, conversation.name AS conversation_name,
           author.display_name AS author, message.created_at::text AS at,
           CASE WHEN space.direct THEN 'direct'
                WHEN conversation.kind = 'thread' THEN 'thread' ELSE 'channel' END AS link_kind,
           CASE WHEN space.direct
                THEN COALESCE(conversation.parent_channel_id, conversation.id)
                ELSE conversation.id END AS link_id,
           NULL::uuid AS link_child,
           (CASE WHEN $3 = '' THEN 0 ELSE ts_rank(message.search_vector, query.q) END)::real AS rank
    FROM query, messages message
    JOIN conversations conversation ON conversation.id = message.conversation_id
    JOIN projects project ON project.id = message.project_id
    CROSS JOIN LATERAL (SELECT project.kind = 'direct' AS direct) space
    LEFT JOIN project_memberships author
      ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
    WHERE ($3 = '' OR message.search_vector @@ query.q)
      AND message.removed_at IS NULL
      AND message.project_id = ANY($2)
      AND conversation.archived_at IS NULL
      AND conversation_visible_to(conversation.id, $1)
      AND ($4::uuid[] IS NULL OR conversation.id = ANY($4) OR conversation.parent_channel_id = ANY($4))
      AND ($5::uuid[] IS NULL OR message.author_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR message.created_at >= $6)
      AND ($7::timestamptz IS NULL OR message.created_at <= $7)
      -- $9 (messages only): the grammar's has: and is: filters.
      AND ('has:file' <> ALL($9) OR EXISTS (
          SELECT 1 FROM files attached
          WHERE attached.message_id = message.id AND attached.removed_at IS NULL))
      AND ('has:link' <> ALL($9) OR message.plain_text ~* 'https?://')
      AND ('has:reaction' <> ALL($9) OR EXISTS (
          SELECT 1 FROM message_reactions reaction WHERE reaction.message_id = message.id))
      AND ('has:pin' <> ALL($9) OR EXISTS (
          SELECT 1 FROM conversation_pins pin
          WHERE pin.target_kind = 'message' AND pin.target_id = message.id))
      AND ('has:task' <> ALL($9) OR EXISTS (
          SELECT 1 FROM thing_references reference
          WHERE reference.source_kind = 'message' AND reference.source_id = message.id
            AND reference.target_kind = 'task'))
      AND ('is:thread' <> ALL($9) OR conversation.kind = 'thread')
      AND ('is:dm' <> ALL($9) OR space.direct)
      AND ('is:saved' <> ALL($9) OR EXISTS (
          SELECT 1 FROM saved_items saved
          WHERE saved.actor_id = $1 AND saved.message_id = message.id))
      AND ('is:unread' <> ALL($9) OR (
          message.author_actor_id IS DISTINCT FROM $1
          AND NOT EXISTS (
              SELECT 1
              FROM conversation_entries entry
              JOIN conversation_read_states state
                ON state.conversation_id = entry.conversation_id AND state.actor_id = $1
              WHERE entry.message_id = message.id
                AND entry.sequence <= state.highest_seen_sequence)))
    ORDER BY rank DESC, message.created_at DESC
    LIMIT $8
"#;

const THREADS_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('english', $3) AS q)
    SELECT 'thread' AS kind, thread.id,
           thread.name AS title, thread.thread_status AS status,
           COALESCE(thread.thread_outcome, '') AS snippet,
           thread.project_id, project.organization_id AS workspace_id,
           CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END AS project_name,
           parent.id AS conversation_id, parent.name AS conversation_name,
           NULL::text AS author,
           COALESCE(thread.thread_status_changed_at, thread.created_at)::text AS at,
           CASE WHEN space.direct THEN 'direct' ELSE 'thread' END AS link_kind,
           CASE WHEN space.direct THEN parent.id ELSE thread.id END AS link_id,
           NULL::uuid AS link_child,
           (CASE WHEN $3 = '' THEN 0
                 ELSE ts_rank(to_tsvector('english', thread.name || ' ' || COALESCE(thread.thread_outcome, '')),
                              query.q) * 1.5 END)::real AS rank
    FROM query, conversations thread
    JOIN conversations parent ON parent.id = thread.parent_channel_id
    JOIN projects project ON project.id = thread.project_id
    CROSS JOIN LATERAL (SELECT project.kind = 'direct' AS direct) space
    WHERE thread.kind = 'thread'
      AND ($3 = ''
           OR to_tsvector('english', thread.name || ' ' || COALESCE(thread.thread_outcome, '')) @@ query.q)
      AND thread.project_id = ANY($2)
      AND thread.archived_at IS NULL
      AND conversation_visible_to(thread.id, $1)
      AND ($4::uuid[] IS NULL OR parent.id = ANY($4) OR thread.id = ANY($4))
      AND ($5::uuid[] IS NULL OR thread.thread_status_changed_by_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR thread.created_at >= $6)
      AND ($7::timestamptz IS NULL OR thread.created_at <= $7)
      -- $9 (threads only): is:dm keeps threads in DMs and groups.
      AND ('is:dm' <> ALL($9) OR space.direct)
    ORDER BY rank DESC, thread.created_at DESC
    LIMIT $8
"#;

const TASKS_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('english', $3) AS q)
    SELECT 'task' AS kind, item.id,
           project.project_key || '-' || item.number::text || ' ' || item.title AS title,
           status.name AS status,
           ts_headline('english', item.description, query.q,
                       'MaxWords=30, MinWords=8, StartSel=**, StopSel=**') AS snippet,
           item.project_id, project.organization_id AS workspace_id, project.name AS project_name,
           NULL::uuid AS conversation_id, NULL::text AS conversation_name,
           NULL::text AS author, item.updated_at::text AS at,
           'task' AS link_kind, item.id AS link_id,
           NULL::uuid AS link_child,
           (ts_rank(item.search_vector, query.q) * 1.3)::real AS rank
    FROM query, task_rows item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    -- A task's words are its record's and its description's, matched as one
    -- document (task_rows.search_vector joins the two), so a search whose
    -- words are split between the title and the description still finds it.
    WHERE item.search_vector @@ query.q
      AND item.project_id = ANY($2)
      AND item.archived_at IS NULL
      AND $4::uuid[] IS NULL
      AND ($5::uuid[] IS NULL OR item.assignee_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR item.updated_at >= $6)
      AND ($7::timestamptz IS NULL OR item.created_at <= $7)
      AND $1::uuid IS NOT NULL
    ORDER BY rank DESC
    LIMIT $8
"#;

const PAGES_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('english', $3) AS q)
    SELECT 'page' AS kind, page.id, page.title, NULL::text AS status,
           ts_headline('english', page.body, query.q,
                       'MaxWords=40, MinWords=12, StartSel=**, StopSel=**') AS snippet,
           page.project_id, project.organization_id AS workspace_id, project.name AS project_name,
           NULL::uuid AS conversation_id, NULL::text AS conversation_name,
           editor.display_name AS author, page.updated_at::text AS at,
           'page' AS link_kind, page.id AS link_id,
           NULL::uuid AS link_child,
           (ts_rank(page.search_vector, query.q) * 1.6)::real AS rank
    FROM query, project_pages page
    JOIN projects project ON project.id = page.project_id
    LEFT JOIN project_memberships editor
      ON editor.project_id = page.project_id AND editor.actor_id = page.updated_by_actor_id
    WHERE page.search_vector @@ query.q
      AND page.project_id = ANY($2)
      AND page.archived_at IS NULL
      AND $4::uuid[] IS NULL
      AND ($5::uuid[] IS NULL OR page.updated_by_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR page.updated_at >= $6)
      AND ($7::timestamptz IS NULL OR page.created_at <= $7)
      AND $1::uuid IS NOT NULL
    ORDER BY rank DESC
    LIMIT $8
"#;

const DECISIONS_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('english', $3) AS q)
    SELECT 'decision' AS kind, decision.id,
           decision.title AS title,
           CASE WHEN decision.status = 'superseded' THEN 'superseded' END AS status,
           left(decision.detail, 400) AS snippet,
           decision.project_id, project.organization_id AS workspace_id,
           CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END AS project_name,
           decision.source_conversation_id AS conversation_id, source.name AS conversation_name,
           decider.display_name AS author, decision.created_at::text AS at,
           'decision' AS link_kind,
           decision.id AS link_id, NULL::uuid AS link_child,
           (ts_rank(decision.search_vector, query.q)
               * CASE WHEN decision.status = 'active' THEN 2.0 ELSE 0.8 END)::real AS rank
    FROM query, project_decisions decision
    JOIN projects project ON project.id = decision.project_id
    LEFT JOIN conversations source ON source.id = decision.source_conversation_id
    LEFT JOIN project_memberships decider
      ON decider.project_id = decision.project_id AND decider.actor_id = decision.decided_by_actor_id
    WHERE decision.search_vector @@ query.q
      AND decision.project_id = ANY($2)
      AND (decision.source_conversation_id IS NULL
           OR conversation_visible_to(decision.source_conversation_id, $1))
      AND ($4::uuid[] IS NULL OR decision.source_conversation_id = ANY($4))
      AND ($5::uuid[] IS NULL OR decision.decided_by_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR decision.created_at >= $6)
      AND ($7::timestamptz IS NULL OR decision.created_at <= $7)
    ORDER BY rank DESC
    LIMIT $8
"#;

/// A collection's records (AWD-110), by title and text values, in
/// collections that aren't archived. The author filter is who changed it last.
const RECORDS_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('english', $3) AS q)
    SELECT 'record' AS kind, record.id,
           CASE WHEN record.title = '' THEN 'Untitled #' || record.number::text
                ELSE record.title END AS title,
           NULL::text AS status,
           'In ' || collection.name AS snippet,
           record.project_id, project.organization_id AS workspace_id, project.name AS project_name,
           NULL::uuid AS conversation_id, NULL::text AS conversation_name,
           editor.display_name AS author, record.updated_at::text AS at,
           'record' AS link_kind,
           record.collection_id AS link_id, record.id AS link_child,
           (ts_rank(record.search_vector, query.q) * 1.2)::real AS rank
    FROM query, collection_records record
    JOIN collections collection
      ON collection.project_id = record.project_id AND collection.id = record.collection_id
    JOIN projects project ON project.id = record.project_id
    LEFT JOIN project_memberships editor
      ON editor.project_id = record.project_id AND editor.actor_id = record.updated_by_actor_id
    WHERE record.search_vector @@ query.q
      AND record.project_id = ANY($2)
      AND record.archived_at IS NULL
      AND collection.archived_at IS NULL
      -- M1.6: a built module switched off hides its collections' records.
      AND module_part_on('collection', collection.id)
      -- A task's record (the built-in Tasks collection) is found as the task.
      AND NOT collection.acts_like_task
      AND $4::uuid[] IS NULL
      AND ($5::uuid[] IS NULL OR record.updated_by_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR record.updated_at >= $6)
      AND ($7::timestamptz IS NULL OR record.created_at <= $7)
      AND $1::uuid IS NOT NULL
    ORDER BY rank DESC
    LIMIT $8
"#;

/// Whiteboards by title and the text on them, in projects searched and,
/// as `$9`, the workspaces whose own boards show. Deleted boards aren't
/// found; each board passes its own rule.
const WHITEBOARDS_SQL: &str = r#"
    WITH query AS (
        SELECT websearch_to_tsquery('simple', $3) AS q,
               '%' || replace(replace(replace($3, '\', '\\'), '%', '\%'), '_', '\_') || '%'
                   AS pattern
    )
    SELECT 'whiteboard' AS kind, board.id, board.title, NULL::text AS status,
           CASE WHEN board.text_content = '' THEN ''
                ELSE ts_headline('simple', board.text_content, query.q,
                                 'MaxWords=30, MinWords=8, StartSel=**, StopSel=**') END AS snippet,
           board.project_id, board.organization_id AS workspace_id,
           COALESCE(project.name, 'Workspace') AS project_name,
           NULL::uuid AS conversation_id, NULL::text AS conversation_name,
           COALESCE(profile.display_name, account.display_name) AS author,
           board.updated_at::text AS at,
           'whiteboard' AS link_kind, board.id AS link_id, NULL::uuid AS link_child,
           (CASE WHEN board.search_vector @@ query.q
                 THEN ts_rank(board.search_vector, query.q) * 1.2 ELSE 0.05 END)::real AS rank
    FROM query, whiteboards board
    LEFT JOIN projects project ON project.id = board.project_id
    LEFT JOIN organization_actor_profiles profile
      ON profile.organization_id = board.organization_id
     AND profile.actor_id = COALESCE(board.updated_by_actor_id, board.created_by_actor_id)
    LEFT JOIN users account
      ON account.actor_id = COALESCE(board.updated_by_actor_id, board.created_by_actor_id)
    WHERE (board.search_vector @@ query.q OR board.title ILIKE query.pattern)
      AND board.deleted_at IS NULL
      AND (board.project_id = ANY($2)
           OR (board.project_id IS NULL AND board.organization_id = ANY($9)))
      AND whiteboard_visible_to(board.id, $1)
      AND $4::uuid[] IS NULL
      AND ($5::uuid[] IS NULL OR COALESCE(board.updated_by_actor_id, board.created_by_actor_id) = ANY($5))
      AND ($6::timestamptz IS NULL OR board.updated_at >= $6)
      AND ($7::timestamptz IS NULL OR board.created_at <= $7)
    ORDER BY rank DESC, board.updated_at DESC
    LIMIT $8
"#;

/// Sheets by name, description, tab names and text cells, in projects
/// where Sheets is on. Archived sheets aren't found.
const SHEETS_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('simple', $3) AS q)
    SELECT 'sheet' AS kind, sheet.id, sheet.name AS title, NULL::text AS status,
           CASE WHEN sheet.text_content = '' THEN sheet.description
                ELSE ts_headline('simple', sheet.text_content, query.q,
                                 'MaxWords=30, MinWords=8, StartSel=**, StopSel=**') END AS snippet,
           sheet.project_id, project.organization_id AS workspace_id, project.name AS project_name,
           NULL::uuid AS conversation_id, NULL::text AS conversation_name,
           editor.display_name AS author, sheet.updated_at::text AS at,
           'sheet' AS link_kind, sheet.id AS link_id, NULL::uuid AS link_child,
           (ts_rank(sheet.search_vector, query.q) * 1.2)::real AS rank
    FROM query, sheets sheet
    JOIN projects project ON project.id = sheet.project_id
    LEFT JOIN project_memberships editor
      ON editor.project_id = sheet.project_id AND editor.actor_id = sheet.updated_by_actor_id
    WHERE sheet.search_vector @@ query.q
      AND sheet.project_id = ANY($2)
      AND sheet.archived_at IS NULL
      AND $4::uuid[] IS NULL
      AND ($5::uuid[] IS NULL OR sheet.updated_by_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR sheet.updated_at >= $6)
      AND ($7::timestamptz IS NULL OR sheet.created_at <= $7)
      AND $1::uuid IS NOT NULL
    ORDER BY rank DESC
    LIMIT $8
"#;

/// Files by name, on messages the viewer can see and on tasks.
const FILES_SQL: &str = r#"
    WITH query AS (
        SELECT websearch_to_tsquery('simple', $3) AS q,
               '%' || replace(replace(replace($3, '\', '\\'), '%', '\%'), '_', '\_') || '%'
                   AS pattern
    )
    SELECT 'file' AS kind, file.id, file.name AS title, NULL::text AS status,
           CASE WHEN file.message_id IS NOT NULL THEN
                    'In ' || CASE WHEN conversation.kind = 'dm' THEN 'a DM' ELSE conversation.name END
                ELSE 'On ' || project.project_key || '-' || item.number::text || ' ' || item.title
           END AS snippet,
           file.project_id, project.organization_id AS workspace_id,
           CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END AS project_name,
           conversation.id AS conversation_id, conversation.name AS conversation_name,
           uploader.display_name AS author, file.created_at::text AS at,
           CASE WHEN file.message_id IS NULL THEN 'task'
                WHEN space.direct THEN 'direct'
                WHEN conversation.kind = 'thread' THEN 'thread' ELSE 'channel' END AS link_kind,
           CASE WHEN file.message_id IS NULL THEN file.work_item_id
                WHEN space.direct
                THEN COALESCE(conversation.parent_channel_id, conversation.id)
                ELSE conversation.id END AS link_id,
           file.message_id AS link_child,
           (CASE WHEN file.search_vector @@ query.q
                 THEN ts_rank(file.search_vector, query.q) * 1.2 ELSE 0.05 END)::real AS rank
    FROM query, files file
    JOIN projects project ON project.id = file.project_id
    CROSS JOIN LATERAL (SELECT project.kind = 'direct' AS direct) space
    LEFT JOIN conversations conversation ON conversation.id = file.conversation_id
    LEFT JOIN messages message ON message.id = file.message_id
    LEFT JOIN task_rows item
      ON item.project_id = file.project_id AND item.id = file.work_item_id
    LEFT JOIN project_memberships uploader
      ON uploader.project_id = file.project_id AND uploader.actor_id = file.uploaded_by_actor_id
    WHERE (file.search_vector @@ query.q OR file.name ILIKE query.pattern)
      AND file.project_id = ANY($2)
      AND file.attached_at IS NOT NULL
      AND file.removed_at IS NULL
      AND (file.conversation_id IS NULL OR (
          message.removed_at IS NULL
          AND conversation.archived_at IS NULL
          AND conversation_visible_to(conversation.id, $1)
      ))
      AND (file.work_item_id IS NULL OR item.archived_at IS NULL)
      -- $9 (files only): projects where the viewer still sees tasks, so a
      -- task's file never names a task they can't open.
      AND (file.work_item_id IS NULL OR file.project_id = ANY($9))
      -- $10 (files only): projects whose conversations show (Messages for DMs,
      -- Channels for a project), so a file never names a conversation whose
      -- module is off.
      AND (file.conversation_id IS NULL OR file.project_id = ANY($10))
      AND ($4::uuid[] IS NULL OR conversation.id = ANY($4) OR conversation.parent_channel_id = ANY($4))
      AND ($5::uuid[] IS NULL OR file.uploaded_by_actor_id = ANY($5))
      AND ($6::timestamptz IS NULL OR file.created_at >= $6)
      AND ($7::timestamptz IS NULL OR file.created_at <= $7)
      -- $11 (files only): is:thread and is:dm keep files sent there.
      AND ('is:thread' <> ALL($11) OR conversation.kind = 'thread')
      AND ('is:dm' <> ALL($11) OR (file.conversation_id IS NOT NULL AND space.direct))
    ORDER BY rank DESC, file.created_at DESC
    LIMIT $8
"#;

/// Every Project a person can search from the browser.
/// Of `projects`, those where the viewer's role (and, for an app acting for
/// them, its limit) still lets them see tasks.
pub async fn projects_showing_tasks(
    pool: &PgPool,
    viewer: Uuid,
    projects: &[Uuid],
) -> Result<Vec<Uuid>, AppError> {
    projects_allowing(pool, viewer, projects, crate::policy::Permission::TasksView).await
}

/// Of `projects`, those where the viewer's role (and, for an app acting for
/// them, its limit) keeps `permission`.
pub async fn projects_allowing(
    pool: &PgPool,
    viewer: Uuid,
    projects: &[Uuid],
    permission: crate::policy::Permission,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project_id FROM project_memberships
        WHERE actor_id = $1 AND project_id = ANY($2) AND enabled
          AND role_allows(role, role_id, $3)
        "#,
    )
    .bind(viewer)
    .bind(projects)
    .bind(permission.info().key)
    .fetch_all(crate::db::scoped(pool))
    .await?
    .into_iter()
    .filter(|project| !crate::policy::agent_limit_hides(viewer, *project, permission))
    .collect())
}

pub async fn browser_projects(pool: &PgPool, actor_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    reachable_projects(pool, actor_id, None).await
}

/// The spaces a caller reaches through the credential it called with: an
/// agent reaches by its connection, so its connection is passed here.
pub async fn reachable_projects(
    pool: &PgPool,
    actor_id: Uuid,
    connection_id: Option<Uuid>,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $1 AND membership.enabled
        WHERE project.archived_at IS NULL
          AND actor_can_access_project($1, $2, project.id)
        "#,
    )
    .bind(actor_id)
    .bind(connection_id)
    .fetch_all(crate::db::scoped(pool))
    .await?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchQuery {
    q: String,
    project_id: Option<Uuid>,
    kinds: Option<String>,
    limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResponse {
    hits: Vec<SearchHit>,
}

async fn search_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiQuery(query): ApiQuery<SearchQuery>,
) -> Result<Json<SearchResponse>, AppError> {
    let mut projects = browser_projects(&state.pool, principal.actor_id).await?;
    if let Some(project_id) = query.project_id {
        projects.retain(|id| *id == project_id);
    }
    let kinds = query
        .kinds
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .filter_map(SearchKind::parse)
        .collect::<Vec<_>>();
    if query.q.chars().count() > MAX_QUERY_CHARACTERS {
        return Err(AppError::Validation("q"));
    }
    let parsed = grammar::parse(&query.q);
    if !parsed.errors.is_empty() {
        return Err(AppError::Refused {
            field: "q",
            message: "A filter in the search takes other values. The filters are from:, in:, \
                      during:, on:, before:, after:, has:, is: and kind:.",
        });
    }
    let Some(resolved) = resolve(
        &state.pool,
        principal.actor_id,
        &projects,
        &parsed,
        &kinds,
        SearchFilter::default(),
    )
    .await?
    else {
        return Ok(Json(SearchResponse { hits: Vec::new() }));
    };
    if resolved.text.is_empty() && !resolved.filter.narrows() {
        return Err(AppError::Refused {
            field: "q",
            message: "Add words to search for, or a filter such as from: or in:.",
        });
    }
    let hits = search(
        &state.pool,
        Addresses::of(&state),
        principal.actor_id,
        &projects,
        &resolved.text,
        &resolved.kinds,
        &resolved.filter,
        query.limit.unwrap_or(30),
    )
    .await?;
    Ok(Json(SearchResponse { hits }))
}
