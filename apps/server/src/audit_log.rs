//! S8: the workspace's audit log, read.
//!
//! `audit_events` is written all over the server; this reads it for one
//! workspace, newest first, with filters for who, which app, what kind and
//! when. Owners and admins read it. An event about a conversation or a
//! message shows only when the reader can see that conversation
//! (`conversation_visible_to`), so the log never reveals a private
//! conversation to someone outside it. Metadata holds ids and counts, never
//! message text or secret values, and is returned as written.

use axum::{Json, Router, extract::State, routing::get};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::FromRow;
use uuid::Uuid;

use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiPath, ApiQuery, AppError},
    workspace::caller_role,
};

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 100;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/organizations/{organization_id}/audit-events",
            get(list_events),
        )
        .route(
            "/api/v1/organizations/{organization_id}/audit-events/facets",
            get(facets),
        )
}

/// Which app an event came through.
#[derive(Clone, Debug, Eq, PartialEq)]
enum AppFilter {
    Channel(&'static str),
    Client(String),
}

fn parse_app(value: &str) -> Result<AppFilter, AppError> {
    match value {
        "browser" => Ok(AppFilter::Channel("browser")),
        "mcp" => Ok(AppFilter::Channel("mcp")),
        "api" => Ok(AppFilter::Channel("api")),
        "automation" => Ok(AppFilter::Channel("automation")),
        "host_cli" => Ok(AppFilter::Channel("host_cli")),
        "system" => Ok(AppFilter::Channel("system")),
        _ => {
            let client = value
                .strip_prefix("client:")
                .filter(|client| (1..=80).contains(&client.chars().count()))
                .filter(|client| !client.chars().any(char::is_control))
                .ok_or(AppError::Validation("app"))?;
            Ok(AppFilter::Client(client.to_owned()))
        }
    }
}

/// An action prefix such as `setting` or `project_membership.updated`.
fn parse_kind(value: &str) -> Result<String, AppError> {
    let valid = (1..=120).contains(&value.len())
        && value.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        })
        && value.as_bytes()[0].is_ascii_lowercase();
    if valid {
        Ok(value.to_owned())
    } else {
        Err(AppError::Validation("kind"))
    }
}

/// The cursor is the last event's time and id: `<rfc3339>~<uuid>`.
fn parse_cursor(value: &str) -> Result<(DateTime<Utc>, Uuid), AppError> {
    let (time, id) = value
        .split_once('~')
        .ok_or(AppError::Validation("before"))?;
    Ok((
        DateTime::parse_from_rfc3339(time)
            .map_err(|_| AppError::Validation("before"))?
            .with_timezone(&Utc),
        Uuid::parse_str(id).map_err(|_| AppError::Validation("before"))?,
    ))
}

