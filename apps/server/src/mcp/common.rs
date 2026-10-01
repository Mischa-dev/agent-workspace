//! Shared pieces for the MCP tools that work the same way for a person acting
//! through a connected app and for an agent acting as itself.

use std::collections::HashMap;

use rmcp::schemars;
use serde::Serialize;
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use super::McpPrincipal;
use crate::{AppState, error::AppError, policy::ActorKind};

/// Longest message or document excerpt returned in lists.
pub(super) const SNIPPET_CHARACTERS: usize = 280;
/// Longest message text returned when reading a conversation.
pub(super) const MESSAGE_CHARACTERS: usize = 8_000;

/// Shared wording, so every tool says the same thing about the same problem.
pub(super) const READ_ONLY_MESSAGE: &str =
    "This connection is read-only. Reconnect it with full access to make changes.";
pub(super) const ARCHIVED_PROJECT_MESSAGE: &str =
    "This project is archived. Restore it with project_archive (archived: false) first.";

/// Someone else changed a task between the caller's read and write.
pub(super) fn version_conflict_message(task: &str, version: i64) -> String {
    format!(
        "Someone changed {task} first (now version {version}). Open it again with task_open, then retry with expectedVersion {version}."
    )
}

/// An archived task refuses every change but a restore.
pub(super) fn archived_task_message(task: &str) -> String {
    format!("{task} is archived. Restore it before changing it.")
}

/// The result shape every newer tool uses. `message` is written for the agent
/// reading it: it says what happened and what to try next.
#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(super) enum ToolResult<T> {
    Ok { value: T },
    NotFound { message: String },
    InvalidRequest { field: String, message: String },
    PermissionDenied { message: String },
    Conflict { message: String },
    TemporarilyUnavailable { message: String },
}

impl<T> ToolResult<T> {
    pub(super) fn ok(value: T) -> Self {
        Self::Ok { value }
    }

    pub(super) fn not_found(what: &str) -> Self {
        Self::NotFound {
            message: format!("{what} wasn't found, or you can't see it."),
        }
    }

    pub(super) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::InvalidRequest {
            field: field.to_owned(),
            message: message.into(),
        }
    }

    pub(super) fn denied(message: impl Into<String>) -> Self {
        Self::PermissionDenied {
            message: message.into(),
        }
    }

    pub(super) fn read_only() -> Self {
        Self::denied(READ_ONLY_MESSAGE)
    }

    pub(super) fn conflict(message: impl Into<String>) -> Self {
        Self::Conflict {
            message: message.into(),
        }
    }

    pub(super) fn unavailable() -> Self {
        Self::TemporarilyUnavailable {
            message: "Agent Workspace couldn't finish that. Try again shortly.".to_owned(),
        }
    }

    pub(super) fn from_error(error: AppError, operation: &'static str) -> Self {
        match error {
            AppError::NotFound(what) => Self::not_found(what),
            AppError::Validation(field) | AppError::Invalid(field) => {
                Self::invalid(field, format!("{field} isn't valid."))
            }
            AppError::Conflict(_, message) => Self::conflict(message),
            AppError::VersionConflict {
                current_version, ..
            } => Self::conflict(format!(
                "Someone changed this first (now version {current_version}). Read it again, then retry."
            )),
            AppError::ProjectArchived => Self::conflict("That project is archived."),
            AppError::Forbidden
            | AppError::AuthenticationRequired
            | AppError::InvalidCredentials => {
                Self::denied("You don't have permission to do that here.")
            }
            AppError::PayloadTooLarge => Self::invalid("text", "That's too long."),
            error => {
                tracing::error!(
                    operation,
                    category = error.log_category(),
                    "MCP tool failed"
                );
                Self::unavailable()
            }
        }
    }
}

/// Projects this caller can act in right now.
///
/// People connected through an app reach every Project they belong to. Agents
/// reach the Projects their connection allows. Older Project-scoped
/// credentials reach only their own Project.
pub(super) async fn accessible_project_ids(
    state: &AppState,
    principal: &McpPrincipal,
) -> Result<Vec<Uuid>, AppError> {
    if principal.actor_kind == ActorKind::Human && !principal.account_wide {
        return Ok(vec![principal.project_id]);
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = $1
         AND membership.enabled
        WHERE project.archived_at IS NULL
          AND actor_can_access_project($1, $2, project.id)
        ORDER BY project.id
        LIMIT 500
        "#,
    )
    .bind(principal.actor_id)
    .bind(principal.external_agent_connection_id)
    .fetch_all(&state.pool)
    .await?)
}

