//! Shared pieces for the MCP tools that work the same way for a person acting
//! through a connected app and for an agent acting as itself.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
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
    Ok {
        value: T,
    },
    NotFound {
        message: String,
    },
    InvalidRequest {
        field: String,
        message: String,
    },
    PermissionDenied {
        message: String,
        /// "module" when a module that's off here refused it (AWD-75).
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_category: Option<String>,
    },
    Conflict {
        message: String,
    },
    TemporarilyUnavailable {
        message: String,
    },
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
            retry_category: None,
        }
    }

    /// A call refused because a module it needs is off where it acts
    /// (AWD-75), as `run_tool` refuses a tool of a module that's off.
    pub(super) fn module_off(tool: &str, module: &str) -> Self {
        Self::PermissionDenied {
            message: format!(
                "{tool} needs {module} here, which is turned off (for this workspace, or for your role). module_list says which modules are on and why."
            ),
            retry_category: Some("module".to_owned()),
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
            AppError::Refused { field, message } => Self::invalid(field, message),
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

/// The one Project an older Project-scoped credential stays in, whatever
/// else the person belongs to. None when the caller's reach is every Project
/// `actor_can_access_project` allows.
pub(super) fn confined_project(principal: &McpPrincipal) -> Option<Uuid> {
    (principal.actor_kind == ActorKind::Human && !principal.account_wide)
        .then_some(principal.project_id)
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
    accessible_project_ids_in(&state.pool, principal).await
}

/// `accessible_project_ids` on a connection the caller already has. A write
/// transaction that holds locks reads it here rather than waiting for a
/// second pool connection.
pub(super) async fn accessible_project_ids_in(
    executor: impl sqlx::PgExecutor<'_>,
    principal: &McpPrincipal,
) -> Result<Vec<Uuid>, AppError> {
    if let Some(project_id) = confined_project(principal) {
        return Ok(vec![project_id]);
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
    .fetch_all(executor)
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

/// A stored membership role: the plain role, and the custom role's
/// permissions when it holds one.
#[derive(Clone, Debug, sqlx::FromRow)]
pub(super) struct StoredRole {
    pub role: String,
    pub role_permissions: Option<Vec<String>>,
}

impl StoredRole {
    pub fn of(role: &str, role_permissions: Option<&[String]>) -> Self {
        Self {
            role: role.to_owned(),
            role_permissions: role_permissions.map(<[String]>::to_vec),
        }
    }
}

/// The caller's membership role in a Project, if they can act there.
pub(super) async fn project_role(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: Uuid,
) -> Result<Option<StoredRole>, AppError> {
    Ok(sqlx::query_as::<_, StoredRole>(
        r#"
        SELECT membership.role,
               workspace_role_permissions(membership.role_id) AS role_permissions
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
    role: &StoredRole,
    action: crate::policy::Action,
) -> bool {
    super::mcp_policy_allows(
        principal.actor_id,
        principal.actor_kind,
        project_id,
        &role.role,
        role.role_permissions.as_deref(),
        action,
    )
}

/// Record in this transaction which connected app a person is acting
/// through, so rows written by shared command code carry it (migration 0042).
pub(super) async fn mark_via(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
) -> Result<(), AppError> {
    // K3.2: the events this write emits name who acted and through which app.
    crate::events::set_context(
        transaction,
        &crate::events::EventContext::actor(principal.actor_id, principal.via_grant_id()),
    )
    .await
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

/// Reads an argument agents send either as a string or as a number (such as
/// a board position, `3` or `"3"`) as the string, so the tool can answer a
/// bad value in plain words instead of failing to parse the call. Pair it
/// with `#[schemars(with = "Option<String>")]`: the input schema stays a
/// plain string, without an anyOf.
pub(super) fn string_or_number<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Text(String),
        Number(serde_json::Number),
    }
    Ok(
        Option::<Raw>::deserialize(deserializer)?.map(|raw| match raw {
            Raw::Text(text) => text,
            Raw::Number(number) => number.to_string(),
        }),
    )
}

/// Reads a yes/no argument agents send either as a boolean or as a word (such
/// as `claimed: true` or `claimed: "me"`), with true as "yes" and false as
/// "no". Pair it with `#[schemars(with = "Option<String>")]`, so the input
/// schema stays a plain string.
pub(super) fn flag_or_text<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Flag(bool),
        Text(String),
    }
    Ok(
        Option::<Raw>::deserialize(deserializer)?.map(|raw| match raw {
            Raw::Flag(true) => "yes".to_owned(),
            Raw::Flag(false) => "no".to_owned(),
            Raw::Text(text) => text,
        }),
    )
}

/// Reads a list argument agents may also send as one value (`status: "Done"`
/// as well as `status: ["Done", "In Review"]`). Pair it with
/// `#[schemars(with = "Option<Vec<String>>")]`, so the input schema stays a
/// plain list.
pub(super) fn one_or_many<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        One(String),
        Many(Vec<String>),
    }
    Ok(
        Option::<Raw>::deserialize(deserializer)?.map(|raw| match raw {
            Raw::One(value) => vec![value],
            Raw::Many(values) => values,
        }),
    )
}

/// A date (YYYY-MM-DD, as the start or the end of that day in UTC) or an
/// RFC 3339 timestamp.
pub(super) fn parse_time(value: &str, end_of_day: bool) -> Option<DateTime<Utc>> {
    let value = value.trim();
    if let Ok(time) = DateTime::parse_from_rfc3339(value) {
        return Some(time.with_timezone(&Utc));
    }
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").ok()?;
    let time = if end_of_day {
        date.and_hms_opt(23, 59, 59)?
    } else {
        date.and_hms_opt(0, 0, 0)?
    };
    Some(time.and_utc())
}

/// A moment to look back to: a span such as `24h` or `7d` (at most ten
/// years) before now, or anything `parse_time` reads.
pub(super) fn parse_since(value: &str) -> Option<DateTime<Utc>> {
    let value = value.trim();
    let span = |digits: &str, unit: char| {
        if digits.is_empty() || digits.len() > 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let amount = digits.parse::<i64>().ok()?;
        let span = match unit {
            'h' => chrono::Duration::try_hours(amount)?,
            _ => chrono::Duration::try_days(amount)?,
        };
        (span <= chrono::Duration::try_days(3650)?).then_some(span)
    };
    let lower = value.to_ascii_lowercase();
    for unit in ['h', 'd'] {
        if let Some(digits) = lower.strip_suffix(unit) {
            return span(digits, unit).map(|span| Utc::now() - span);
        }
    }
    parse_time(value, false)
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

/// Accept a message's UUID, or a link to it. The web app links a message as
/// its conversation's address plus `#message-<id>` (such as
/// `/projects/…/channels/<conversation>#message-<id>` or
/// `/messages/<conversation>#message-<id>`), which `parse_id` alone reads as
/// the conversation. Anything without that anchor is read by `parse_id`.
pub(super) fn parse_message_id(value: &str) -> Option<Uuid> {
    let value = value.trim();
    if value.len() > 2048 {
        return None;
    }
    match value.rsplit_once("#message-") {
        Some((_, anchor)) => anchor
            .split(|character: char| !(character.is_ascii_hexdigit() || character == '-'))
            .next()
            .and_then(|id| id.parse::<Uuid>().ok()),
        None => parse_id(value),
    }
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

/// Like [`content_from_text`], for a task comment: `@handle` becomes a
/// mention and the project's own task keys (`KEY-12`) become task
/// references, the nodes the web composer writes, so an agent's comment
/// notifies the people it names and shows up in "Referenced in" (P0.4).
pub(super) async fn comment_content_from_text(
    state: &AppState,
    project_id: Uuid,
    text: &str,
) -> Result<serde_json::Value, AppError> {
    let content = content_from_text(state, project_id, text).await?;
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_optional(&state.pool)
            .await?;
    let Some(project_key) = project_key else {
        return Ok(content);
    };
    let numbers = task_key_numbers(text, &project_key);
    if numbers.is_empty() {
        return Ok(content);
    }
    let tasks = sqlx::query_as::<_, (i64, Uuid)>(
        "SELECT number, id FROM work_items WHERE project_id = $1 AND number = ANY($2)",
    )
    .bind(project_id)
    .bind(&numbers)
    .fetch_all(&state.pool)
    .await?
    .into_iter()
    .collect::<HashMap<_, _>>();
    let nodes = content["nodes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|node| match node["text"].as_str() {
            Some(text) if node["type"] == "text" => reference_nodes(text, &project_key, &tasks),
            _ => vec![node],
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({ "version": 1, "nodes": nodes }))
}

/// At most this many task references are made from one comment's text.
const MAX_TEXT_REFERENCES: usize = 32;

/// The numbers of `KEY-<n>` tokens in `text` that stand alone as words.
fn task_key_numbers(text: &str, project_key: &str) -> Vec<i64> {
    task_key_tokens(text, project_key)
        .into_iter()
        .map(|(_, _, number)| number)
        .take(MAX_TEXT_REFERENCES)
        .collect()
}

/// (start, end, number) of each standalone `KEY-<n>` in `text`.
fn task_key_tokens(text: &str, project_key: &str) -> Vec<(usize, usize, i64)> {
    let prefix = format!("{project_key}-");
    let mut tokens = Vec::new();
    for (start, _) in text.match_indices(&prefix) {
        let before = text[..start].chars().next_back();
        if before.is_some_and(|character| character.is_alphanumeric() || character == '_') {
            continue;
        }
        let digits_start = start + prefix.len();
        let digits = text[digits_start..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        let end = digits_start + digits.len();
        let after = text[end..].chars().next();
        if digits.is_empty()
            || after.is_some_and(|character| character.is_alphanumeric() || character == '_')
        {
            continue;
        }
        if let Ok(number) = digits.parse::<i64>() {
            tokens.push((start, end, number));
        }
    }
    tokens
}

/// One text node split around the task keys it names.
fn reference_nodes(
    text: &str,
    project_key: &str,
    tasks: &HashMap<i64, Uuid>,
) -> Vec<serde_json::Value> {
    let mut nodes = Vec::new();
    let mut cursor = 0;
    for (start, end, number) in task_key_tokens(text, project_key)
        .into_iter()
        .take(MAX_TEXT_REFERENCES)
    {
        let Some(task_id) = tasks.get(&number) else {
            continue;
        };
        if start > cursor {
            nodes.push(serde_json::json!({ "type": "text", "text": &text[cursor..start] }));
        }
        nodes.push(serde_json::json!({ "type": "work_item_ref", "workItemId": task_id }));
        cursor = end;
    }
    if cursor < text.len() || nodes.is_empty() {
        nodes.push(serde_json::json!({ "type": "text", "text": &text[cursor..] }));
    }
    nodes
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

    use super::{
        flag_or_text, parse_id, parse_message_id, parse_since, parse_time, snippet,
        string_or_number, text_nodes,
    };

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
    fn message_links_name_the_message_not_its_conversation() {
        let conversation = Uuid::now_v7();
        let message = Uuid::now_v7();
        assert_eq!(parse_message_id(&message.to_string()), Some(message));
        assert_eq!(
            parse_message_id(&format!(" {} ", message.simple())),
            Some(message)
        );
        for link in [
            format!("https://example.test/projects/x/channels/{conversation}#message-{message}"),
            format!("/messages/{conversation}#message-{message}"),
            format!(
                "https://example.test/projects/x/threads/{conversation}?tab=1#message-{message}"
            ),
            format!("/messages/{conversation}#message-{message}/"),
        ] {
            assert_eq!(parse_message_id(&link), Some(message), "{link}");
        }
        // Without the anchor a link is read as any other: here, the channel,
        // which then isn't found as a message.
        assert_eq!(
            parse_message_id(&format!("/projects/x/channels/{conversation}")),
            Some(conversation)
        );
        // A broken anchor never falls back to the conversation's ID.
        assert_eq!(
            parse_message_id(&format!("/messages/{conversation}#message-nope")),
            None
        );
        assert_eq!(parse_message_id("#message-"), None);
        assert_eq!(parse_message_id(&"x".repeat(3000)), None);
    }

    #[test]
    fn numbers_and_strings_both_read_as_strings() {
        #[derive(serde::Deserialize)]
        struct Arguments {
            #[serde(default, deserialize_with = "string_or_number")]
            position: Option<String>,
        }
        let read = |value: serde_json::Value| {
            serde_json::from_value::<Arguments>(value).map(|arguments| arguments.position)
        };
        assert_eq!(
            read(serde_json::json!({"position": 3})).expect("number"),
            Some("3".to_owned())
        );
        assert_eq!(
            read(serde_json::json!({"position": "top"})).expect("string"),
            Some("top".to_owned())
        );
        assert_eq!(read(serde_json::json!({})).expect("absent"), None);
        assert_eq!(
            read(serde_json::json!({"position": null})).expect("null"),
            None
        );
        assert!(read(serde_json::json!({"position": true})).is_err());
    }

    #[test]
    fn flags_and_words_both_read_as_words() {
        #[derive(serde::Deserialize)]
        struct Arguments {
            #[serde(default, deserialize_with = "flag_or_text")]
            claimed: Option<String>,
        }
        let read = |value: serde_json::Value| {
            serde_json::from_value::<Arguments>(value).map(|arguments| arguments.claimed)
        };
        assert_eq!(
            read(serde_json::json!({"claimed": true})).expect("true"),
            Some("yes".to_owned())
        );
        assert_eq!(
            read(serde_json::json!({"claimed": false})).expect("false"),
            Some("no".to_owned())
        );
        assert_eq!(
            read(serde_json::json!({"claimed": "@mara"})).expect("handle"),
            Some("@mara".to_owned())
        );
        assert_eq!(read(serde_json::json!({})).expect("absent"), None);
        assert!(read(serde_json::json!({"claimed": 3})).is_err());
    }

    #[test]
    fn since_takes_dates_timestamps_and_spans() {
        let now = chrono::Utc::now();
        let day = parse_since("24h").expect("hours");
        let hours = (now - day).num_hours();
        assert!((23..=24).contains(&hours), "{hours}");
        let week = parse_since(" 7D ").expect("days");
        assert!((now - week).num_days() >= 6, "{week}");
        assert_eq!(
            parse_since("2026-09-30").expect("date").to_rfc3339(),
            "2026-09-30T00:00:00+00:00"
        );
        assert_eq!(
            parse_since("2026-09-30T14:00:00Z")
                .expect("timestamp")
                .to_rfc3339(),
            "2026-09-30T14:00:00+00:00"
        );
        for refused in [
            "soon",
            "",
            "h",
            "12345h",
            "3651d",
            "-2d",
            "7w",
            "2026-13-01",
        ] {
            assert_eq!(parse_since(refused), None, "{refused}");
        }
        assert_eq!(
            parse_time("2026-09-30", true).expect("end").to_rfc3339(),
            "2026-09-30T23:59:59+00:00"
        );
    }

    #[test]
    fn snippets_collapse_whitespace_and_stay_short() {
        assert_eq!(snippet("a\n\n b   c", 10), "a b c");
        assert_eq!(snippet("abcdefghij", 5).chars().count(), 5);
    }
}
