//! Project memory: pages with revision history, decisions, and pins.
//!
//! Pages hold what's true now ("Project guide", "Theming", "How we deploy")
//! and are edited in place; every save keeps the previous version. Decisions
//! record what was chosen and why, and a decision taken in a private
//! conversation is only visible to people who can see that conversation.
//! Pins put pages, decisions and messages at the top of a conversation.
//!
//! People (browser or connected app) and agents edit memory the same way.

use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::HeaderMap,
    routing::{delete, get, patch, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::{authorize_conversation_row_for, fetch_conversation_access, parse_actor_kind},
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    page_docs,
    policy::{
        Action, ActorKind, Decision, Membership, ProjectScope, Resource, RoleGrant, Subject,
        authorize,
    },
};

pub const MAX_PAGE_TITLE: usize = 200;
pub const MAX_PAGE_BODY: usize = 200_000;
pub const MAX_SKILL_DESCRIPTION: usize = 500;
pub const MAX_DECISION_TITLE: usize = 300;
pub const MAX_DECISION_DETAIL: usize = 20_000;
/// A page nobody has touched in this many days is flagged for review.
pub const STALE_AFTER_DAYS: i32 = 120;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{id}/pages",
            get(list_pages_http).post(create_page_http),
        )
        .route(
            "/api/v1/pages/{id}",
            get(get_page_http).patch(update_page_http),
        )
        .route(
            "/api/v1/pages/{id}/revisions/{version}",
            get(get_revision_http),
        )
        .route(
            "/api/v1/pages/{id}/export",
            get(page_docs::transfer::export_page),
        )
        .route(
            "/api/v1/projects/{id}/pages/import",
            post(page_docs::transfer::import_page)
                .layer(DefaultBodyLimit::max(page_docs::transfer::MAX_IMPORT_BYTES)),
        )
        .route(
            "/api/v1/pages/{id}/document",
            get(page_docs::http::get_document),
        )
        .route(
            "/api/v1/pages/{id}/document/sync",
            post(page_docs::http::sync_document),
        )
        .route(
            "/api/v1/pages/{id}/document/updates",
            post(page_docs::http::update_document).layer(DefaultBodyLimit::max(
                page_docs::http::MAX_UPDATE_REQUEST_BYTES,
            )),
        )
        .route(
            "/api/v1/pages/{id}/discussion",
            get(page_docs::discussion::get_discussion),
        )
        .route(
            "/api/v1/pages/{id}/comments",
            post(page_docs::discussion::create_comment),
        )
        .route(
            "/api/v1/pages/{id}/comments/{comment_id}",
            patch(page_docs::discussion::update_comment)
                .delete(page_docs::discussion::delete_comment),
        )
        .route(
            "/api/v1/pages/{id}/suggestions/{suggestion_id}/decision",
            post(page_docs::discussion::decide_suggestion),
        )
        .route(
            "/api/v1/projects/{id}/decisions",
            get(list_decisions_http).post(record_decision_http),
        )
        .route(
            "/api/v1/conversations/{id}/pins",
            get(list_pins_http).post(pin_http),
        )
        .route(
            "/api/v1/conversations/{id}/pins/{kind}/{target_id}",
            delete(unpin_http),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/claim",
            get(get_claim_http),
        )
}