/// The Projects a tool call should look at: the one it named, or every
/// Project the caller can reach.
pub(super) async fn scope_projects(
    state: &AppState,
    principal: &McpPrincipal,
) -> Result<Vec<Uuid>, AppError> {
    if principal.project_explicit {
        Ok(vec![principal.project_id])
    } else {
        accessible_project_ids(state, principal).await
    }
}

/// Whether a call has a project to act in. Account-wide connections reach
/// every project, so they must name one; agents and older project-scoped
/// credentials act in their own project by default.
pub(super) fn project_is_chosen(principal: &McpPrincipal) -> bool {
    principal.project_explicit || !principal.account_wide
}

/// The caller's membership role in a Project, if they can act there.
pub(super) async fn project_role(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_scalar::<_, String>(
        r#"
        SELECT membership.role
        FROM project_memberships membership
        WHERE membership.project_id = $2 AND membership.actor_id = $1 AND membership.enabled
          AND actor_can_access_project($1, $3, $2)
        "#,
    )
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.external_agent_connection_id)
    .fetch_optional(&state.pool)
    .await?)
}

/// Whether the caller's role in a Project allows an action.
pub(super) fn role_allows(
    principal: &McpPrincipal,
    project_id: Uuid,
    role: &str,
    action: crate::policy::Action,
) -> bool {
    super::mcp_policy_allows(
        principal.actor_id,
        principal.actor_kind,
        project_id,
        role,
        action,
    )
}

