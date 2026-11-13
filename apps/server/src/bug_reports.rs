//! Bug report links: other apps send bug reports straight into a project.
//!
//! A link is a secret URL for one project, made in Project settings › Bug
//! reports. Anything that can POST to it creates a task there: a curl line,
//! a `fetch` in a crash handler, `navigator.sendBeacon`, an HTML form. The
//! body can be JSON, a form, or plain text, and only a title or a
//! description is needed; every other field is listed under "Details" in
//! the task. CORS is open, so a web page can call it from any origin.
//!
//! The task is created as the person who made the link, through the same
//! `task_create` tool the app and agents use, so it gets the same checks.
//! Every report first checks that person's current access (principle 9); a
//! link whose maker can no longer add tasks turns itself off with the
//! reason. Identical reports, or ones carrying the same `id`, within a day
//! make one task, not one each, so a crash loop can't flood the board.
//!
//! Only the token's SHA-256 digest is stored. The full link is shown once;
//! replacing it makes a new one and the old one stops working.

use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgExecutor, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    events::{self, Caller},
    mcp::{self, McpPrincipal},
    policy::{Action as PolicyAction, ActorKind},
    work_item_commands::{MAX_DESCRIPTION_CHARACTERS, MAX_TITLE_CHARACTERS},
};

const TOKEN_PREFIX: &str = "awbr_";
/// Links per project, at most.
const MAX_PER_PROJECT: i64 = 25;
/// The largest report body; a stack trace and some details fit easily.
const MAX_REPORT_BYTES: usize = 64 * 1024;
/// Extra fields listed under "Details", at most.
const MAX_DETAILS: usize = 40;
const MAX_DETAIL_CHARACTERS: usize = 8_000;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/bug-report-links",
            get(list_route).post(create_route),
        )
        .route(
            "/api/v1/bug-report-links/{link_id}",
            patch(update_route).delete(delete_route),
        )
        .route(
            "/api/v1/bug-report-links/{link_id}/replace",
            post(replace_route),
        )
        .route(
            "/api/v1/bug-reports/{token}",
            post(report_route).options(preflight_route),
        )
}

// ---------------------------------------------------------------------------
// Links

#[derive(Debug, FromRow)]
struct LinkRow {
    id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    owner_actor_id: Uuid,
    owner_name: Option<String>,
    name: String,
    token_hint: String,
    state: String,
    disabled_reason: Option<String>,
    report_count: i64,
    last_report_at: Option<String>,
    version: i64,
    created_at: String,
}