/// Who is changing memory, and through which connected app if any.
#[derive(Clone, Copy, Debug)]
pub struct Writer {
    pub actor_id: Uuid,
    pub actor_kind: ActorKind,
    pub via_grant_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// Views

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageSummary {
    pub id: Uuid,
    pub title: String,
    pub is_guide: bool,
    /// Set when the page is a skill: the line that says when to use it.
    pub skill_description: Option<String>,
    pub version: i64,
    pub updated_at: String,
    pub updated_by: Option<String>,
    pub stale: bool,
    pub excerpt: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageView {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub body: String,
    pub is_guide: bool,
    pub skill_description: Option<String>,
    pub version: i64,
    pub created_at: String,
    pub updated_at: String,
    pub updated_by: Option<String>,
    pub archived: bool,
    pub stale: bool,
    #[sqlx(skip)]
    pub revisions: Vec<RevisionSummary>,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSummary {
    pub version: i64,
    pub title: String,
    pub author: Option<String>,
    pub via: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub session: RevisionSession,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionView {
    pub version: i64,
    pub title: String,
    pub body: String,
    pub author: Option<String>,
    pub via: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    #[sqlx(flatten)]
    #[serde(flatten)]
    pub session: RevisionSession,
}

/// The agent session a version was written in (AWD-53): what ran it, and
/// the conversation it works in when the viewer can see that conversation.
#[derive(Clone, Debug, Default, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_harness: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_run_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_conversation_id: Option<Uuid>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_conversation_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_conversation_kind: Option<String>,
}

/// Columns for [`RevisionSession`], with `$viewer` the viewer's actor id;
/// joins `session` and `session_conversation`.
const REVISION_SESSION_COLUMNS: &str = r#"
    session.harness AS session_harness,
    CASE WHEN session_conversation.id IS NOT NULL THEN session.agent_run_id END AS session_run_id,
    session_conversation.id AS session_conversation_id,
    session_conversation.name AS session_conversation_name,
    session_conversation.kind AS session_conversation_kind
"#;

fn revision_session_joins(viewer: &str) -> String {
    format!(
        r#"
        LEFT JOIN agent_sessions session ON session.id = revision.agent_session_id
        LEFT JOIN conversations session_conversation
          ON session_conversation.id = session.conversation_id
         AND conversation_visible_to(session.conversation_id, {viewer})
        "#
    )
}

/// The writer's agent session for a page's project: their active one that
/// moved most lately. `$page` and `$actor` are the bound parameters.
fn writer_session(page: &str, actor: &str) -> String {
    format!(
        r#"(SELECT session.id FROM agent_sessions session
            JOIN project_pages session_page ON session_page.id = {page}
            WHERE session.agent_actor_id = {actor}
              AND session.project_id = session_page.project_id
              AND session.state = 'active'
            ORDER BY session.last_activity_at DESC, session.id
            LIMIT 1)"#
    )
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionView {
    pub id: Uuid,
    pub project_id: Uuid,
    pub title: String,
    pub detail: String,
    pub status: String,
    pub superseded_by_decision_id: Option<Uuid>,
    pub source_conversation_id: Option<Uuid>,
    pub source_conversation_name: Option<String>,
    /// channel, thread, dm or group: how the app links to the source.
    pub source_conversation_kind: Option<String>,
    pub source_message_id: Option<Uuid>,
    pub decided_by: String,
    pub via: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinView {
    pub kind: String,
    pub id: Uuid,
    pub title: String,
    pub text: String,
    pub pinned_at: String,
}

// ---------------------------------------------------------------------------
// Access

#[derive(Debug, FromRow)]
struct ProjectAccessRow {
    role: String,
    role_permissions: Option<Vec<String>>,
    enabled: bool,
    archived: bool,
    kind: String,
}

/// Check that an actor may take an action in a Project. Returns whether the
/// Project is the hidden Messages space, where pages aren't offered.
pub async fn project_access(
    executor: impl sqlx::PgExecutor<'_>,
    actor_id: Uuid,
    actor_kind: ActorKind,
    project_id: Uuid,
    action: Action,
) -> Result<bool, AppError> {
    let row = sqlx::query_as::<_, ProjectAccessRow>(
        r#"
        SELECT membership.role, workspace_role_permissions(membership.role_id) AS role_permissions,
               membership.enabled,
               project.archived_at IS NOT NULL AS archived, project.kind
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $2
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        JOIN actors actor ON actor.id = $2 AND actor.status = 'active'
        WHERE project.id = $1
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(executor)
    .await?
    .ok_or(AppError::NotFound("project"))?;
    if row.archived {
        return Err(AppError::NotFound("project"));
    }
    let subject = Subject::new(
        actor_id,
        actor_kind,
        true,
        Some(Membership::project(
            project_id,
            RoleGrant::stored(&row.role, row.role_permissions.as_deref())?,
            row.enabled,
        )),
        None,
    );
    if !matches!(
        authorize(
            &subject,
            action,
            Resource::Project(ProjectScope::project(project_id))
        ),
        Decision::Allow
    ) {
        return Err(AppError::NotFound("project"));
    }
    Ok(row.kind == "direct")
}

// ---------------------------------------------------------------------------
// Pages

pub async fn list_pages(
    pool: &PgPool,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<PageSummary>, AppError> {
    Ok(sqlx::query_as::<_, PageSummary>(
        r#"
        SELECT page.id, page.title, page.is_guide, page.skill_description, page.version,
               page.updated_at::text AS updated_at,
               editor.display_name AS updated_by,
               page.updated_at < now() - make_interval(days => $3) AS stale,
               left(regexp_replace(page.body, '\s+', ' ', 'g'), 240) AS excerpt
        FROM project_pages page
        LEFT JOIN project_memberships editor
          ON editor.project_id = page.project_id AND editor.actor_id = page.updated_by_actor_id
        WHERE page.project_id = $1 AND ($2 OR page.archived_at IS NULL)
        ORDER BY page.is_guide DESC, page.updated_at DESC, page.id
        LIMIT 500
        "#,
    )
    .bind(project_id)
    .bind(include_archived)
    .bind(STALE_AFTER_DAYS)
    .fetch_all(pool)
    .await?)
}

const PAGE_SELECT: &str = r#"
    SELECT page.id, page.project_id, page.title, page.body, page.is_guide,
           page.skill_description, page.version,
           page.created_at::text AS created_at, page.updated_at::text AS updated_at,
           editor.display_name AS updated_by,
           page.archived_at IS NOT NULL AS archived,
           page.updated_at < now() - make_interval(days => $2) AS stale
    FROM project_pages page
    LEFT JOIN project_memberships editor
      ON editor.project_id = page.project_id AND editor.actor_id = page.updated_by_actor_id
    WHERE page.id = $1
"#;

/// A page with its latest versions; `viewer` decides which agent sessions'
/// conversations the history may name.
pub async fn get_page(
    pool: &PgPool,
    page_id: Uuid,
    viewer: Uuid,
) -> Result<Option<PageView>, AppError> {
    let Some(mut page) = sqlx::query_as::<_, PageView>(PAGE_SELECT)
        .bind(page_id)
        .bind(STALE_AFTER_DAYS)
        .fetch_optional(pool)
        .await?
    else {
        return Ok(None);
    };
    page.revisions = sqlx::query_as::<_, RevisionSummary>(&format!(
        r#"
        SELECT revision.version, revision.title, author.display_name AS author,
               via_client.client_name AS via, revision.note,
               revision.created_at::text AS created_at,
               {REVISION_SESSION_COLUMNS}
        FROM project_page_revisions revision
        JOIN project_pages page ON page.id = revision.page_id
        LEFT JOIN project_memberships author
          ON author.project_id = page.project_id AND author.actor_id = revision.actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = revision.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        {joins}
        WHERE revision.page_id = $1
        ORDER BY revision.version DESC
        LIMIT 50
        "#,
        joins = revision_session_joins("$2"),
    ))
    .bind(page_id)
    .bind(viewer)
    .fetch_all(pool)
    .await?;
    Ok(Some(page))
}

/// The guide page for a Project, if it has one.
pub async fn guide_page(
    pool: &PgPool,
    project_id: Uuid,
    viewer: Uuid,
) -> Result<Option<PageView>, AppError> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM project_pages WHERE project_id = $1 AND is_guide AND archived_at IS NULL",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await?;
    match id {
        Some(id) => get_page(pool, id, viewer).await,
        None => Ok(None),
    }
}

/// A Project's skills, by title. Agents read the descriptions and load a
/// skill's page only when it applies.
pub async fn list_skills(pool: &PgPool, project_id: Uuid) -> Result<Vec<PageSummary>, AppError> {
    Ok(sqlx::query_as::<_, PageSummary>(
        r#"
        SELECT page.id, page.title, page.is_guide, page.skill_description, page.version,
               page.updated_at::text AS updated_at,
               editor.display_name AS updated_by,
               page.updated_at < now() - make_interval(days => $2) AS stale,
               left(regexp_replace(page.body, '\s+', ' ', 'g'), 240) AS excerpt
        FROM project_pages page
        LEFT JOIN project_memberships editor
          ON editor.project_id = page.project_id AND editor.actor_id = page.updated_by_actor_id
        WHERE page.project_id = $1 AND page.skill_description IS NOT NULL
          AND page.archived_at IS NULL
        ORDER BY lower(page.title), page.id
        LIMIT 200
        "#,
    )
    .bind(project_id)
    .bind(STALE_AFTER_DAYS)
    .fetch_all(pool)
    .await?)
}

pub async fn get_revision(
    pool: &PgPool,
    page_id: Uuid,
    version: i64,
    viewer: Uuid,
) -> Result<Option<RevisionView>, AppError> {
    Ok(sqlx::query_as::<_, RevisionView>(&format!(
        r#"
        SELECT revision.version, revision.title, revision.body,
               author.display_name AS author, via_client.client_name AS via,
               revision.note, revision.created_at::text AS created_at,
               {REVISION_SESSION_COLUMNS}
        FROM project_page_revisions revision
        JOIN project_pages page ON page.id = revision.page_id
        LEFT JOIN project_memberships author
          ON author.project_id = page.project_id AND author.actor_id = revision.actor_id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = revision.via_oauth_grant_id AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        {joins}
        WHERE revision.page_id = $1 AND revision.version = $2
        "#,
        joins = revision_session_joins("$3"),
    ))
    .bind(page_id)
    .bind(version)
    .bind(viewer)
    .fetch_optional(pool)
    .await?)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageInput {
    pub title: Option<String>,
    pub body: Option<String>,
    pub is_guide: Option<bool>,
    /// Make the page a skill with this one-line description of when to use
    /// it; an empty string makes it a plain page again.
    pub skill_description: Option<String>,
    /// Refuse the save if someone else changed the page since this version.
    pub expected_version: Option<i64>,
    /// Put back the title and body saved in this version.
    pub restore_version: Option<i64>,
    /// Archive (true) or restore (false) the page.
    pub archived: Option<bool>,
    /// A short note on what changed.
    pub note: Option<String>,
}

fn normalize_title(value: &str, max: usize) -> Result<String, AppError> {
    let title = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() || title.chars().count() > max || title.chars().any(char::is_control) {
        return Err(AppError::Validation("title"));
    }
    Ok(title)
}

pub(crate) fn normalize_body(value: &str, max: usize) -> Result<String, AppError> {
    if value.chars().count() > max || value.contains('\0') {
        return Err(AppError::Validation("body"));
    }
    Ok(value.to_owned())
}

/// A skill description is one line. `Some(None)` turns the skill off.
fn normalize_skill_description(value: Option<&str>) -> Result<Option<Option<String>>, AppError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let description = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if description.is_empty() {
        return Ok(Some(None));
    }
    if description.chars().count() > MAX_SKILL_DESCRIPTION {
        return Err(AppError::Validation("skillDescription"));
    }
    Ok(Some(Some(description)))
}

/// The name an agent calls a skill by: its page title in lowercase words
/// joined by hyphens, as Claude Code and Codex name skill folders.
pub fn skill_name(title: &str) -> String {
    let mut name = String::new();
    for character in title.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            name.push(character);
        } else if !name.is_empty() && !name.ends_with('-') {
            name.push('-');
        }
    }
    while name.ends_with('-') {
        name.pop();
    }
    name.truncate(
        name.char_indices()
            .nth(64)
            .map_or(name.len(), |(index, _)| index),
    );
    let name = name.trim_end_matches('-');
    // A title of only symbols or emoji still needs a name harnesses accept.
    if name.is_empty() {
        "skill".to_owned()
    } else {
        name.to_owned()
    }
}

pub(crate) fn normalize_note(value: Option<&str>) -> Result<Option<String>, AppError> {
    match value.map(str::trim).filter(|value| !value.is_empty()) {
        Some(note) if note.chars().count() <= 500 => Ok(Some(note.to_owned())),
        Some(_) => Err(AppError::Validation("note")),
        None => Ok(None),
    }
}

fn map_page_title_conflict(error: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(database) = &error
        && database.constraint() == Some("project_pages_active_title")
    {
        return AppError::Conflict("page_title", "A page with that title already exists.");
    }
    error.into()
}

/// Create a page in a Project. The caller has checked access.
pub async fn create_page(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    project_id: Uuid,
    input: &PageInput,
) -> Result<Uuid, AppError> {
    let title = normalize_title(
        input
            .title
            .as_deref()
            .ok_or(AppError::Validation("title"))?,
        MAX_PAGE_TITLE,
    )?;
    let body = normalize_body(input.body.as_deref().unwrap_or_default(), MAX_PAGE_BODY)?;
    let note = normalize_note(input.note.as_deref())?;
    let is_guide = input.is_guide.unwrap_or(false);
    let skill_description =
        normalize_skill_description(input.skill_description.as_deref())?.flatten();
    if is_guide {
        clear_guide(transaction, writer, project_id, None).await?;
    }
    let page_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO project_pages (
            id, project_id, title, body, is_guide, skill_description,
            created_by_actor_id, updated_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $7)
        "#,
    )
    .bind(page_id)
    .bind(project_id)
    .bind(&title)
    .bind(&body)
    .bind(is_guide)
    .bind(&skill_description)
    .bind(writer.actor_id)
    .execute(&mut **transaction)
    .await
    .map_err(map_page_title_conflict)?;
    insert_revision(
        transaction,
        writer,
        page_id,
        1,
        &title,
        &body,
        note.as_deref(),
    )
    .await?;
    page_docs::store::page_updated(transaction, project_id, page_id, 1).await?;
    Ok(page_id)
}

/// Change a page: edit it, restore an earlier version, or archive it. Every
/// change saves a new version.
pub async fn update_page(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    page_id: Uuid,
    input: &PageInput,
) -> Result<Uuid, AppError> {
    let (project_id, title, body, version, is_guide, skill_description, archived) =
        sqlx::query_as::<_, (Uuid, String, String, i64, bool, Option<String>, bool)>(
            r#"
            SELECT project_id, title, body, version, is_guide, skill_description,
                   archived_at IS NOT NULL
            FROM project_pages WHERE id = $1 FOR UPDATE
            "#,
        )
        .bind(page_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("page"))?;
    if let Some(expected) = input.expected_version
        && expected != version
    {
        return Err(AppError::VersionConflict {
            resource: "page",
            current_version: version,
        });
    }
    let (mut new_title, mut new_body) = (title.clone(), body.clone());
    let mut note = normalize_note(input.note.as_deref())?;
    if let Some(restore) = input.restore_version {
        let revision = sqlx::query_as::<_, (String, String)>(
            "SELECT title, body FROM project_page_revisions WHERE page_id = $1 AND version = $2",
        )
        .bind(page_id)
        .bind(restore)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("page version"))?;
        new_title = revision.0;
        new_body = revision.1;
        note.get_or_insert_with(|| format!("Restored version {restore}"));
    }
    if let Some(value) = input.title.as_deref() {
        new_title = normalize_title(value, MAX_PAGE_TITLE)?;
    }
    if let Some(value) = input.body.as_deref() {
        new_body = normalize_body(value, MAX_PAGE_BODY)?;
    }
    // A page with a stored document changes through it, and its Markdown is
    // derived from the result.
    if new_body != body
        && let Some(derived) =
            page_docs::store::set_markdown(transaction, page_id, &new_body).await?
    {
        if derived.chars().count() > MAX_PAGE_BODY {
            return Err(page_docs::store::too_long());
        }
        new_body = derived;
    }
    let new_guide = input.is_guide.unwrap_or(is_guide);
    let new_skill = normalize_skill_description(input.skill_description.as_deref())?
        .unwrap_or_else(|| skill_description.clone());
    let new_archived = input.archived.unwrap_or(archived);
    if new_title == title
        && new_body == body
        && new_guide == is_guide
        && new_skill == skill_description
        && new_archived == archived
    {
        return Ok(project_id);
    }
    if new_skill.is_some() != skill_description.is_some() {
        note.get_or_insert_with(|| {
            if new_skill.is_some() {
                "Made a skill"
            } else {
                "No longer a skill"
            }
            .to_owned()
        });
    }
    if new_guide && !is_guide {
        clear_guide(transaction, writer, project_id, Some(page_id)).await?;
    }
    if new_archived != archived {
        note.get_or_insert_with(|| if new_archived { "Archived" } else { "Restored" }.to_owned());
    }
    let next_version = version + 1;
    sqlx::query(
        r#"
        UPDATE project_pages
        SET title = $2, body = $3, is_guide = $4,
            archived_at = CASE WHEN $5 THEN COALESCE(archived_at, now()) ELSE NULL END,
            version = $6, updated_by_actor_id = $7, updated_at = now(),
            skill_description = $8
        WHERE id = $1
        "#,
    )
    .bind(page_id)
    .bind(&new_title)
    .bind(&new_body)
    .bind(new_guide && !new_archived)
    .bind(new_archived)
    .bind(next_version)
    .bind(writer.actor_id)
    .bind(&new_skill)
    .execute(&mut **transaction)
    .await
    .map_err(map_page_title_conflict)?;
    insert_revision(
        transaction,
        writer,
        page_id,
        next_version,
        &new_title,
        &new_body,
        note.as_deref(),
    )
    .await?;
    page_docs::store::page_updated(transaction, project_id, page_id, next_version).await?;
    Ok(project_id)
}

/// Stop the current guide being the guide. That is a change to the page, so
/// it gets a new version like any other.
async fn clear_guide(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    project_id: Uuid,
    except: Option<Uuid>,
) -> Result<(), AppError> {
    let demoted = sqlx::query_as::<_, (Uuid, i64, String, String)>(
        r#"
        UPDATE project_pages
        SET is_guide = false, version = version + 1,
            updated_by_actor_id = $3, updated_at = now()
        WHERE project_id = $1 AND is_guide AND ($2::uuid IS NULL OR id <> $2)
        RETURNING id, version, title, body
        "#,
    )
    .bind(project_id)
    .bind(except)
    .bind(writer.actor_id)
    .fetch_all(&mut **transaction)
    .await?;
    for (page_id, version, title, body) in demoted {
        insert_revision(
            transaction,
            writer,
            page_id,
            version,
            &title,
            &body,
            Some("No longer the guide"),
        )
        .await?;
        page_docs::store::page_updated(transaction, project_id, page_id, version).await?;
    }
    Ok(())
}

/// Records a version in a page's history. With `merge_minutes`, a change
/// with no note goes into the latest version instead, when that's the same
/// writer's (through the same app) from less than that many minutes ago:
/// live editing saves every few seconds, and its history is kept by the
/// stretch, not by the keystroke. Returns whether a new version was added.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn record_revision(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    page_id: Uuid,
    version: i64,
    title: &str,
    body: &str,
    note: Option<&str>,
    merge_minutes: Option<i32>,
) -> Result<bool, AppError> {
    if let Some(minutes) = merge_minutes
        && note.is_none()
    {
        let merged = sqlx::query(&format!(
            r#"
            UPDATE project_page_revisions revision
            SET version = $2, title = $3, body = $4
            WHERE revision.page_id = $1
              AND revision.version = (
                  SELECT max(latest.version) FROM project_page_revisions latest
                  WHERE latest.page_id = $1
              )
              AND revision.actor_id = $5
              AND revision.via_oauth_grant_id IS NOT DISTINCT FROM $6
              AND revision.agent_session_id IS NOT DISTINCT FROM {session}
              AND revision.note IS NULL
              AND revision.created_at > now() - make_interval(mins => $7)
            "#,
            session = writer_session("$1", "$5"),
        ))
        .bind(page_id)
        .bind(version)
        .bind(title)
        .bind(body)
        .bind(writer.actor_id)
        .bind(writer.via_grant_id)
        .bind(minutes)
        .execute(&mut **transaction)
        .await?
        .rows_affected();
        if merged == 1 {
            return Ok(false);
        }
    }
    insert_revision(transaction, writer, page_id, version, title, body, note).await?;
    Ok(true)
}