/// Record in this transaction which connected app a person is acting
/// through, so rows written by shared command code carry it (migration 0042).
pub(super) async fn mark_via(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
) -> Result<(), AppError> {
    if let Some(grant_id) = principal.via_grant_id() {
        sqlx::query("SELECT set_config('aw.via_grant', $1, true)")
            .bind(grant_id.to_string())
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

pub(super) async fn insert_audit(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    project_id: Uuid,
    action: &str,
    target_kind: &str,
    target_id: Uuid,
    metadata: serde_json::Value,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class, action,
            target_kind, target_id, outcome, metadata, channel, client_name,
            mcp_credential_id, mcp_oauth_grant_id, external_agent_connection_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, 'succeeded', $8, 'mcp', $9, $10, $11, $12)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(project_id)
    .bind(principal.authentication_class)
    .bind(action)
    .bind(target_kind)
    .bind(target_id)
    .bind(metadata)
    .bind(&principal.client_name)
    .bind(principal.mcp_credential_id)
    .bind(principal.oauth_grant_id)
    .bind(principal.external_agent_connection_id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Accept a UUID, or an Agent Workspace URL that ends in one.
pub(super) fn parse_id(value: &str) -> Option<Uuid> {
    let value = value.trim();
    if value.len() > 2048 {
        return None;
    }
    value.parse::<Uuid>().ok().or_else(|| {
        value
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|segment| segment.split(['?', '#']).next())
            .and_then(|segment| segment.parse::<Uuid>().ok())
    })
}

pub(super) fn snippet(value: &str, max_characters: usize) -> String {
    let collapsed = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_characters {
        return collapsed;
    }
    let mut cut = collapsed
        .chars()
        .take(max_characters.saturating_sub(1))
        .collect::<String>();
    cut.push('…');
    cut
}

pub(super) fn truncate_text(value: &str, max_characters: usize) -> (String, bool) {
    if value.chars().count() <= max_characters {
        (value.to_owned(), false)
    } else {
        (value.chars().take(max_characters).collect(), true)
    }
}

pub(super) fn public_url(state: &AppState, path: &str) -> String {
    format!("{}{path}", super::oauth::public_origin(state))
}

pub(super) fn conversation_path(project_id: Uuid, kind: &str, id: Uuid, direct: bool) -> String {
    if direct {
        format!("/messages/{id}")
    } else if kind == "thread" {
        format!("/projects/{project_id}/threads/{id}")
    } else {
        format!("/projects/{project_id}/channels/{id}")
    }
}

#[derive(Debug, FromRow)]
struct HandleRow {
    actor_id: Uuid,
    handle: String,
}

/// Turn plain text into message content, making each `@handle` of someone in
/// the Project a real mention so they're notified.
pub(super) async fn content_from_text(
    state: &AppState,
    project_id: Uuid,
    text: &str,
) -> Result<serde_json::Value, AppError> {
    let handles = sqlx::query_as::<_, HandleRow>(
        r#"
        SELECT membership.actor_id, lower(membership.handle) AS handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1 AND membership.enabled AND actor.status = 'active'
          AND actor.kind IN ('human', 'external_agent')
        "#,
    )
    .bind(project_id)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .map(|row| (row.handle, row.actor_id))
    .collect::<HashMap<_, _>>();
    Ok(serde_json::json!({ "version": 1, "nodes": text_nodes(text, &handles) }))
}

fn text_nodes(text: &str, handles: &HashMap<String, Uuid>) -> Vec<serde_json::Value> {
    let mut nodes = Vec::new();
    let mut pending = String::new();
    let mut mentions = 0;
    let characters = text.char_indices().collect::<Vec<_>>();
    let mut index = 0;
    while index < characters.len() {
        let (offset, character) = characters[index];
        let starts_word = index == 0 || !is_handle_character(characters[index - 1].1);
        if character == '@' && starts_word && mentions < 32 {
            let rest = &text[offset + 1..];
            let handle = rest
                .chars()
                .take_while(|value| is_handle_character(*value))
                .collect::<String>();
            if let Some(actor_id) = handles.get(&handle.to_lowercase()) {
                if !pending.is_empty() {
                    nodes.push(serde_json::json!({ "type": "text", "text": pending }));
                    pending = String::new();
                }
                nodes.push(serde_json::json!({ "type": "mention", "actorId": actor_id }));
                mentions += 1;
                index += 1 + handle.chars().count();
                continue;
            }
        }
        pending.push(character);
        index += 1;
    }
    if !pending.is_empty() || nodes.is_empty() {
        nodes.push(serde_json::json!({ "type": "text", "text": pending }));
    }
    nodes
}

fn is_handle_character(value: char) -> bool {
    value.is_ascii_alphanumeric() || value == '_' || value == '-'
}

/// Resolve people given as actor IDs or `@handles` among a Project's members.
pub(super) async fn resolve_people(
    state: &AppState,
    project_id: Uuid,
    people: &[String],
) -> Result<Result<Vec<Uuid>, String>, AppError> {
    let mut resolved = Vec::new();
    for person in people {
        let person = person.trim();
        let found = if let Ok(actor_id) = person.parse::<Uuid>() {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT actor_id FROM project_memberships WHERE project_id = $1 AND actor_id = $2 AND enabled",
            )
            .bind(project_id)
            .bind(actor_id)
            .fetch_optional(&state.pool)
            .await?
        } else {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT actor_id FROM project_memberships WHERE project_id = $1 AND lower(handle) = lower($2) AND enabled",
            )
            .bind(project_id)
            .bind(person.trim_start_matches('@'))
            .fetch_optional(&state.pool)
            .await?
        };
        match found {
            Some(actor_id) => resolved.push(actor_id),
            None => return Ok(Err(person.to_owned())),
        }
    }
    Ok(Ok(resolved))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use uuid::Uuid;

    use super::{parse_id, snippet, text_nodes};

    #[test]
    fn handles_become_mentions_only_for_known_people() {
        let mara = Uuid::now_v7();
        let handles = HashMap::from([("mara".to_owned(), mara)]);
        let nodes = text_nodes("hey @Mara, ask @nobody or email a@b.c", &handles);
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0]["text"], "hey ");
        assert_eq!(nodes[1]["actorId"], mara.to_string());
        assert_eq!(nodes[2]["text"], ", ask @nobody or email a@b.c");
    }

    #[test]
    fn ids_come_from_urls_too() {
        let id = Uuid::now_v7();
        assert_eq!(parse_id(&id.to_string()), Some(id));
        assert_eq!(
            parse_id(&format!(
                "https://example.test/projects/x/threads/{id}?tab=1"
            )),
            Some(id)
        );
        assert_eq!(parse_id("AW-12"), None);
    }

    #[test]
    fn snippets_collapse_whitespace_and_stay_short() {
        assert_eq!(snippet("a\n\n b   c", 10), "a b c");
        assert_eq!(snippet("abcdefghij", 5).chars().count(), 5);
    }
}