const LINK_SELECT: &str = r#"
    SELECT link.id, link.organization_id, link.project_id, link.owner_actor_id,
           owner.display_name AS owner_name, link.name, link.token_hint, link.state,
           link.disabled_reason, link.report_count,
           to_char(link.last_report_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS last_report_at,
           link.version,
           to_char(link.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD"T"HH24:MI:SS.MS"Z"') AS created_at
    FROM bug_report_links link
    LEFT JOIN users owner ON owner.actor_id = link.owner_actor_id
"#;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkView {
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub owner_actor_id: Uuid,
    pub owner_name: Option<String>,
    /// The token's last four characters, so people can tell links apart.
    pub token_hint: String,
    pub state: String,
    pub disabled_reason: Option<String>,
    pub report_count: i64,
    pub last_report_at: Option<String>,
    /// Whether the caller can pause, replace or delete it.
    pub can_manage: bool,
    pub version: i64,
    pub created_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkList {
    pub links: Vec<LinkView>,
    /// Whether the caller can make a link here.
    pub can_create: bool,
}

/// A new or replaced link, with its URL, shown once.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkWithUrl {
    pub link: LinkView,
    pub url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CreateLink {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateLink {
    pub name: Option<String>,
    /// active or paused.
    pub state: Option<String>,
}

fn view(row: LinkRow, can_manage: bool) -> LinkView {
    LinkView {
        id: row.id,
        project_id: row.project_id,
        name: row.name,
        owner_actor_id: row.owner_actor_id,
        owner_name: row.owner_name,
        token_hint: row.token_hint,
        state: row.state,
        disabled_reason: row.disabled_reason,
        report_count: row.report_count,
        last_report_at: row.last_report_at,
        can_manage,
        version: row.version,
        created_at: row.created_at,
    }
}

fn normalize_name(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err(AppError::Validation("name"));
    }
    Ok(value.to_owned())
}

fn new_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    format!("{TOKEN_PREFIX}{}", URL_SAFE_NO_PAD.encode(bytes))
}

fn digest(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

fn hint(token: &str) -> String {
    token[token.len() - 4..].to_owned()
}

fn link_url(state: &AppState, token: &str) -> String {
    format!(
        "{}/api/v1/bug-reports/{token}",
        events::public_origin(state).trim_end_matches('/')
    )
}

/// What the caller may do with links in a project: (add tasks, manage the
/// project). Not found unless they can see the project.
async fn project_rights(
    pool: &PgPool,
    caller: &Caller,
    project_id: Uuid,
) -> Result<(bool, bool), AppError> {
    let direct = crate::memory::project_access(
        pool,
        caller.actor_id,
        caller.actor_kind,
        project_id,
        PolicyAction::WorkItemView,
    )
    .await?;
    if direct {
        return Err(AppError::NotFound("project"));
    }
    let human = caller.actor_kind == ActorKind::Human;
    let can_create = human
        && crate::memory::project_access(
            pool,
            caller.actor_id,
            caller.actor_kind,
            project_id,
            PolicyAction::WorkItemCreate,
        )
        .await
        .is_ok();
    let admin = human
        && crate::memory::project_access(
            pool,
            caller.actor_id,
            caller.actor_kind,
            project_id,
            PolicyAction::ProjectManage,
        )
        .await
        .is_ok();
    Ok((can_create, admin))
}

async fn fetch(executor: impl PgExecutor<'_>, link_id: Uuid) -> Result<LinkRow, AppError> {
    sqlx::query_as::<_, LinkRow>(&format!("{LINK_SELECT} WHERE link.id = $1"))
        .bind(link_id)
        .fetch_optional(executor)
        .await?
        .ok_or(AppError::NotFound("bug_report_link"))
}

/// The project a link belongs to, if it exists.
pub(crate) async fn link_project(pool: &PgPool, link_id: Uuid) -> Result<Option<Uuid>, AppError> {
    Ok(
        sqlx::query_scalar::<_, Uuid>("SELECT project_id FROM bug_report_links WHERE id = $1")
            .bind(link_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// A link the caller may change: their own, or any in a project they
/// manage. Not found otherwise.
async fn manageable(pool: &PgPool, caller: &Caller, link_id: Uuid) -> Result<LinkRow, AppError> {
    let row = fetch(pool, link_id).await?;
    let (_, admin) = project_rights(pool, caller, row.project_id)
        .await
        .map_err(|_| AppError::NotFound("bug_report_link"))?;
    if caller.actor_kind != ActorKind::Human || !(admin || row.owner_actor_id == caller.actor_id) {
        return Err(AppError::NotFound("bug_report_link"));
    }
    Ok(row)
}

pub(crate) async fn list(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
) -> Result<LinkList, AppError> {
    let (can_create, admin) = project_rights(&state.pool, caller, project_id).await?;
    let rows = sqlx::query_as::<_, LinkRow>(&format!(
        "{LINK_SELECT} WHERE link.project_id = $1 ORDER BY link.created_at DESC, link.id DESC LIMIT 100"
    ))
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?;
    let human = caller.actor_kind == ActorKind::Human;
    Ok(LinkList {
        links: rows
            .into_iter()
            .map(|row| {
                let can_manage = human && (admin || row.owner_actor_id == caller.actor_id);
                view(row, can_manage)
            })
            .collect(),
        can_create,
    })
}

pub(crate) async fn create(
    state: &AppState,
    caller: &Caller,
    project_id: Uuid,
    request: CreateLink,
) -> Result<LinkWithUrl, AppError> {
    let name = normalize_name(&request.name)?;
    let (can_create, _) = project_rights(&state.pool, caller, project_id).await?;
    if !can_create {
        return Err(AppError::Forbidden);
    }
    let mut transaction = state.pool.begin().await?;
    let organization_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT organization_id FROM projects WHERE id = $1 FOR UPDATE",
    )
    .bind(project_id)
    .fetch_one(&mut *transaction)
    .await?;
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM bug_report_links WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
    if count >= MAX_PER_PROJECT {
        return Err(AppError::Validation("bug_report_links.limit"));
    }
    let link_id = Uuid::now_v7();
    let token = new_token();
    sqlx::query(
        r#"
        INSERT INTO bug_report_links (
            id, organization_id, project_id, owner_actor_id, name, token_digest, token_hint
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(link_id)
    .bind(organization_id)
    .bind(project_id)
    .bind(caller.actor_id)
    .bind(&name)
    .bind(digest(&token))
    .bind(hint(&token))
    .execute(&mut *transaction)
    .await?;
    events::audit(
        &mut *transaction,
        caller,
        organization_id,
        Some(project_id),
        "bug_report_link.create",
        "bug_report_link",
        link_id,
        json!({ "name": name }),
    )
    .await?;
    transaction.commit().await?;
    let row = fetch(&state.pool, link_id).await?;
    Ok(LinkWithUrl {
        link: view(row, true),
        url: link_url(state, &token),
    })
}

pub(crate) async fn update(
    state: &AppState,
    caller: &Caller,
    link_id: Uuid,
    request: UpdateLink,
) -> Result<LinkView, AppError> {
    let row = manageable(&state.pool, caller, link_id).await?;
    let name = request.name.as_deref().map(normalize_name).transpose()?;
    let new_state = match request.state.as_deref() {
        None => None,
        Some(value @ ("active" | "paused")) => Some(value),
        Some(_) => return Err(AppError::Validation("state")),
    };
    // Turning a link back on makes the person doing it its maker, so it
    // acts as someone who can add tasks here now.
    let owner = if new_state == Some("active") && row.state != "active" {
        let (can_create, _) = project_rights(&state.pool, caller, row.project_id).await?;
        if !can_create {
            return Err(AppError::Forbidden);
        }
        caller.actor_id
    } else {
        row.owner_actor_id
    };
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE bug_report_links
        SET name = coalesce($2, name),
            state = coalesce($3, state),
            disabled_reason = CASE WHEN $3::text IS NULL THEN disabled_reason ELSE NULL END,
            owner_actor_id = $4,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(link_id)
    .bind(&name)
    .bind(new_state)
    .bind(owner)
    .execute(&mut *transaction)
    .await?;
    events::audit(
        &mut *transaction,
        caller,
        row.organization_id,
        Some(row.project_id),
        "bug_report_link.update",
        "bug_report_link",
        link_id,
        json!({ "name": name, "state": new_state }),
    )
    .await?;
    transaction.commit().await?;
    Ok(view(fetch(&state.pool, link_id).await?, true))
}

pub(crate) async fn replace(
    state: &AppState,
    caller: &Caller,
    link_id: Uuid,
) -> Result<LinkWithUrl, AppError> {
    let row = manageable(&state.pool, caller, link_id).await?;
    let token = new_token();
    let mut transaction = state.pool.begin().await?;
    sqlx::query(
        r#"
        UPDATE bug_report_links
        SET token_digest = $2, token_hint = $3, version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(link_id)
    .bind(digest(&token))
    .bind(hint(&token))
    .execute(&mut *transaction)
    .await?;
    events::audit(
        &mut *transaction,
        caller,
        row.organization_id,
        Some(row.project_id),
        "bug_report_link.replace",
        "bug_report_link",
        link_id,
        json!({}),
    )
    .await?;
    transaction.commit().await?;
    Ok(LinkWithUrl {
        link: view(fetch(&state.pool, link_id).await?, true),
        url: link_url(state, &token),
    })
}

pub(crate) async fn delete(
    state: &AppState,
    caller: &Caller,
    link_id: Uuid,
) -> Result<(), AppError> {
    let row = manageable(&state.pool, caller, link_id).await?;
    let mut transaction = state.pool.begin().await?;
    sqlx::query("DELETE FROM bug_report_links WHERE id = $1")
        .bind(link_id)
        .execute(&mut *transaction)
        .await?;
    events::audit(
        &mut *transaction,
        caller,
        row.organization_id,
        Some(row.project_id),
        "bug_report_link.delete",
        "bug_report_link",
        link_id,
        json!({ "name": row.name }),
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Reports

/// A report, read from whatever the sender posted.
#[derive(Debug, Default, PartialEq)]
struct Report {
    title: Option<String>,
    description: Option<String>,
    /// The sender's own id for this report, so a resend makes no new task.
    report_id: Option<String>,
    priority: Option<String>,
    details: Vec<(String, String)>,
}

/// Reads a JSON object, a form, or plain text. Plain text that is a JSON
/// object is read as JSON, since `sendBeacon` and `no-cors` fetches send
/// JSON as text.
fn parse_report(content_type: Option<&str>, body: &[u8]) -> Result<Report, AppError> {
    let content_type = content_type
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let text = std::str::from_utf8(body).map_err(|_| AppError::Invalid("report body"))?;
    let fields: Map<String, Value> = if content_type == "application/x-www-form-urlencoded" {
        serde_urlencoded::from_str::<Vec<(String, String)>>(text)
            .map_err(|_| AppError::Invalid("report form"))?
            .into_iter()
            .map(|(key, value)| (key, Value::String(value)))
            .collect()
    } else {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(object)) => object,
            Ok(_) | Err(_) if content_type == "application/json" => {
                return Err(AppError::Invalid("report JSON"));
            }
            _ => {
                let mut object = Map::new();
                object.insert("description".to_owned(), Value::String(text.to_owned()));
                object
            }
        }
    };
    let mut report = Report::default();
    for (key, value) in fields {
        let text = match &value {
            Value::Null => continue,
            Value::String(text) => text.clone(),
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => serde_json::to_string_pretty(&value).unwrap_or_default(),
        };
        if text.trim().is_empty() {
            continue;
        }
        let slot = match key.to_ascii_lowercase().as_str() {
            "title" | "summary" | "subject" => &mut report.title,
            "description" | "body" | "message" | "text" | "details" => &mut report.description,
            "id" | "reportid" | "report_id" => &mut report.report_id,
            "priority" | "severity" => &mut report.priority,
            _ => {
                if report.details.len() < MAX_DETAILS {
                    report.details.push((key, text));
                }
                continue;
            }
        };
        if slot.is_none() {
            *slot = Some(text);
        } else if report.details.len() < MAX_DETAILS {
            report.details.push((key, text));
        }
    }
    Ok(report)
}

fn clip(value: &str, characters: usize) -> String {
    if value.chars().count() <= characters {
        return value.to_owned();
    }
    let mut clipped = value.chars().take(characters - 1).collect::<String>();
    clipped.push('…');
    clipped
}

/// Text with control characters other than line breaks and tabs removed.
fn printable(value: &str) -> String {
    value
        .replace("\r\n", "\n")
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect()
}

/// One line, for a title or a detail's name.
fn one_line(value: &str) -> String {
    value
        .split(|character: char| character.is_whitespace() || character.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// The task's title and description for a report.
fn task_text(report: &Report, link_name: &str) -> Result<(String, String), AppError> {
    let description = report.description.as_deref().map(printable);
    let title = report
        .title
        .as_deref()
        .map(one_line)
        .filter(|title| !title.is_empty())
        .or_else(|| {
            description
                .as_deref()
                .and_then(|text| text.lines().map(one_line).find(|line| !line.is_empty()))
        });
    let Some(title) = title else {
        if report.details.is_empty() {
            return Err(AppError::Validation("title"));
        }
        return Ok((
            format!("Bug report from {link_name}"),
            describe(None, report, link_name),
        ));
    };
    Ok((
        clip(&title, MAX_TITLE_CHARACTERS),
        describe(description.as_deref(), report, link_name),
    ))
}

fn describe(description: Option<&str>, report: &Report, link_name: &str) -> String {
    let mut out = String::new();
    if let Some(description) = description.map(str::trim).filter(|text| !text.is_empty()) {
        out.push_str(&clip(description, MAX_DESCRIPTION_CHARACTERS / 2));
        out.push_str("\n\n");
    }
    if !report.details.is_empty() {
        out.push_str("**Details**\n\n");
        for (key, value) in &report.details {
            let key = clip(&one_line(key), 80);
            let value = clip(&printable(value), MAX_DETAIL_CHARACTERS);
            if value.contains('\n') || value.chars().count() > 200 {
                let fence = if value.contains("```") { "~~~~" } else { "```" };
                out.push_str(&format!("- **{key}**:\n\n{fence}\n{value}\n{fence}\n\n"));
            } else {
                out.push_str(&format!("- **{key}**: {}\n", one_line(&value)));
            }
        }
        out.push('\n');
    }
    out.push_str(&format!(
        "_Sent through the bug report link \"{}\"._",
        one_line(link_name)
    ));
    clip(&out, MAX_DESCRIPTION_CHARACTERS)
}

/// A stable request id: the sender's id, or the report's text, so a resend
/// within a day gives back the same task.
fn request_id(link_id: Uuid, report: &Report, title: &str, description: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(link_id.as_bytes());
    match &report.report_id {
        Some(id) => {
            hasher.update(b"id:");
            hasher.update(id.trim().as_bytes());
        }
        None => {
            hasher.update(b"text:");
            hasher.update(title.as_bytes());
            hasher.update([0]);
            hasher.update(description.as_bytes());
        }
    }
    format!("bug-report.{}", &hex::encode(hasher.finalize())[..40])
}

fn priority(value: Option<&str>) -> Option<String> {
    let value = value?.trim().to_ascii_lowercase();
    let mapped = match value.as_str() {
        "low" | "minor" | "trivial" => "low",
        "medium" | "normal" | "moderate" => "medium",
        "high" | "major" | "error" => "high",
        "urgent" | "critical" | "blocker" | "fatal" | "crash" => "urgent",
        _ => return None,
    };
    Some(mapped.to_owned())
}

#[derive(FromRow)]
struct ReportLink {
    id: Uuid,
    project_id: Uuid,
    owner_actor_id: Uuid,
    name: String,
    state: String,
    bug_type: Option<String>,
}

/// Why the maker can't add tasks through it any more, if they can't.
async fn maker_refusal(pool: &PgPool, link: &ReportLink) -> Result<Option<String>, AppError> {
    let reachable = sqlx::query_scalar::<_, bool>(
        "SELECT coalesce(actor_can_access_project($1, NULL, $2), false)",
    )
    .bind(link.owner_actor_id)
    .bind(link.project_id)
    .fetch_one(pool)
    .await?;
    let allowed = reachable
        && matches!(
            crate::memory::project_access(
                pool,
                link.owner_actor_id,
                ActorKind::Human,
                link.project_id,
                PolicyAction::WorkItemCreate,
            )
            .await,
            Ok(false)
        );
    Ok((!allowed).then(|| {
        "Turned off because the person who made it can no longer add tasks in this project. \
         Turn it on again to send reports as you."
            .to_owned()
    }))
}

async fn receive(
    state: &AppState,
    token: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, AppError> {
    let token_digest = digest(token);
    state.rate_limits.check_bug_report(&token_digest)?;
    if body.len() > MAX_REPORT_BYTES {
        return Err(AppError::PayloadTooLarge);
    }
    let link = sqlx::query_as::<_, ReportLink>(
        r#"
        SELECT link.id, link.project_id, link.owner_actor_id, link.name, link.state,
               (SELECT item_type.name FROM work_item_types item_type
                WHERE item_type.project_id = link.project_id AND item_type.enabled
                  AND lower(item_type.name) = 'bug'
                LIMIT 1) AS bug_type
        FROM bug_report_links link
        WHERE link.token_digest = $1
        "#,
    )
    .bind(&token_digest)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("bug_report_link"))?;
    if link.state != "active" {
        return Ok(refused(
            StatusCode::FORBIDDEN,
            "This bug report link is turned off.",
        ));
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    let report = parse_report(content_type, body)?;
    let (title, description) = task_text(&report, &link.name)?;
    if let Some(reason) = maker_refusal(&state.pool, &link).await? {
        sqlx::query(
            r#"
            UPDATE bug_report_links
            SET state = 'disabled', disabled_reason = $2, version = version + 1, updated_at = now()
            WHERE id = $1 AND state = 'active'
            "#,
        )
        .bind(link.id)
        .bind(&reason)
        .execute(&state.pool)
        .await?;
        return Ok(refused(
            StatusCode::FORBIDDEN,
            "This bug report link is turned off.",
        ));
    }
    let principal = McpPrincipal {
        credential_id: link.id,
        mcp_credential_id: None,
        oauth_grant_id: None,
        actor_id: link.owner_actor_id,
        actor_kind: ActorKind::Human,
        project_id: link.project_id,
        external_agent_connection_id: None,
        client_name: format!("Bug report link: {}", link.name)
            .chars()
            .take(80)
            .collect(),
        scopes: vec!["read".to_owned(), "write".to_owned()],
        authentication_class: "bug_report_link",
        account_wide: false,
        project_explicit: true,
    };
    let mut arguments = json!({
        "projectId": link.project_id,
        "title": title,
        "description": description,
        "clientRequestId": request_id(link.id, &report, &title, &description),
    });
    if let Some(bug_type) = &link.bug_type {
        arguments["type"] = json!(bug_type);
    }
    if let Some(priority) = priority(report.priority.as_deref()) {
        arguments["priority"] = json!(priority);
    }
    let result = mcp::call_tool_direct(state, principal, "task_create", arguments)
        .await
        .map_err(|error| match error {
            mcp::DirectCallError::Refused(error) => error,
            mcp::DirectCallError::UnknownTool | mcp::DirectCallError::InvalidArguments(_) => {
                AppError::Internal("bug report task_create call")
            }
        })?;
    if result.get("outcome").and_then(Value::as_str) != Some("applied") {
        let outcome = result
            .get("outcome")
            .and_then(Value::as_str)
            .unwrap_or("none");
        tracing::warn!(outcome, "bug report refused by task_create");
        let message = result
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("The task couldn't be created.");
        return Ok(refused(crate::public_api::status_for(&result), message));
    }
    let duplicate = result
        .get("idempotentReplay")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !duplicate {
        sqlx::query(
            r#"
            UPDATE bug_report_links
            SET report_count = report_count + 1, last_report_at = now()
            WHERE id = $1
            "#,
        )
        .bind(link.id)
        .execute(&state.pool)
        .await?;
    }
    let status = if duplicate {
        StatusCode::OK
    } else {
        StatusCode::CREATED
    };
    Ok((
        status,
        Json(json!({
            "ok": true,
            "task": result.get("key"),
            "duplicate": duplicate,
        })),
    )
        .into_response())
}

fn refused(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": message }))).into_response()
}

/// Lets a page on any origin send reports and read the answer.
fn open_cors(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("content-type"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("86400"),
    );
    response
}

// ---------------------------------------------------------------------------
// HTTP

async fn report_route(
    State(state): State<AppState>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let response = if !token.starts_with(TOKEN_PREFIX) || token.len() > 128 {
        refused(StatusCode::NOT_FOUND, "There's no bug report link here.")
    } else {
        match receive(&state, &token, &headers, &body).await {
            Ok(response) => response,
            Err(AppError::NotFound(_)) => {
                refused(StatusCode::NOT_FOUND, "There's no bug report link here.")
            }
            Err(AppError::Validation("title")) => refused(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Send a title or a description.",
            ),
            Err(error @ (AppError::Invalid(_) | AppError::PayloadTooLarge)) => {
                let message = match error {
                    AppError::PayloadTooLarge => "The report is larger than 64 KB.",
                    _ => "Send JSON, a form, or plain text.",
                };
                let status = error.into_response().status();
                refused(status, message)
            }
            Err(error) => error.into_response(),
        }
    };
    open_cors(response)
}

async fn preflight_route() -> Response {
    open_cors(StatusCode::NO_CONTENT.into_response())
}

async fn list_route(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<LinkList>, AppError> {
    let caller = Caller::browser(&principal)?;
    Ok(Json(list(&state, &caller, project_id).await?))
}

async fn create_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateLink>,
) -> Result<(StatusCode, Json<LinkWithUrl>), AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok((
        StatusCode::CREATED,
        Json(create(&state, &caller, project_id, request).await?),
    ))
}

async fn update_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(link_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<UpdateLink>,
) -> Result<Json<LinkView>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok(Json(update(&state, &caller, link_id, request).await?))
}

async fn replace_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(link_id): ApiPath<Uuid>,
) -> Result<Json<LinkWithUrl>, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    Ok(Json(replace(&state, &caller, link_id).await?))
}

async fn delete_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(link_id): ApiPath<Uuid>,
) -> Result<StatusCode, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let caller = Caller::browser(&principal)?;
    delete(&state, &caller, link_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_json_forms_and_text() {
        let json = parse_report(
            Some("application/json"),
            br#"{"title":"Crash on level 3","description":"It froze.","version":"1.2.0","stack":"line one\nline two","id":"abc"}"#,
        )
        .expect("json");
        assert_eq!(json.title.as_deref(), Some("Crash on level 3"));
        assert_eq!(json.description.as_deref(), Some("It froze."));
        assert_eq!(json.report_id.as_deref(), Some("abc"));
        assert_eq!(json.details.len(), 2);

        // sendBeacon and no-cors fetches send JSON as text.
        let beacon = parse_report(Some("text/plain;charset=UTF-8"), br#"{"message":"Boom"}"#)
            .expect("beacon");
        assert_eq!(beacon.description.as_deref(), Some("Boom"));

        let form = parse_report(
            Some("application/x-www-form-urlencoded"),
            b"title=Button+broken&page=%2Fsettings",
        )
        .expect("form");
        assert_eq!(form.title.as_deref(), Some("Button broken"));
        assert_eq!(
            form.details,
            vec![("page".to_owned(), "/settings".to_owned())]
        );

        let text = parse_report(None, b"The save button does nothing\nOn Firefox").expect("text");
        let (title, description) = task_text(&text, "Game").expect("task text");
        assert_eq!(title, "The save button does nothing");
        assert!(description.starts_with("The save button does nothing\nOn Firefox"));

        assert!(parse_report(Some("application/json"), b"[1, 2]").is_err());
    }

    #[test]
    fn task_text_needs_something_and_stays_in_bounds() {
        assert!(task_text(&Report::default(), "Game").is_err());
        let only_details = Report {
            details: vec![("level".to_owned(), "3".to_owned())],
            ..Report::default()
        };
        let (title, description) = task_text(&only_details, "Game").expect("details only");
        assert_eq!(title, "Bug report from Game");
        assert!(description.contains("- **level**: 3"));

        let long = Report {
            title: Some(format!("{}\u{0007}", "x".repeat(400))),
            details: vec![("stack".to_owned(), "a\n```\nb".to_owned())],
            ..Report::default()
        };
        let (title, description) = task_text(&long, "Game").expect("long");
        assert_eq!(title.chars().count(), MAX_TITLE_CHARACTERS);
        assert!(!title.chars().any(char::is_control));
        assert!(description.contains("~~~~\na\n```\nb\n~~~~"));
    }

    #[test]
    fn same_report_same_request_id() {
        let link = Uuid::now_v7();
        let report = Report::default();
        let first = request_id(link, &report, "t", "d");
        assert_eq!(first, request_id(link, &report, "t", "d"));
        assert_ne!(first, request_id(link, &report, "t", "e"));
        assert_ne!(first, request_id(Uuid::now_v7(), &report, "t", "d"));
        assert!(first.len() <= 128);
        let with_id = Report {
            report_id: Some("r1".to_owned()),
            ..Report::default()
        };
        assert_eq!(
            request_id(link, &with_id, "t", "d"),
            request_id(link, &with_id, "other", "text")
        );
        assert_eq!(priority(Some("Critical")).as_deref(), Some("urgent"));
        assert_eq!(priority(Some("whatever")), None);
    }
}