async fn insert_revision(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    page_id: Uuid,
    version: i64,
    title: &str,
    body: &str,
    note: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query(&format!(
        r#"
        INSERT INTO project_page_revisions (
            page_id, version, title, body, actor_id, via_oauth_grant_id, note, agent_session_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, {session})
        "#,
        session = writer_session("$1", "$5"),
    ))
    .bind(page_id)
    .bind(version)
    .bind(title)
    .bind(body)
    .bind(writer.actor_id)
    .bind(writer.via_grant_id)
    .bind(note)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Decisions

const DECISION_SELECT: &str = r#"
    SELECT decision.id, decision.project_id, decision.title, decision.detail, decision.status,
           decision.superseded_by_decision_id, decision.source_conversation_id,
           source.name AS source_conversation_name, source.kind AS source_conversation_kind,
           decision.source_message_id,
           COALESCE(decider.display_name, 'Former member') AS decided_by,
           via_client.client_name AS via, decision.created_at::text AS created_at
    FROM project_decisions decision
    LEFT JOIN conversations source ON source.id = decision.source_conversation_id
    LEFT JOIN project_memberships decider
      ON decider.project_id = decision.project_id AND decider.actor_id = decision.decided_by_actor_id
    LEFT JOIN mcp_oauth_grants via_grant
      ON via_grant.id = decision.via_oauth_grant_id AND via_grant.show_via
    LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
"#;

/// Decisions in a Project the viewer may see, newest first.
pub async fn list_decisions(
    pool: &PgPool,
    project_id: Uuid,
    viewer: Uuid,
    include_superseded: bool,
    limit: i64,
) -> Result<Vec<DecisionView>, AppError> {
    Ok(sqlx::query_as::<_, DecisionView>(&format!(
        r#"{DECISION_SELECT}
        WHERE decision.project_id = $1
          AND ($3 OR decision.status = 'active')
          AND (decision.source_conversation_id IS NULL
               OR conversation_visible_to(decision.source_conversation_id, $2))
        ORDER BY decision.created_at DESC, decision.id
        LIMIT $4
        "#
    ))
    .bind(project_id)
    .bind(viewer)
    .bind(include_superseded)
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

pub async fn get_decision(
    pool: &PgPool,
    decision_id: Uuid,
    viewer: Uuid,
) -> Result<Option<DecisionView>, AppError> {
    Ok(sqlx::query_as::<_, DecisionView>(&format!(
        r#"{DECISION_SELECT}
        WHERE decision.id = $1
          AND (decision.source_conversation_id IS NULL
               OR conversation_visible_to(decision.source_conversation_id, $2))
        "#
    ))
    .bind(decision_id)
    .bind(viewer)
    .fetch_optional(pool)
    .await?)
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecisionInput {
    pub title: String,
    pub detail: Option<String>,
    /// The message where it was decided.
    pub source_message_id: Option<Uuid>,
    /// The conversation where it was decided.
    pub source_conversation_id: Option<Uuid>,
    /// An earlier decision this one replaces.
    pub supersedes: Option<Uuid>,
}

/// Record a decision in a Project. A source message or conversation must be
/// one the writer can see, and in the same Project.
pub async fn record_decision(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    project_id: Uuid,
    input: &DecisionInput,
) -> Result<Uuid, AppError> {
    let title = normalize_title(&input.title, MAX_DECISION_TITLE)?;
    let detail = normalize_body(
        input.detail.as_deref().unwrap_or_default(),
        MAX_DECISION_DETAIL,
    )?;
    let mut source_conversation_id = input.source_conversation_id;
    if let Some(message_id) = input.source_message_id {
        // A removed message can't be a decision's source: not found.
        let conversation_id = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT conversation_id FROM messages
            WHERE id = $1 AND project_id = $2 AND removed_at IS NULL
              AND conversation_visible_to(conversation_id, $3)
            FOR SHARE
            "#,
        )
        .bind(message_id)
        .bind(project_id)
        .bind(writer.actor_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("message"))?;
        if source_conversation_id.is_some_and(|id| id != conversation_id) {
            return Err(AppError::Validation("sourceConversationId"));
        }
        source_conversation_id = Some(conversation_id);
    } else if let Some(conversation_id) = source_conversation_id {
        let visible = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND project_id = $2 AND conversation_visible_to(id, $3))",
        )
        .bind(conversation_id)
        .bind(project_id)
        .bind(writer.actor_id)
        .fetch_one(&mut **transaction)
        .await?;
        if !visible {
            return Err(AppError::NotFound("conversation"));
        }
    }
    let decision_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO project_decisions (
            id, project_id, title, detail, source_conversation_id, source_message_id,
            decided_by_actor_id, via_oauth_grant_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(decision_id)
    .bind(project_id)
    .bind(&title)
    .bind(&detail)
    .bind(source_conversation_id)
    .bind(input.source_message_id)
    .bind(writer.actor_id)
    .bind(writer.via_grant_id)
    .execute(&mut **transaction)
    .await?;
    if let Some(old) = input.supersedes {
        let updated = sqlx::query(
            r#"
            UPDATE project_decisions
            SET status = 'superseded', superseded_by_decision_id = $2, updated_at = now()
            WHERE id = $1 AND project_id = $3 AND status = 'active'
              AND (source_conversation_id IS NULL
                   OR conversation_visible_to(source_conversation_id, $4))
            "#,
        )
        .bind(old)
        .bind(decision_id)
        .bind(project_id)
        .bind(writer.actor_id)
        .execute(&mut **transaction)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(AppError::NotFound("decision to supersede"));
        }
    }
    Ok(decision_id)
}

// ---------------------------------------------------------------------------
// Pins

pub async fn list_pins(
    pool: &PgPool,
    conversation_id: Uuid,
    viewer: Uuid,
) -> Result<Vec<PinView>, AppError> {
    Ok(sqlx::query_as::<_, PinView>(
        r#"
        SELECT pin.target_kind AS kind, pin.target_id AS id,
               COALESCE(page.title, decision.title,
                        'Message from ' || COALESCE(author.display_name, 'someone')) AS title,
               left(COALESCE(page.body, decision.detail, message.plain_text, ''), 600) AS text,
               pin.created_at::text AS pinned_at
        FROM conversation_pins pin
        LEFT JOIN project_pages page
          ON pin.target_kind = 'page' AND page.id = pin.target_id AND page.archived_at IS NULL
        LEFT JOIN project_decisions decision
          ON pin.target_kind = 'decision' AND decision.id = pin.target_id
         AND decision.status = 'active'
         AND (decision.source_conversation_id IS NULL
              OR conversation_visible_to(decision.source_conversation_id, $2))
        LEFT JOIN messages message
          ON pin.target_kind = 'message' AND message.id = pin.target_id
         AND message.removed_at IS NULL
         AND conversation_visible_to(message.conversation_id, $2)
        LEFT JOIN project_memberships author
          ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
        WHERE pin.conversation_id = $1
          AND (page.id IS NOT NULL OR decision.id IS NOT NULL OR message.id IS NOT NULL)
        ORDER BY pin.created_at
        LIMIT 50
        "#,
    )
    .bind(conversation_id)
    .bind(viewer)
    .fetch_all(pool)
    .await?)
}

/// Pin or unpin a page, decision, or message on a conversation. The
/// conversation must be one the writer can send messages in, and the target
/// must belong to the same Project.
pub async fn set_pin(
    transaction: &mut Transaction<'_, Postgres>,
    writer: Writer,
    conversation_id: Uuid,
    kind: &str,
    target_id: Uuid,
    pinned: bool,
) -> Result<Uuid, AppError> {
    let conversation = crate::conversations::lock_conversation_for(
        transaction,
        writer.actor_id,
        writer.actor_kind,
        conversation_id,
        Action::MessageSend,
        false,
    )
    .await?;
    if !pinned {
        sqlx::query(
            "DELETE FROM conversation_pins WHERE conversation_id = $1 AND target_kind = $2 AND target_id = $3",
        )
        .bind(conversation_id)
        .bind(kind)
        .bind(target_id)
        .execute(&mut **transaction)
        .await?;
        return Ok(conversation.project_id);
    }
    let exists = match kind {
        "page" => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM project_pages WHERE id = $1 AND project_id = $2 AND archived_at IS NULL)",
            )
            .bind(target_id)
            .bind(conversation.project_id)
            .fetch_one(&mut **transaction)
            .await?
        }
        "decision" => {
            sqlx::query_scalar::<_, bool>(
                r#"SELECT EXISTS (SELECT 1 FROM project_decisions WHERE id = $1 AND project_id = $2
                   AND (source_conversation_id IS NULL OR conversation_visible_to(source_conversation_id, $3)))"#,
            )
            .bind(target_id)
            .bind(conversation.project_id)
            .bind(writer.actor_id)
            .fetch_one(&mut **transaction)
            .await?
        }
        // A removed message can't be pinned. The share lock waits out a
        // removal in progress, which then sees (and deletes) this pin, or
        // sees the message removed.
        "message" => sqlx::query_scalar::<_, i32>(
            r#"
            SELECT 1 FROM messages
            WHERE id = $1 AND project_id = $2 AND removed_at IS NULL
              AND conversation_visible_to(conversation_id, $3)
            FOR SHARE
            "#,
        )
        .bind(target_id)
        .bind(conversation.project_id)
        .bind(writer.actor_id)
        .fetch_optional(&mut **transaction)
        .await?
        .is_some(),
        _ => return Err(AppError::Validation("kind")),
    };
    if !exists {
        return Err(AppError::NotFound("pin target"));
    }
    sqlx::query(
        r#"
        INSERT INTO conversation_pins (conversation_id, target_kind, target_id, pinned_by_actor_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT DO NOTHING
        "#,
    )
    .bind(conversation_id)
    .bind(kind)
    .bind(target_id)
    .bind(writer.actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(conversation.project_id)
}

// ---------------------------------------------------------------------------
// Browser API

pub(crate) fn browser_writer(principal: &AuthenticatedActor) -> Result<Writer, AppError> {
    Ok(Writer {
        actor_id: principal.actor_id,
        actor_kind: parse_actor_kind(&principal.actor_kind)?,
        via_grant_id: None,
    })
}

pub(crate) async fn audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: &str,
    target_kind: &str,
    target_id: Uuid,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, metadata
        ) VALUES ($1, $2, $3, $4, 'password_session', $5, $6, $7, 'succeeded', '{}'::jsonb)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.session_id)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ListQuery {
    include_archived: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PagesResponse {
    pages: Vec<PageSummary>,
    can_edit: bool,
}

async fn list_pages_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<ListQuery>,
) -> Result<Json<PagesResponse>, AppError> {
    let kind = parse_actor_kind(&principal.actor_kind)?;
    project_access(
        &state.pool,
        principal.actor_id,
        kind,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let can_edit = project_access(
        &state.pool,
        principal.actor_id,
        kind,
        project_id,
        Action::PageEdit,
    )
    .await
    .is_ok();
    Ok(Json(PagesResponse {
        pages: list_pages(
            &state.pool,
            project_id,
            query.include_archived.unwrap_or(false),
        )
        .await?,
        can_edit,
    }))
}

async fn create_page_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<PageInput>,
) -> Result<Json<PageView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let writer = browser_writer(&principal)?;
    let mut transaction = state.pool.begin().await?;
    let direct = project_access(
        &mut *transaction,
        writer.actor_id,
        writer.actor_kind,
        project_id,
        Action::PageEdit,
    )
    .await?;
    if direct {
        return Err(AppError::Validation("project"));
    }
    let page_id = create_page(&mut transaction, writer, project_id, &input).await?;
    audit(
        &mut transaction,
        &principal,
        project_id,
        "page.created",
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    get_page(&state.pool, page_id, principal.actor_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("page"))
}

