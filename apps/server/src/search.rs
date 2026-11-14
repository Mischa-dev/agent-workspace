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
    auth::AuthenticatedActor,
    error::{ApiQuery, AppError},
    modules::state::ProjectModules,
};

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
    pub conversation_id: Option<Uuid>,
    pub author_id: Option<Uuid>,
    pub since: Option<DateTime<Utc>>,
    pub until: Option<DateTime<Utc>>,
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
    pub path: String,
    #[serde(skip)]
    pub rank: f32,
}

pub async fn search(
    pool: &PgPool,
    viewer: Uuid,
    projects: &[Uuid],
    text: &str,
    kinds: &[SearchKind],
    filter: &SearchFilter,
    limit: i64,
) -> Result<Vec<SearchHit>, AppError> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > MAX_QUERY_CHARACTERS {
        return Err(AppError::Validation("query"));
    }
    let limit = limit.clamp(1, 100);
    let kinds = if kinds.is_empty() {
        &SearchKind::ALL[..]
    } else {
        kinds
    };
    // AWD-75: each kind searches only where its module is on.
    let modules = {
        let mut connection = pool.acquire().await?;
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
    let board_workspaces = if kinds.contains(&SearchKind::Whiteboards) {
        let mut connection = pool.acquire().await?;
        let on = crate::modules::state::by_workspace(&mut connection, viewer)
            .await?
            .into_iter()
            .filter(|(_, set)| set.contains("whiteboards"))
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
            SearchKind::Whiteboards => modules.with(projects, "whiteboards"),
            SearchKind::Sheets => modules.with(projects, "sheets"),
            // Files hang off messages and tasks.
            SearchKind::Files => modules.keep(projects, |set, direct| {
                set.allows_conversations_in(if direct { "direct" } else { "standard" })
                    || (!direct && set.contains("tasks"))
            }),
        };
        if projects.is_empty() && !(*kind == SearchKind::Whiteboards && !board_workspaces.is_empty())
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
            .bind(filter.conversation_id)
            .bind(filter.author_id)
            .bind(filter.since)
            .bind(filter.until)
            .bind(limit);
        if *kind == SearchKind::Files {
            query = query
                .bind(&task_projects[..])
                .bind(&conversation_projects[..]);
        }
        if *kind == SearchKind::Whiteboards {
            query = query.bind(&board_workspaces[..]);
        }
        let rows = query.fetch_all(pool).await?;
        hits.extend(rows);
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
// $1 viewer, $2 projects, $3 text, $4 conversation, $5 author, $6 since,
// $7 until, $8 limit. Unused ones are compared against NULL so the planner
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
           ts_headline('english', message.plain_text, query.q,
                       'MaxWords=40, MinWords=12, StartSel=**, StopSel=**') AS snippet,
           message.project_id, project.organization_id AS workspace_id,
           CASE WHEN project.kind = 'direct' THEN 'Messages' ELSE project.name END AS project_name,
           conversation.id AS conversation_id, conversation.name AS conversation_name,
           author.display_name AS author, message.created_at::text AS at,
           CASE WHEN project.kind = 'direct' THEN
                    '/messages/' || COALESCE(conversation.parent_channel_id, conversation.id)::text
                ELSE '/projects/' || message.project_id::text ||
                     CASE WHEN conversation.kind = 'thread' THEN '/threads/' ELSE '/channels/' END ||
                     conversation.id::text
           END AS path,
           (ts_rank(message.search_vector, query.q) * 1.0)::real AS rank
    FROM query, messages message
    JOIN conversations conversation ON conversation.id = message.conversation_id
    JOIN projects project ON project.id = message.project_id
    LEFT JOIN project_memberships author
      ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
    WHERE message.search_vector @@ query.q
      AND message.removed_at IS NULL
      AND message.project_id = ANY($2)
      AND conversation.archived_at IS NULL
      AND conversation_visible_to(conversation.id, $1)
      AND ($4::uuid IS NULL OR conversation.id = $4 OR conversation.parent_channel_id = $4)
      AND ($5::uuid IS NULL OR message.author_actor_id = $5)
      AND ($6::timestamptz IS NULL OR message.created_at >= $6)
      AND ($7::timestamptz IS NULL OR message.created_at <= $7)
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
           CASE WHEN project.kind = 'direct' THEN '/messages/' || parent.id::text
                ELSE '/projects/' || thread.project_id::text || '/threads/' || thread.id::text
           END AS path,
           (ts_rank(to_tsvector('english', thread.name || ' ' || COALESCE(thread.thread_outcome, '')),
                    query.q) * 1.5)::real AS rank
    FROM query, conversations thread
    JOIN conversations parent ON parent.id = thread.parent_channel_id
    JOIN projects project ON project.id = thread.project_id
    WHERE thread.kind = 'thread'
      AND to_tsvector('english', thread.name || ' ' || COALESCE(thread.thread_outcome, '')) @@ query.q
      AND thread.project_id = ANY($2)
      AND thread.archived_at IS NULL
      AND conversation_visible_to(thread.id, $1)
      AND ($4::uuid IS NULL OR parent.id = $4 OR thread.id = $4)
      AND ($5::uuid IS NULL OR thread.thread_status_changed_by_actor_id = $5)
      AND ($6::timestamptz IS NULL OR thread.created_at >= $6)
      AND ($7::timestamptz IS NULL OR thread.created_at <= $7)
    ORDER BY rank DESC
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
           '/projects/' || item.project_id::text || '/work-items/' || item.id::text AS path,
           (ts_rank(item.search_vector, query.q) * 1.3)::real AS rank
    FROM query, work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    WHERE item.search_vector @@ query.q
      AND item.project_id = ANY($2)
      AND item.archived_at IS NULL
      AND $4::uuid IS NULL
      AND ($5::uuid IS NULL OR item.assignee_actor_id = $5)
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
           '/projects/' || page.project_id::text || '/pages/' || page.id::text AS path,
           (ts_rank(page.search_vector, query.q) * 1.6)::real AS rank
    FROM query, project_pages page
    JOIN projects project ON project.id = page.project_id
    LEFT JOIN project_memberships editor
      ON editor.project_id = page.project_id AND editor.actor_id = page.updated_by_actor_id
    WHERE page.search_vector @@ query.q
      AND page.project_id = ANY($2)
      AND page.archived_at IS NULL
      AND $4::uuid IS NULL
      AND ($5::uuid IS NULL OR page.updated_by_actor_id = $5)
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
           '/projects/' || decision.project_id::text || '/decisions#' || decision.id::text AS path,
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
      AND ($4::uuid IS NULL OR decision.source_conversation_id = $4)
      AND ($5::uuid IS NULL OR decision.decided_by_actor_id = $5)
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
           '/projects/' || record.project_id::text || '/collections/' ||
               record.collection_id::text || '?record=' || record.id::text AS path,
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
      -- A task's record (the built-in Tasks collection) is found as the task.
      AND NOT collection.acts_like_task
      AND $4::uuid IS NULL
      AND ($5::uuid IS NULL OR record.updated_by_actor_id = $5)
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
           '/whiteboards/' || board.id::text AS path,
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
      AND $4::uuid IS NULL
      AND ($5::uuid IS NULL OR COALESCE(board.updated_by_actor_id, board.created_by_actor_id) = $5)
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
           '/projects/' || sheet.project_id::text || '/sheets/' || sheet.id::text AS path,
           (ts_rank(sheet.search_vector, query.q) * 1.2)::real AS rank
    FROM query, sheets sheet
    JOIN projects project ON project.id = sheet.project_id
    LEFT JOIN project_memberships editor
      ON editor.project_id = sheet.project_id AND editor.actor_id = sheet.updated_by_actor_id
    WHERE sheet.search_vector @@ query.q
      AND sheet.project_id = ANY($2)
      AND sheet.archived_at IS NULL
      AND $4::uuid IS NULL
      AND ($5::uuid IS NULL OR sheet.updated_by_actor_id = $5)
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
           CASE
               WHEN file.message_id IS NULL THEN
                    '/projects/' || file.project_id::text || '/work-items/' || file.work_item_id::text
               WHEN project.kind = 'direct' THEN
                    '/messages/' || COALESCE(conversation.parent_channel_id, conversation.id)::text
                    || '#message-' || file.message_id::text
               ELSE '/projects/' || file.project_id::text ||
                    CASE WHEN conversation.kind = 'thread' THEN '/threads/' ELSE '/channels/' END ||
                    conversation.id::text || '#message-' || file.message_id::text
           END AS path,
           (CASE WHEN file.search_vector @@ query.q
                 THEN ts_rank(file.search_vector, query.q) * 1.2 ELSE 0.05 END)::real AS rank
    FROM query, files file
    JOIN projects project ON project.id = file.project_id
    LEFT JOIN conversations conversation ON conversation.id = file.conversation_id
    LEFT JOIN messages message ON message.id = file.message_id
    LEFT JOIN work_items item
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
      AND ($4::uuid IS NULL OR conversation.id = $4 OR conversation.parent_channel_id = $4)
      AND ($5::uuid IS NULL OR file.uploaded_by_actor_id = $5)
      AND ($6::timestamptz IS NULL OR file.created_at >= $6)
      AND ($7::timestamptz IS NULL OR file.created_at <= $7)
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
          AND role = ANY($4) AND custom_role_allows(role_id, $3)
        "#,
    )
    .bind(viewer)
    .bind(projects)
    .bind(permission.info().key)
    .bind(crate::roles::base_roles_with(permission))
    .fetch_all(pool)
    .await?
    .into_iter()
    .filter(|project| !crate::policy::agent_limit_hides(viewer, *project, permission))
    .collect())
}

pub async fn browser_projects(pool: &PgPool, actor_id: Uuid) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $1 AND membership.enabled
        WHERE project.archived_at IS NULL
          AND actor_can_access_project($1, NULL, project.id)
        "#,
    )
    .bind(actor_id)
    .fetch_all(pool)
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
    let hits = search(
        &state.pool,
        principal.actor_id,
        &projects,
        &query.q,
        &kinds,
        &SearchFilter::default(),
        query.limit.unwrap_or(30),
    )
    .await?;
    Ok(Json(SearchResponse { hits }))
}
