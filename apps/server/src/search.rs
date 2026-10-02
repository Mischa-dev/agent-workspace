//! Full-text search across messages, thread outcomes, tasks, pages and
//! decisions. Results only ever include what the viewer can see: messages and
//! threads go through `conversation_visible_to`, and a decision from a private
//! conversation stays as private as it.
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
}

impl SearchKind {
    pub const ALL: [Self; 5] = [
        Self::Decisions,
        Self::Pages,
        Self::Threads,
        Self::Tasks,
        Self::Messages,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "message" | "messages" => Some(Self::Messages),
            "thread" | "threads" => Some(Self::Threads),
            "task" | "tasks" => Some(Self::Tasks),
            "page" | "pages" => Some(Self::Pages),
            "decision" | "decisions" => Some(Self::Decisions),
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
    pub snippet: String,
    pub project_id: Uuid,
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
    let mut hits = Vec::new();
    for kind in kinds {
        let sql = match kind {
            SearchKind::Messages => MESSAGES_SQL,
            SearchKind::Threads => THREADS_SQL,
            SearchKind::Tasks => TASKS_SQL,
            SearchKind::Pages => PAGES_SQL,
            SearchKind::Decisions => DECISIONS_SQL,
        };
        let rows = sqlx::query_as::<_, SearchHit>(sql)
            .bind(viewer)
            .bind(projects)
            .bind(text)
            .bind(filter.conversation_id)
            .bind(filter.author_id)
            .bind(filter.since)
            .bind(filter.until)
            .bind(limit)
            .fetch_all(pool)
            .await?;
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
// drops them.

const MESSAGES_SQL: &str = r#"
    WITH query AS (SELECT websearch_to_tsquery('english', $3) AS q)
    SELECT 'message' AS kind, message.id,
           COALESCE(author.display_name, 'Someone') || ' in ' ||
               CASE WHEN conversation.kind = 'dm' THEN 'a DM' ELSE conversation.name END AS title,
           ts_headline('english', message.plain_text, query.q,
                       'MaxWords=40, MinWords=12, StartSel=**, StopSel=**') AS snippet,
           message.project_id,
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
           thread.name || ' (' || thread.thread_status || ')' AS title,
           COALESCE(thread.thread_outcome, '') AS snippet,
           thread.project_id,
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
           project.project_key || '-' || item.number::text || ' ' || item.title
               || ' (' || status.name || ')' AS title,
           ts_headline('english', item.description, query.q,
                       'MaxWords=30, MinWords=8, StartSel=**, StopSel=**') AS snippet,
           item.project_id, project.name AS project_name,
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
    SELECT 'page' AS kind, page.id, page.title,
           ts_headline('english', page.body, query.q,
                       'MaxWords=40, MinWords=12, StartSel=**, StopSel=**') AS snippet,
           page.project_id, project.name AS project_name,
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
           decision.title || CASE WHEN decision.status = 'superseded' THEN ' (superseded)' ELSE '' END
               AS title,
           left(decision.detail, 400) AS snippet,
           decision.project_id,
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

/// Every Project a person can search from the browser.
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