pub(crate) async fn page_project(pool: &PgPool, page_id: Uuid) -> Result<Uuid, AppError> {
    sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM project_pages WHERE id = $1")
        .bind(page_id)
        .fetch_optional(pool)
        .await?
        .ok_or(AppError::NotFound("page"))
}

async fn get_page_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
) -> Result<Json<PageView>, AppError> {
    let project_id = page_project(&state.pool, page_id).await?;
    project_access(
        &state.pool,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        project_id,
        Action::ProjectView,
    )
    .await
    .map_err(|_| AppError::NotFound("page"))?;
    get_page(&state.pool, page_id, principal.actor_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("page"))
}

async fn update_page_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(page_id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<PageInput>,
) -> Result<Json<PageView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let writer = browser_writer(&principal)?;
    let project_id = page_project(&state.pool, page_id).await?;
    let mut transaction = state.pool.begin().await?;
    project_access(
        &mut *transaction,
        writer.actor_id,
        writer.actor_kind,
        project_id,
        Action::PageEdit,
    )
    .await
    .map_err(|_| AppError::NotFound("page"))?;
    update_page(&mut transaction, writer, page_id, &input).await?;
    audit(
        &mut transaction,
        &principal,
        project_id,
        "page.updated",
        "page",
        page_id,
    )
    .await?;
    transaction.commit().await?;
    get_page(&state.pool, page_id, principal.actor_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("page"))
}