fn cursor(occurred_at: DateTime<Utc>, id: Uuid) -> String {
    format!(
        "{}~{id}",
        occurred_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true)
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventQuery {
    #[serde(default)]
    actor_id: Option<Uuid>,
    #[serde(default)]
    app: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    project_id: Option<Uuid>,
    #[serde(default)]
    since: Option<DateTime<Utc>>,
    #[serde(default)]
    until: Option<DateTime<Utc>>,
    #[serde(default)]
    before: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
}

#[derive(Debug, FromRow)]
struct EventRow {
    id: Uuid,
    occurred_at: DateTime<Utc>,
    action: String,
    outcome: String,
    actor_id: Option<Uuid>,
    actor_name: Option<String>,
    actor_kind: Option<String>,
    project_id: Option<Uuid>,
    project_name: Option<String>,
    project_key: Option<String>,
    channel: Option<String>,
    client_name: Option<String>,
    target_kind: String,
    target_id: Option<Uuid>,
    metadata: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventActor {
    actor_id: Uuid,
    display_name: String,
    kind: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventProject {
    id: Uuid,
    name: String,
    key: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventApp {
    channel: Option<String>,
    client_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventTarget {
    kind: String,
    id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventView {
    id: Uuid,
    occurred_at: DateTime<Utc>,
    action: String,
    outcome: String,
    actor: Option<EventActor>,
    project: Option<EventProject>,
    app: EventApp,
    target: EventTarget,
    metadata: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EventsResponse {
    events: Vec<EventView>,
    next_cursor: Option<String>,
}

/// Owners and admins of the workspace read its log.
async fn require_reader(
    state: &AppState,
    principal: &AuthenticatedActor,
    organization_id: Uuid,
) -> Result<(), AppError> {
    let mut connection = state.pool.acquire().await?;
    if caller_role(&mut connection, principal, organization_id, false)
        .await?
        .manages()
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Events in the workspace the reader may see: everything but events about
/// conversations or messages they can't see. `$1` is the workspace, `$2`
/// the reader.
const VISIBLE_EVENTS: &str = r#"
    FROM audit_events event
    LEFT JOIN projects project ON project.id = event.project_id
    LEFT JOIN actors actor ON actor.id = event.actor_id
    LEFT JOIN organization_actor_profiles profile
      ON profile.organization_id = event.organization_id AND profile.actor_id = event.actor_id
    LEFT JOIN users user_account ON user_account.actor_id = event.actor_id
    WHERE event.organization_id = $1
      AND (event.target_kind <> 'conversation' OR event.target_id IS NULL
           OR conversation_visible_to(event.target_id, $2))
      AND (event.target_kind <> 'message' OR event.target_id IS NULL
           OR EXISTS (SELECT 1 FROM messages message
                      WHERE message.id = event.target_id
                        AND conversation_visible_to(message.conversation_id, $2)))
      AND NOT (event.metadata ? 'conversation_id'
               AND (event.metadata->>'conversation_id') ~ '^[0-9a-f-]{36}$'
               AND NOT conversation_visible_to((event.metadata->>'conversation_id')::uuid, $2))
      AND NOT (event.metadata ? 'conversationId'
               AND (event.metadata->>'conversationId') ~ '^[0-9a-f-]{36}$'
               AND NOT conversation_visible_to((event.metadata->>'conversationId')::uuid, $2))
"#;

async fn list_events(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<EventQuery>,
) -> Result<Json<EventsResponse>, AppError> {
    require_reader(&state, &principal, organization_id).await?;
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(AppError::Validation("limit"));
    }
    let app = query.app.as_deref().map(parse_app).transpose()?;
    let kind = query.kind.as_deref().map(parse_kind).transpose()?;
    let before = query.before.as_deref().map(parse_cursor).transpose()?;
    let (channel, client) = match app {
        Some(AppFilter::Channel(channel)) => (Some(channel), None),
        Some(AppFilter::Client(client)) => (None, Some(client)),
        None => (None, None),
    };
    let rows = sqlx::query_as::<_, EventRow>(&format!(
        r#"
        SELECT event.id, event.occurred_at, event.action, event.outcome,
               event.actor_id,
               COALESCE(profile.display_name, user_account.display_name,
                        CASE actor.kind WHEN 'integration' THEN 'Integration'
                                        WHEN 'system' THEN 'System' ELSE NULL END) AS actor_name,
               actor.kind AS actor_kind,
               project.id AS project_id, project.name AS project_name,
               project.project_key, event.channel, event.client_name,
               event.target_kind, event.target_id, event.metadata
        {VISIBLE_EVENTS}
          AND ($3::uuid IS NULL OR event.actor_id = $3)
          AND ($4::text IS NULL OR event.channel = $4)
          AND ($5::text IS NULL OR event.client_name = $5)
          AND ($6::text IS NULL OR event.action = $6 OR left(event.action, length($6) + 1) = $6 || '.')
          AND ($7::uuid IS NULL OR event.project_id = $7)
          AND ($8::timestamptz IS NULL OR event.occurred_at >= $8)
          AND ($9::timestamptz IS NULL OR event.occurred_at < $9)
          AND ($10::timestamptz IS NULL OR (event.occurred_at, event.id) < ($10, $11))
        ORDER BY event.occurred_at DESC, event.id DESC
        LIMIT $12
        "#
    ))
    .bind(organization_id)
    .bind(principal.actor_id)
    .bind(query.actor_id)
    .bind(channel)
    .bind(client)
    .bind(kind)
    .bind(query.project_id)
    .bind(query.since)
    .bind(query.until)
    .bind(before.map(|(time, _)| time))
    .bind(before.map(|(_, id)| id))
    .bind(limit + 1)
    .fetch_all(&state.pool)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let events = rows
        .into_iter()
        .take(limit as usize)
        .map(|row| EventView {
            id: row.id,
            occurred_at: row.occurred_at,
            action: row.action,
            outcome: row.outcome,
            actor: row.actor_id.map(|actor_id| EventActor {
                actor_id,
                display_name: row.actor_name.unwrap_or_else(|| "Unknown".to_owned()),
                kind: row.actor_kind.unwrap_or_else(|| "unknown".to_owned()),
            }),
            project: row
                .project_id
                .zip(row.project_name)
                .zip(row.project_key)
                .map(|((id, name), key)| EventProject { id, name, key }),
            app: EventApp {
                channel: row.channel,
                client_name: row.client_name,
            },
            target: EventTarget {
                kind: row.target_kind,
                id: row.target_id,
            },
            metadata: row.metadata,
        })
        .collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| {
            events
                .last()
                .map(|event| cursor(event.occurred_at, event.id))
        })
        .flatten();
    Ok(Json(EventsResponse {
        events,
        next_cursor,
    }))
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct FacetActor {
    actor_id: Uuid,
    display_name: String,
    kind: String,
}

#[derive(Debug, Serialize)]
struct FacetOption {
    value: String,
    label: String,
}

#[derive(Debug, FromRow, Serialize)]
struct FacetProject {
    id: Uuid,
    name: String,
    key: String,
}

#[derive(Debug, Serialize)]
struct FacetsResponse {
    actors: Vec<FacetActor>,
    apps: Vec<FacetOption>,
    kinds: Vec<FacetOption>,
    projects: Vec<FacetProject>,
}

/// A readable name for an action's first part.
fn kind_label(kind: &str) -> String {
    match kind {
        "authentication" => "Sign-in".to_owned(),
        "account" => "Account".to_owned(),
        "setting" => "Settings".to_owned(),
        "secret" => "Keys".to_owned(),
        "workspace" => "Workspace".to_owned(),
        "workspace_membership" => "Workspace members".to_owned(),
        "project" => "Projects".to_owned(),
        "project_membership" => "Project members".to_owned(),
        "conversation" => "Conversations".to_owned(),
        "mcp" => "Apps and tokens".to_owned(),
        "github" => "GitHub".to_owned(),
        other => {
            let spaced = other.replace('_', " ");
            let mut characters = spaced.chars();
            characters
                .next()
                .map(|first| first.to_uppercase().chain(characters).collect())
                .unwrap_or_default()
        }
    }
}

fn channel_label(channel: &str) -> &'static str {
    match channel {
        "browser" => "Browser",
        "mcp" => "Connected apps",
        "api" => "Public API",
        "automation" => "Automations",
        "host_cli" => "Server command line",
        "system" => "System",
        _ => "Other",
    }
}

async fn facets(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
) -> Result<Json<FacetsResponse>, AppError> {
    require_reader(&state, &principal, organization_id).await?;
    let actors = sqlx::query_as::<_, FacetActor>(
        r#"
        SELECT DISTINCT ON (membership.actor_id) membership.actor_id,
               COALESCE(profile.display_name, user_account.display_name, 'Unknown') AS display_name,
               actor.kind
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
         AND actor.kind IN ('human', 'external_agent')
        LEFT JOIN organization_actor_profiles profile
          ON profile.organization_id = membership.organization_id
         AND profile.actor_id = membership.actor_id
        LEFT JOIN users user_account ON user_account.actor_id = membership.actor_id
        WHERE membership.organization_id = $1
        ORDER BY membership.actor_id
        LIMIT 500
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?;
    let mut actors = actors;
    actors.sort_by(|left, right| {
        left.display_name
            .to_lowercase()
            .cmp(&right.display_name.to_lowercase())
    });
    let channels = sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT channel FROM audit_events
        WHERE organization_id = $1 AND channel IS NOT NULL
        ORDER BY 1
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?;
    let clients = sqlx::query_scalar::<_, String>(
        r#"
        SELECT client_name FROM audit_events
        WHERE organization_id = $1 AND client_name IS NOT NULL
        GROUP BY client_name ORDER BY max(occurred_at) DESC LIMIT 50
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?;
    let kinds = sqlx::query_scalar::<_, String>(
        r#"
        SELECT DISTINCT split_part(action, '.', 1) FROM audit_events
        WHERE organization_id = $1
        ORDER BY 1
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?;
    let projects = sqlx::query_as::<_, FacetProject>(
        r#"
        SELECT id, name, project_key AS key FROM projects
        WHERE organization_id = $1 AND kind NOT IN ('direct', 'workspace')
        ORDER BY archived_at NULLS FIRST, lower(name), id
        LIMIT 500
        "#,
    )
    .bind(organization_id)
    .fetch_all(&state.pool)
    .await?;
    let mut apps = channels
        .iter()
        .map(|channel| FacetOption {
            value: channel.clone(),
            label: channel_label(channel).to_owned(),
        })
        .collect::<Vec<_>>();
    apps.extend(clients.into_iter().map(|client| FacetOption {
        value: format!("client:{client}"),
        label: client,
    }));
    Ok(Json(FacetsResponse {
        actors,
        apps,
        kinds: kinds
            .into_iter()
            .map(|kind| FacetOption {
                label: kind_label(&kind),
                value: kind,
            })
            .collect(),
        projects,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_filters_are_channels_or_named_clients() {
        assert_eq!(
            parse_app("mcp").expect("test value"),
            AppFilter::Channel("mcp")
        );
        assert_eq!(
            parse_app("client:Claude Code").expect("test value"),
            AppFilter::Client("Claude Code".to_owned())
        );
        assert!(parse_app("client:").is_err());
        assert!(parse_app("phone").is_err());
    }

    #[test]
    fn kinds_are_action_prefixes() {
        assert!(parse_kind("setting").is_ok());
        assert!(parse_kind("project_membership.updated").is_ok());
        assert!(parse_kind("Setting").is_err());
        assert!(parse_kind("setting.").is_err());
        assert!(parse_kind("a%").is_err());
    }

    #[test]
    fn cursors_round_trip() {
        let now = Utc::now();
        let id = Uuid::now_v7();
        let (time, parsed) = parse_cursor(&cursor(now, id)).expect("test value");
        assert_eq!(parsed, id);
        assert_eq!(time.timestamp_micros(), now.timestamp_micros());
        assert!(parse_cursor("nonsense").is_err());
    }
}