async fn get_revision_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((page_id, version)): ApiPath<(Uuid, i64)>,
) -> Result<Json<RevisionView>, AppError> {
    let project_id = page_project(&state.pool, page_id).await?;
    project_access(
        &state.pool,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        project_id,
        Action::ProjectView,
    )
    .await
    .map_err(|_| AppError::NotFound("page"))?;
    get_revision(&state.pool, page_id, version, principal.actor_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("page version"))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecisionListQuery {
    include_superseded: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DecisionsResponse {
    decisions: Vec<DecisionView>,
    can_record: bool,
}

async fn list_decisions_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<DecisionListQuery>,
) -> Result<Json<DecisionsResponse>, AppError> {
    let kind = parse_actor_kind(&principal.actor_kind)?;
    project_access(
        &state.pool,
        principal.actor_id,
        kind,
        project_id,
        Action::ProjectView,
    )
    .await?;
    let can_record = project_access(
        &state.pool,
        principal.actor_id,
        kind,
        project_id,
        Action::PageEdit,
    )
    .await
    .is_ok();
    Ok(Json(DecisionsResponse {
        decisions: list_decisions(
            &state.pool,
            project_id,
            principal.actor_id,
            query.include_superseded.unwrap_or(false),
            200,
        )
        .await?,
        can_record,
    }))
}

async fn record_decision_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(input): ApiJson<DecisionInput>,
) -> Result<Json<DecisionView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let writer = browser_writer(&principal)?;
    let mut transaction = state.pool.begin().await?;
    project_access(
        &mut *transaction,
        writer.actor_id,
        writer.actor_kind,
        project_id,
        Action::PageEdit,
    )
    .await?;
    let decision_id = record_decision(&mut transaction, writer, project_id, &input).await?;
    audit(
        &mut transaction,
        &principal,
        project_id,
        "decision.recorded",
        "decision",
        decision_id,
    )
    .await?;
    transaction.commit().await?;
    get_decision(&state.pool, decision_id, principal.actor_id)
        .await?
        .map(Json)
        .ok_or(AppError::NotFound("decision"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PinsResponse {
    pins: Vec<PinView>,
}

async fn list_pins_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
) -> Result<Json<PinsResponse>, AppError> {
    // Visibility and membership go through the shared conversation lookup.
    let row = fetch_conversation_access(&state.pool, principal.actor_id, conversation_id)
        .await?
        .ok_or(AppError::NotFound("conversation"))?;
    authorize_conversation_row_for(
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        &row,
        Action::ProjectView,
        false,
    )?;
    Ok(Json(PinsResponse {
        pins: list_pins(&state.pool, conversation_id, principal.actor_id).await?,
    }))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PinRequest {
    kind: String,
    target_id: Uuid,
}

async fn pin_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(conversation_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<PinRequest>,
) -> Result<Json<PinsResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let writer = browser_writer(&principal)?;
    let mut transaction = state.pool.begin().await?;
    let project_id = set_pin(
        &mut transaction,
        writer,
        conversation_id,
        &request.kind,
        request.target_id,
        true,
    )
    .await?;
    audit(
        &mut transaction,
        &principal,
        project_id,
        "conversation.pinned",
        "conversation",
        conversation_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(PinsResponse {
        pins: list_pins(&state.pool, conversation_id, principal.actor_id).await?,
    }))
}

async fn unpin_http(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((conversation_id, kind, target_id)): ApiPath<(Uuid, String, Uuid)>,
) -> Result<Json<PinsResponse>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let writer = browser_writer(&principal)?;
    let mut transaction = state.pool.begin().await?;
    let project_id = set_pin(
        &mut transaction,
        writer,
        conversation_id,
        &kind,
        target_id,
        false,
    )
    .await?;
    audit(
        &mut transaction,
        &principal,
        project_id,
        "conversation.unpinned",
        "conversation",
        conversation_id,
    )
    .await?;
    transaction.commit().await?;
    Ok(Json(PinsResponse {
        pins: list_pins(&state.pool, conversation_id, principal.actor_id).await?,
    }))
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimView {
    claimed_by: String,
    claimed_by_kind: String,
    note: Option<String>,
    claimed_at: String,
    expires_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ClaimResponse {
    claim: Option<ClaimView>,
}

/// Who is working on a task right now, if anyone has claimed it.
async fn get_claim_http(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<ClaimResponse>, AppError> {
    project_access(
        &state.pool,
        principal.actor_id,
        parse_actor_kind(&principal.actor_kind)?,
        project_id,
        Action::WorkItemView,
    )
    .await?;
    let claim = sqlx::query_as::<_, ClaimView>(
        r#"
        SELECT COALESCE(member.display_name, 'Someone') AS claimed_by,
               actor.kind AS claimed_by_kind, claim.note,
               claim.claimed_at::text AS claimed_at, claim.expires_at::text AS expires_at
        FROM work_item_claims claim
        JOIN actors actor ON actor.id = claim.actor_id
        LEFT JOIN project_memberships member
          ON member.project_id = claim.project_id AND member.actor_id = claim.actor_id
        WHERE claim.project_id = $1 AND claim.work_item_id = $2 AND claim.expires_at > now()
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(&state.pool)
    .await?;
    Ok(Json(ClaimResponse { claim }))
}

#[cfg(test)]
mod tests {
    use super::skill_name;

    #[test]
    fn skill_names_are_never_empty() {
        assert_eq!(skill_name("Deploy the web app!"), "deploy-the-web-app");
        assert_eq!(skill_name("🚀"), "skill");
        assert_eq!(skill_name("--"), "skill");
    }
}
