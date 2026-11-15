//! A task's history for task_open with include: ["history"]: what happened
//! to it, who did it, when, and through which app (docs/plans/mcp-tools.md,
//! Phase 2). It reads the same timeline the web app shows
//! (`work_items::fetch_timeline_rows`) and turns each entry into a line an
//! agent can read without decoding activity payloads.
//!
//! Activity payloads hold IDs only. Names are looked up per page, in one
//! query per kind of thing, and a name is shown only when the reader may
//! see it: conversations, messages and decisions through
//! `conversation_visible_to`, other tasks through the reader's reachable
//! projects. Archived labels are left out, as they are everywhere else.
//! A change's before and after (`changes`, roadmap T8) reads in plain words,
//! such as "Priority: High → Urgent", with names from
//! `work_item_history::label_changes`, as the web timeline shows them.

use std::collections::{BTreeSet, HashMap};

use rmcp::schemars;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{accessible_project_ids, truncate_text},
};
use crate::{
    AppState,
    error::AppError,
    work_item_history::{CUSTOM_FIELD_PREFIX, label_changes},
    work_items::{WorkItemTimelineRow, encode_work_item_timeline_cursor, fetch_timeline_rows},
};

pub(super) const DEFAULT_HISTORY_LIMIT: u16 = 30;
pub(super) const MAX_HISTORY_LIMIT: u16 = 100;
/// The most comment text an entry carries.
const COMMENT_CHARACTERS: usize = 1_000;
/// The most criterion text an entry quotes.
const CRITERION_CHARACTERS: i32 = 160;
/// The most description diff an entry shows.
const DIFF_CHARACTERS: usize = 1_500;

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskHistory {
    /// Oldest first: the latest `historyLimit` entries, or those before
    /// `historyBefore`.
    pub entries: Vec<HistoryEntry>,
    /// How many entries the task's history has in all.
    pub total: i64,
    /// Pass as historyBefore for earlier entries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub older_cursor: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct HistoryEntry {
    pub sequence: i64,
    pub at: String,
    /// "comment", or what happened: work_item_created, work_item_updated
    /// (fields, status, labels, parent, board place, claims and links to
    /// pages, messages, decisions and pull requests), work_item_archived,
    /// work_item_restored, criterion_created, criterion_updated,
    /// criterion_completed, criterion_reopened, criterion_removed,
    /// thread_linked, thread_unlinked, repository_linked,
    /// repository_unlinked, repository_primary_changed, worker_invoked,
    /// github_automation_transition, dependency_added, dependency_removed,
    /// relation_added, or relation_removed.
    pub kind: String,
    /// Who, by their name in the project.
    pub by: String,
    pub by_handle: String,
    /// person, agent, integration, system, or worker.
    pub by_kind: String,
    /// The connected app they acted through, when they show it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// What happened, or the comment's text.
    pub text: String,
    /// True when a comment was cut at 1,000 characters.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub text_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to_status: Option<String>,
    /// For an edit: the fields it changed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub criterion_id: Option<String>,
    /// For a task moved here: the archived original, when you can reach its
    /// project. Open it with history for what happened before the move.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_task_id: Option<String>,
}

/// Names for the IDs a page of history mentions, limited to what the reader
/// may see. Anything missing is described without its name.
#[derive(Debug, Default)]
struct Lookups {
    statuses: HashMap<Uuid, String>,
    /// Current text (cut to 160 characters) and position.
    criteria: HashMap<Uuid, (String, i32)>,
    /// "#name" for a channel or thread, None for a DM.
    conversations: HashMap<Uuid, Option<String>>,
    /// The conversation each message is in, as `conversations` names it.
    messages: HashMap<Uuid, Option<String>>,
    repositories: HashMap<Uuid, String>,
    /// Number and title.
    pull_requests: HashMap<Uuid, (i64, String)>,
    pages: HashMap<Uuid, String>,
    decisions: HashMap<Uuid, String>,
    /// Active labels only.
    labels: HashMap<Uuid, String>,
    /// Keys of other tasks in the project, such as a parent.
    tasks: HashMap<Uuid, String>,
    /// The archived original of a task moved here, when it's reachable.
    moved_from: Option<Uuid>,
    /// Where this task went, when it was moved away and that's reachable.
    moved_to: Option<String>,
    /// The task whose history this is.
    this_task: Option<Uuid>,
    /// Keys of tasks linked by a dependency or relation, in this project or
    /// one the reader can reach.
    linked_tasks: HashMap<Uuid, String>,
}

/// The IDs a page of history mentions, gathered before the lookups.
#[derive(Debug, Default)]
struct Mentioned {
    statuses: BTreeSet<Uuid>,
    criteria: BTreeSet<Uuid>,
    conversations: BTreeSet<Uuid>,
    messages: BTreeSet<Uuid>,
    repositories: BTreeSet<Uuid>,
    pull_requests: BTreeSet<Uuid>,
    pages: BTreeSet<Uuid>,
    decisions: BTreeSet<Uuid>,
    labels: BTreeSet<Uuid>,
    tasks: BTreeSet<Uuid>,
    moved_from: Option<String>,
    moved_to: bool,
    linked_tasks: BTreeSet<Uuid>,
}

fn uuid_at(value: &Value, key: &str) -> Option<Uuid> {
    value.get(key)?.as_str()?.parse().ok()
}

fn uuids_at(value: &Value, key: &str) -> Vec<Uuid> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.as_str()?.parse().ok())
        .collect()
}

fn strings_at(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.as_str().map(str::to_owned))
        .collect()
}

fn mentioned(rows: &[WorkItemTimelineRow]) -> Mentioned {
    let mut found = Mentioned::default();
    for row in rows {
        let (Some(kind), Some(payload)) = (row.activity_kind.as_deref(), &row.activity_payload)
        else {
            continue;
        };
        for key in ["previousStatusId", "statusId", "newStatusId"] {
            found.statuses.extend(uuid_at(payload, key));
        }
        found.criteria.extend(uuid_at(payload, "criterionId"));
        found
            .conversations
            .extend(uuid_at(payload, "conversationId"));
        found.repositories.extend(uuid_at(payload, "repositoryId"));
        for key in ["pullRequestId", "githubPullRequestId"] {
            found.pull_requests.extend(uuid_at(payload, key));
        }
        for key in ["labelsAdded", "labelsRemoved"] {
            found.labels.extend(uuids_at(payload, key));
        }
        found.tasks.extend(uuid_at(payload, "parentWorkItemId"));
        if let Some(parent) = payload.get("parent") {
            found.tasks.extend(uuid_at(parent, "from"));
            found.tasks.extend(uuid_at(parent, "to"));
        }
        if let Some(link) = payload.get("link")
            && let Some(id) = uuid_at(link, "id")
        {
            match link.get("kind").and_then(Value::as_str) {
                Some("page") => found.pages.insert(id),
                Some("decision") => found.decisions.insert(id),
                Some("message") => found.messages.insert(id),
                Some("conversation") => found.conversations.insert(id),
                _ => false,
            };
        }
        if matches!(
            kind,
            "dependency_added" | "dependency_removed" | "relation_added" | "relation_removed"
        ) {
            for key in [
                "blockingWorkItemId",
                "blockedWorkItemId",
                "workItemId",
                "otherWorkItemId",
            ] {
                found.linked_tasks.extend(uuid_at(payload, key));
            }
        }
        match kind {
            "work_item_created" => {
                if let Some(key) = payload.get("movedFrom").and_then(Value::as_str) {
                    found.moved_from = Some(key.to_owned());
                }
            }
            "work_item_archived" if payload.get("movedTo").is_some() => found.moved_to = true,
            _ => {}
        }
    }
    found
}

fn ids(set: &BTreeSet<Uuid>) -> Vec<Uuid> {
    set.iter().copied().collect()
}

/// One query per kind of thing the page mentions, each limited to this
/// project and to what the reader may see.
async fn look_up(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: Uuid,
    task_id: Uuid,
    found: Mentioned,
) -> Result<Lookups, AppError> {
    let pool = &state.pool;
    let viewer = principal.actor_id;
    let mut lookups = Lookups {
        this_task: Some(task_id),
        ..Lookups::default()
    };
    if !found.statuses.is_empty() {
        lookups.statuses = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, name FROM work_item_statuses WHERE project_id = $1 AND id = ANY($2)",
        )
        .bind(project_id)
        .bind(ids(&found.statuses))
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    if !found.criteria.is_empty() {
        lookups.criteria = sqlx::query_as::<_, (Uuid, String, i32)>(
            r#"
            SELECT id, left(text, $4), position FROM work_item_acceptance_criteria
            WHERE project_id = $1 AND work_item_id = $2 AND id = ANY($3)
            "#,
        )
        .bind(project_id)
        .bind(task_id)
        .bind(ids(&found.criteria))
        .bind(CRITERION_CHARACTERS)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(id, text, position)| (id, (text, position)))
        .collect();
    }
    // No conversation query skips conversation_visible_to (AGENTS.md): a
    // name the reader can't see is never returned.
    if !found.conversations.is_empty() {
        lookups.conversations = sqlx::query_as::<_, (Uuid, Option<String>)>(
            r#"
            SELECT id, CASE WHEN kind = 'dm' THEN NULL ELSE '#' || name END
            FROM conversations
            WHERE project_id = $1 AND id = ANY($2) AND conversation_visible_to(id, $3)
            "#,
        )
        .bind(project_id)
        .bind(ids(&found.conversations))
        .bind(viewer)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    if !found.messages.is_empty() {
        lookups.messages = sqlx::query_as::<_, (Uuid, Option<String>)>(
            r#"
            SELECT message.id,
                   CASE WHEN conversation.kind = 'dm' THEN NULL ELSE '#' || conversation.name END
            FROM messages message
            JOIN conversations conversation ON conversation.id = message.conversation_id
            WHERE message.project_id = $1 AND message.id = ANY($2)
              AND conversation_visible_to(message.conversation_id, $3)
            "#,
        )
        .bind(project_id)
        .bind(ids(&found.messages))
        .bind(viewer)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    if !found.decisions.is_empty() {
        lookups.decisions = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT id, title FROM project_decisions
            WHERE project_id = $1 AND id = ANY($2)
              AND (source_conversation_id IS NULL
                   OR conversation_visible_to(source_conversation_id, $3))
            "#,
        )
        .bind(project_id)
        .bind(ids(&found.decisions))
        .bind(viewer)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    if !found.pages.is_empty() {
        lookups.pages = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, title FROM project_pages WHERE project_id = $1 AND id = ANY($2)",
        )
        .bind(project_id)
        .bind(ids(&found.pages))
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    // Repositories and pull requests are named only for those whose role
    // (and an app's limit) lets them see code here.
    let sees_code = !crate::search::projects_allowing(
        pool,
        viewer,
        &[project_id],
        crate::policy::Permission::CodeView,
    )
    .await?
    .is_empty();
    if sees_code && !found.repositories.is_empty() {
        lookups.repositories = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT id, owner || '/' || name FROM repositories WHERE project_id = $1 AND id = ANY($2)",
        )
        .bind(project_id)
        .bind(ids(&found.repositories))
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    if sees_code && !found.pull_requests.is_empty() {
        lookups.pull_requests = sqlx::query_as::<_, (Uuid, i64, String)>(
            r#"
            SELECT id, pull_request_number, title FROM github_pull_requests
            WHERE project_id = $1 AND id = ANY($2)
              AND pull_request_visible_to(id, $3)
            "#,
        )
        .bind(project_id)
        .bind(ids(&found.pull_requests))
        .bind(viewer)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|(id, number, title)| (id, (number, title)))
        .collect();
    }
    if !found.labels.is_empty() {
        lookups.labels = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT id, name FROM project_labels
            WHERE project_id = $1 AND id = ANY($2) AND archived_at IS NULL
            "#,
        )
        .bind(project_id)
        .bind(ids(&found.labels))
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    if !found.tasks.is_empty() {
        lookups.tasks = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT item.id, project.project_key || '-' || item.number::text
            FROM task_rows item JOIN projects project ON project.id = item.project_id
            WHERE item.project_id = $1 AND item.id = ANY($2)
            "#,
        )
        .bind(project_id)
        .bind(ids(&found.tasks))
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    // A dependency or relation may name another project's task: it's named
    // only when the reader can reach that project through this connection.
    if !found.linked_tasks.is_empty() {
        let reachable = accessible_project_ids(state, principal).await?;
        lookups.linked_tasks = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT item.id, project.project_key || '-' || item.number::text
            FROM task_rows item JOIN projects project ON project.id = item.project_id
            WHERE item.id = ANY($1) AND (item.project_id = $2 OR item.project_id = ANY($3))
            "#,
        )
        .bind(ids(&found.linked_tasks))
        .bind(project_id)
        .bind(&reachable)
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();
    }
    // A move names another project's task, so it's named only when the
    // reader can reach that project through this connection.
    if found.moved_from.is_some() || found.moved_to {
        let reachable = accessible_project_ids(state, principal).await?;
        if let Some(key) = &found.moved_from {
            // Each move points every alias that led to the task at where it
            // went (task_move), so once this task has moved on too, the
            // original's alias points past it, to the same place as this
            // task's own alias.
            lookups.moved_from = sqlx::query_scalar::<_, Uuid>(
                r#"
                SELECT alias.source_work_item_id
                FROM work_item_key_aliases alias
                JOIN work_items source ON source.id = alias.source_work_item_id
                WHERE upper(alias.project_key || '-' || alias.number::text) = upper($2)
                  AND (alias.work_item_id = $1 OR alias.work_item_id = (
                      SELECT onward.work_item_id FROM work_item_key_aliases onward
                      WHERE onward.source_work_item_id = $1
                  ))
                  AND source.project_id = ANY($3)
                LIMIT 1
                "#,
            )
            .bind(task_id)
            .bind(key)
            .bind(&reachable)
            .fetch_optional(pool)
            .await?;
        }
        if found.moved_to {
            lookups.moved_to = sqlx::query_scalar::<_, String>(
                r#"
                SELECT project.project_key || '-' || item.number::text
                FROM work_item_key_aliases alias
                JOIN task_rows item ON item.id = alias.work_item_id
                JOIN projects project ON project.id = item.project_id
                WHERE alias.source_work_item_id = $1 AND item.project_id = ANY($2)
                "#,
            )
            .bind(task_id)
            .bind(&reachable)
            .fetch_optional(pool)
            .await?;
        }
    }
    Ok(lookups)
}

/// A page of a task's history, oldest first. task_open calls it only after
/// it found the task, so the reader passed its membership check.
pub(super) async fn task_history(
    state: &AppState,
    principal: &McpPrincipal,
    project_id: Uuid,
    task_id: Uuid,
    limit: u16,
    before: Option<i64>,
) -> Result<TaskHistory, AppError> {
    let mut page = fetch_timeline_rows(
        &state.pool,
        project_id,
        task_id,
        before,
        i64::from(limit.clamp(1, MAX_HISTORY_LIMIT)),
    )
    .await?;
    let total = sqlx::query_scalar::<_, i64>(
        "SELECT next_timeline_sequence - 1 FROM work_items WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(task_id)
    .fetch_one(&state.pool)
    .await?;
    let older_cursor = match page.rows.last() {
        Some(oldest) if page.has_more => Some(encode_work_item_timeline_cursor(
            project_id,
            task_id,
            oldest.sequence,
        )?),
        _ => None,
    };
    // IDs in each change's before and after become names, as the web
    // timeline shows them.
    let mut payloads = page
        .rows
        .iter_mut()
        .filter_map(|row| row.activity_payload.as_mut())
        .filter(|payload| payload.get("changes").is_some() || payload.get("statusId").is_some())
        .collect::<Vec<_>>();
    label_changes(&state.pool, project_id, &mut payloads).await?;
    let lookups = look_up(state, principal, project_id, task_id, mentioned(&page.rows)).await?;
    let entries = page
        .rows
        .iter()
        .rev()
        .map(|row| describe(row, &lookups))
        .collect();
    Ok(TaskHistory {
        entries,
        total,
        older_cursor,
    })
}

fn by_kind(actor_kind: &str) -> &str {
    match actor_kind {
        "human" => "person",
        "external_agent" => "agent",
        "execution_worker" => "worker",
        other => other,
    }
}

/// "a", "a and b", "a, b and c".
fn join_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn capitalized(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// A changed field, in the words people use for it.
fn field_words(field: &str) -> &str {
    match field {
        "blockedReason" => "blocker",
        "acceptanceCriteria" => "acceptance criteria",
        other => other,
    }
}

fn labels_phrase(names: &[String]) -> String {
    if names.len() == 1 {
        format!("label {}", names[0])
    } else {
        format!("labels {}", join_and(names))
    }
}

fn label_names(payload: &Value, key: &str, lookups: &Lookups) -> Vec<String> {
    uuids_at(payload, key)
        .into_iter()
        .filter_map(|id| lookups.labels.get(&id).cloned())
        .collect()
}

/// One history line from one timeline entry. Pure, so every payload shape
/// is unit-tested.
fn describe(row: &WorkItemTimelineRow, lookups: &Lookups) -> HistoryEntry {
    let mut entry = HistoryEntry {
        sequence: row.sequence,
        at: row.created_at.clone(),
        by: row.actor_display_name.clone(),
        by_handle: row.actor_handle.clone(),
        by_kind: by_kind(&row.actor_kind).to_owned(),
        via: row.via.clone(),
        ..HistoryEntry::default()
    };
    if row.kind == "comment" {
        entry.kind = "comment".to_owned();
        let (text, truncated) = truncate_text(
            row.comment_plain_text.as_deref().unwrap_or_default(),
            COMMENT_CHARACTERS,
        );
        entry.text = text;
        entry.text_truncated = truncated;
        return entry;
    }
    let kind = row.activity_kind.clone().unwrap_or_default();
    let summary = row.activity_summary.clone().unwrap_or_default();
    let payload = row.activity_payload.clone().unwrap_or(Value::Null);
    let text = match kind.as_str() {
        "work_item_created" => Some(created_text(&payload, lookups, &mut entry)),
        "work_item_updated" => updated_text(&payload, lookups, &mut entry),
        "work_item_archived" => Some(if payload.get("movedTo").is_some() {
            match &lookups.moved_to {
                Some(key) => format!("Moved to {key}; this copy was archived"),
                None => "Moved to another project; this copy was archived".to_owned(),
            }
        } else {
            "Archived".to_owned()
        }),
        "work_item_restored" => Some("Restored".to_owned()),
        "criterion_created"
        | "criterion_updated"
        | "criterion_completed"
        | "criterion_reopened"
        | "criterion_removed" => Some(criterion_text(&kind, &payload, lookups, &mut entry)),
        "thread_linked" | "thread_unlinked" => {
            let verb = if kind == "thread_linked" {
                "Linked"
            } else {
                "Unlinked"
            };
            Some(
                match uuid_at(&payload, "conversationId")
                    .and_then(|id| lookups.conversations.get(&id))
                {
                    Some(Some(name)) => format!("{verb} thread {name}"),
                    _ => format!("{verb} a thread you can't see"),
                },
            )
        }
        "repository_linked" | "repository_unlinked" | "repository_primary_changed" => {
            let name = uuid_at(&payload, "repositoryId")
                .and_then(|id| lookups.repositories.get(&id))
                .map_or_else(
                    || "a repository".to_owned(),
                    |name| format!("repository {name}"),
                );
            Some(match kind.as_str() {
                "repository_linked" => match payload.get("role").and_then(Value::as_str) {
                    Some(role) => format!("Linked {name} ({role})"),
                    None => format!("Linked {name}"),
                },
                "repository_unlinked" => format!("Unlinked {name}"),
                _ => format!("Made {name} the primary repository"),
            })
        }
        "dependency_added" | "dependency_removed" => Some(dependency_text(
            kind == "dependency_added",
            &payload,
            lookups,
        )),
        "relation_added" | "relation_removed" => {
            Some(relation_text(kind == "relation_added", &payload, lookups))
        }
        "worker_invoked" => Some(
            match uuid_at(&payload, "conversationId").and_then(|id| lookups.conversations.get(&id))
            {
                Some(Some(name)) => format!("Started a run in thread {name}"),
                _ => "Started a run in a thread you can't see".to_owned(),
            },
        ),
        "github_automation_transition" => {
            let status = |id_key: &str, name_key: &str| {
                uuid_at(&payload, id_key)
                    .and_then(|id| lookups.statuses.get(&id).cloned())
                    .or_else(|| payload.get(name_key)?.as_str().map(str::to_owned))
            };
            let from = status("previousStatusId", "previousStatus");
            let to = status("newStatusId", "newStatus");
            let pull_request = payload
                .get("pullRequestNumber")
                .and_then(Value::as_i64)
                .map(|number| format!(" (pull request #{number})"))
                .unwrap_or_default();
            entry.from_status.clone_from(&from);
            entry.to_status.clone_from(&to);
            Some(match (from, to) {
                (Some(from), Some(to)) => {
                    format!("GitHub moved it from {from} to {to}{pull_request}")
                }
                _ => format!("GitHub moved it{pull_request}"),
            })
        }
        _ => None,
    };
    entry.kind = kind;
    entry.text = text.unwrap_or(summary);
    entry
}

fn created_text(payload: &Value, lookups: &Lookups, entry: &mut HistoryEntry) -> String {
    if let Some(key) = payload.get("movedFrom").and_then(Value::as_str) {
        entry.from_task_id = lookups.moved_from.map(|id| id.to_string());
        return format!("Moved here from {key}");
    }
    let mut text = match uuid_at(payload, "parentWorkItemId") {
        Some(parent) => match lookups.tasks.get(&parent) {
            Some(key) => format!("Created this task as a subtask of {key}"),
            None => "Created this task as a subtask".to_owned(),
        },
        None => "Created this task".to_owned(),
    };
    let mut with = Vec::new();
    let labels = label_names(payload, "labelsAdded", lookups);
    if !labels.is_empty() {
        with.push(labels_phrase(&labels));
    }
    match payload.get("acceptanceCriteria").and_then(Value::as_i64) {
        Some(1) => with.push("1 acceptance criterion".to_owned()),
        Some(count) if count > 1 => with.push(format!("{count} acceptance criteria")),
        _ => {}
    }
    if !with.is_empty() {
        text.push_str(" with ");
        text.push_str(&with.join(" and "));
    }
    text
}

/// What a `work_item_updated` entry did, or None to fall back to its stored
/// summary. One entry can carry several changes made in one call.
fn updated_text(payload: &Value, lookups: &Lookups, entry: &mut HistoryEntry) -> Option<String> {
    if payload.get("claim").and_then(Value::as_bool) == Some(true) {
        return Some("Started working on this".to_owned());
    }
    let linked = payload
        .get("linked")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let verb = if linked { "Linked" } else { "Unlinked" };
    if let Some(pull_request) = uuid_at(payload, "pullRequestId") {
        return Some(match lookups.pull_requests.get(&pull_request) {
            Some((number, title)) => format!("{verb} pull request #{number} {title}"),
            None => format!("{verb} a pull request"),
        });
    }
    if let Some(link) = payload.get("link") {
        let id = uuid_at(link, "id");
        return Some(match link.get("kind").and_then(Value::as_str) {
            Some("page") => match id.and_then(|id| lookups.pages.get(&id)) {
                Some(title) => format!("{verb} page “{title}”"),
                None => format!("{verb} a page"),
            },
            Some("decision") => match id.and_then(|id| lookups.decisions.get(&id)) {
                Some(title) => format!("{verb} decision “{title}”"),
                None => format!("{verb} a decision you can't see"),
            },
            Some("message") => match id.and_then(|id| lookups.messages.get(&id)) {
                Some(Some(name)) => format!("{verb} a message in {name}"),
                Some(None) => format!("{verb} a message in a DM"),
                None => format!("{verb} a message you can't see"),
            },
            Some("conversation") => match id.and_then(|id| lookups.conversations.get(&id)) {
                Some(Some(name)) => format!("{verb} conversation {name}"),
                Some(None) => format!("{verb} a DM"),
                None => format!("{verb} a conversation you can't see"),
            },
            _ => format!("{verb} something"),
        });
    }

    let fields = strings_at(payload, "changedFields");
    entry.changed_fields.clone_from(&fields);
    let from = uuid_at(payload, "previousStatusId");
    let to = uuid_at(payload, "statusId").or_else(|| uuid_at(payload, "newStatusId"));
    let names = match (from, to) {
        (Some(from), Some(to)) if from != to => lookups
            .statuses
            .get(&from)
            .cloned()
            .zip(lookups.statuses.get(&to).cloned()),
        _ => None,
    };
    if let Some((from, to)) = &names {
        entry.from_status = Some(from.clone());
        entry.to_status = Some(to.clone());
    }
    let changes = payload.get("changes").and_then(Value::as_object);
    let detailed = |field: &str| {
        changes.is_some_and(|changes| {
            changes.contains_key(field) && (change_label(field).is_some() || field == "description")
        })
    };
    let custom_changes = changes
        .map(|changes| {
            changes
                .iter()
                .filter(|(key, _)| key.starts_with(CUSTOM_FIELD_PREFIX))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // Fields with a clause of their own below.
    let others = fields
        .iter()
        .filter(|field| !matches!(field.as_str(), "status" | "labels" | "parent" | "position"))
        .filter(|field| !detailed(field))
        .filter(|field| !(field.as_str() == "fields" && !custom_changes.is_empty()))
        .map(|field| field_words(field).to_owned())
        .collect::<Vec<_>>();
    let position = payload
        .get("position")
        .or_else(|| payload.get("targetPosition"))
        .and_then(Value::as_i64);
    let mut clauses = Vec::new();
    let mut position_said = false;
    match &names {
        Some((from, to)) if others.is_empty() => {
            let mut clause = format!("moved from {from} to {to}");
            if let Some(position) = position {
                clause.push_str(&format!(" (position {position})"));
                position_said = true;
            }
            clauses.push(clause);
        }
        Some((from, to)) => clauses.push(format!(
            "changed status from {from} to {to}, and {}",
            join_and(&others)
        )),
        None => {
            let mut named = others;
            if fields.iter().any(|field| field == "status")
                || (from.is_some() && to.is_some() && from != to)
            {
                named.insert(0, "status".to_owned());
            }
            if !named.is_empty() {
                clauses.push(format!("changed {}", join_and(&named)));
            }
        }
    }
    if let Some(changes) = changes {
        for field in CHANGE_ORDER {
            if let (Some(label), Some(change)) = (change_label(field), changes.get(field)) {
                let side =
                    |side: &str| change_value(field, change.get(side).unwrap_or(&Value::Null));
                clauses.push(format!("{label}: {} → {}", side("from"), side("to")));
            }
        }
    }
    for (_, change) in &custom_changes {
        let side = |side: &str| match change.get(side) {
            None | Some(Value::Null) => "none".to_owned(),
            Some(value) => crate::fields::display_text(value),
        };
        let name = change
            .get("field")
            .and_then(Value::as_str)
            .unwrap_or("A field");
        clauses.push(format!("{name}: {} → {}", side("from"), side("to")));
    }
    let added = label_names(payload, "labelsAdded", lookups);
    let removed = label_names(payload, "labelsRemoved", lookups);
    if !added.is_empty() {
        clauses.push(format!("added {}", labels_phrase(&added)));
    }
    if !removed.is_empty() {
        clauses.push(format!("removed {}", labels_phrase(&removed)));
    }
    if added.is_empty() && removed.is_empty() && fields.iter().any(|field| field == "labels") {
        clauses.push("changed labels".to_owned());
    }
    if let Some(parent) = payload.get("parent") {
        clauses.push(match uuid_at(parent, "to") {
            Some(id) => match lookups.tasks.get(&id) {
                Some(key) => format!("put it under {key}"),
                None => "put it under another task".to_owned(),
            },
            None => "made it a top-level task".to_owned(),
        });
    }
    if let Some(position) = position
        && !position_said
    {
        clauses.push(format!("put it at position {position} in its column"));
    }
    let description = changes
        .and_then(|changes| changes.get("description"))
        .map(description_text);
    match (clauses.is_empty(), description) {
        (true, None) => None,
        (true, Some(description)) => Some(description),
        (false, None) => Some(capitalized(&clauses.join("; "))),
        (false, Some(description)) => Some(format!(
            "{}\n{description}",
            capitalized(&clauses.join("; "))
        )),
    }
}

/// The fields a change line names, in the order they're read out.
const CHANGE_ORDER: [&str; 9] = [
    "title",
    "type",
    "priority",
    "assignee",
    "milestone",
    "startDate",
    "dueDate",
    "estimate",
    "blockedReason",
];

/// A field's name in a change line, for the fields that have one.
fn change_label(field: &str) -> Option<&'static str> {
    Some(match field {
        "title" => "Title",
        "type" => "Type",
        "priority" => "Priority",
        "assignee" => "Assignee",
        "milestone" => "Milestone",
        "startDate" => "Start",
        "dueDate" => "Due",
        "estimate" => "Estimate",
        "blockedReason" => "Blocker",
        _ => return None,
    })
}

/// One side of a change, in plain words: "none" for nothing (or something
/// that no longer exists), a priority by its name, text in quotes.
fn change_value(field: &str, value: &Value) -> String {
    match value {
        Value::Null => "none".to_owned(),
        Value::String(text) => match field {
            "priority" => match text.as_str() {
                "no_priority" => "No priority".to_owned(),
                other => capitalized(other),
            },
            "title" | "blockedReason" => format!("“{text}”"),
            _ => text.clone(),
        },
        Value::Number(number) => match number.as_f64() {
            Some(value) if value.fract() == 0.0 && value.abs() < 1e15 => {
                format!("{value:.0}")
            }
            Some(value) => value.to_string(),
            None => number.to_string(),
        },
        other => other.to_string(),
    }
}

/// A description change as its short line diff.
fn description_text(change: &Value) -> String {
    let diff = change
        .get("diff")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (diff, cut) = truncate_text(diff, DIFF_CHARACTERS);
    let mut text = if diff.is_empty() {
        "Edited the description".to_owned()
    } else {
        format!("Edited the description:\n{diff}")
    };
    if cut || change.get("truncated").and_then(Value::as_bool) == Some(true) {
        text.push_str("\n(the rest of the change is left out)");
    }
    text
}

fn linked_task(id: Option<Uuid>, lookups: &Lookups) -> String {
    id.and_then(|id| lookups.linked_tasks.get(&id))
        .cloned()
        .unwrap_or_else(|| "a task you can't see".to_owned())
}

fn dependency_text(added: bool, payload: &Value, lookups: &Lookups) -> String {
    let blocking = uuid_at(payload, "blockingWorkItemId");
    let blocked = uuid_at(payload, "blockedWorkItemId");
    if blocked.is_some() && blocked == lookups.this_task {
        let other = linked_task(blocking, lookups);
        if added {
            format!("Blocked by {other}")
        } else {
            format!("No longer blocked by {other}")
        }
    } else {
        let other = linked_task(blocked, lookups);
        if added {
            format!("Blocks {other}")
        } else {
            format!("No longer blocks {other}")
        }
    }
}

fn relation_text(added: bool, payload: &Value, lookups: &Lookups) -> String {
    let task = uuid_at(payload, "workItemId");
    let other = uuid_at(payload, "otherWorkItemId");
    let this_side = task.is_some() && task == lookups.this_task;
    let name = linked_task(if this_side { other } else { task }, lookups);
    let duplicate = payload.get("relation").and_then(Value::as_str) == Some("duplicates");
    match (duplicate, this_side, added) {
        (false, _, true) => format!("Related to {name}"),
        (false, _, false) => format!("No longer related to {name}"),
        (true, true, true) => format!("Marked a duplicate of {name}"),
        (true, true, false) => format!("No longer a duplicate of {name}"),
        (true, false, true) => format!("Duplicated by {name}"),
        (true, false, false) => format!("No longer duplicated by {name}"),
    }
}

fn criterion_text(
    kind: &str,
    payload: &Value,
    lookups: &Lookups,
    entry: &mut HistoryEntry,
) -> String {
    let id = uuid_at(payload, "criterionId");
    entry.criterion_id = id.map(|id| id.to_string());
    let position = payload.get("position").and_then(Value::as_i64);
    if kind == "criterion_removed" {
        return match position {
            Some(position) => format!("Removed criterion {position}"),
            None => "Removed a criterion".to_owned(),
        };
    }
    let quoted = id
        .and_then(|id| lookups.criteria.get(&id))
        .map(|(text, _)| format!("“{text}”"));
    let fields = strings_at(payload, "fields");
    let moved_only = fields == ["position"];
    let verb = match kind {
        "criterion_created" => "Added",
        "criterion_completed" => "Checked",
        "criterion_reopened" => "Unchecked",
        _ if moved_only => "Moved",
        _ => "Edited",
    };
    let Some(quoted) = quoted else {
        return format!("{verb} a criterion (since removed)");
    };
    match (kind, position) {
        ("criterion_updated", Some(position)) if moved_only => {
            format!("Moved {quoted} to position {position}")
        }
        ("criterion_updated", Some(position)) if fields.iter().any(|field| field == "position") => {
            format!("Edited {quoted} and moved it to position {position}")
        }
        _ => format!("{verb} {quoted}"),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use uuid::Uuid;

    use super::{Lookups, describe};
    use crate::work_items::WorkItemTimelineRow;

    fn activity(kind: &str, summary: &str, payload: Value) -> WorkItemTimelineRow {
        WorkItemTimelineRow {
            sequence: 4,
            kind: "activity".to_owned(),
            created_at: "2026-10-01 12:00:00+00".to_owned(),
            activity_id: Some(Uuid::now_v7()),
            activity_kind: Some(kind.to_owned()),
            activity_summary: Some(summary.to_owned()),
            activity_payload: Some(payload),
            comment_id: None,
            comment_content: None,
            comment_plain_text: None,
            comment_revision: None,
            comment_edited_at: None,
            comment_removed_at: None,
            comment_removed_by_id: None,
            comment_removed_by_kind: None,
            comment_removed_by_name: None,
            comment_removed_by_handle: None,
            reply_to_comment_id: None,
            reply_to_author_name: None,
            reply_to_text: None,
            reply_to_removed: None,
            actor_id: Uuid::now_v7(),
            actor_kind: "external_agent".to_owned(),
            actor_display_name: "Kai".to_owned(),
            actor_handle: "kai".to_owned(),
            via: None,
        }
    }

    fn statuses(lookups: &mut Lookups, names: &[&str]) -> Vec<Uuid> {
        names
            .iter()
            .map(|name| {
                let id = Uuid::now_v7();
                lookups.statuses.insert(id, (*name).to_owned());
                id
            })
            .collect()
    }

    #[test]
    fn status_changes_read_from_and_to_in_both_payload_styles() {
        let mut lookups = Lookups::default();
        let ids = statuses(&mut lookups, &["Ready", "In Progress"]);
        let entry = describe(
            &activity(
                "work_item_updated",
                "Work Item updated through MCP",
                json!({"changedFields": ["status"], "previousStatusId": ids[0], "statusId": ids[1]}),
            ),
            &lookups,
        );
        assert_eq!(entry.text, "Moved from Ready to In Progress");
        assert_eq!(entry.from_status.as_deref(), Some("Ready"));
        assert_eq!(entry.to_status.as_deref(), Some("In Progress"));
        assert_eq!(entry.changed_fields, ["status"]);
        assert_eq!(entry.by_kind, "agent");

        let moved = describe(
            &activity(
                "work_item_updated",
                "Work Item moved",
                json!({"previousStatusId": ids[0], "statusId": ids[1], "targetPosition": 1}),
            ),
            &lookups,
        );
        assert_eq!(moved.text, "Moved from Ready to In Progress (position 1)");

        let github = describe(
            &activity(
                "github_automation_transition",
                "GitHub pull request automation changed status",
                json!({
                    "previousStatusId": ids[0], "previousStatus": "Old name",
                    "newStatusId": Uuid::now_v7(), "newStatus": "Review",
                    "pullRequestNumber": 42
                }),
            ),
            &lookups,
        );
        assert_eq!(
            github.text,
            "GitHub moved it from Ready to Review (pull request #42)"
        );
        assert_eq!(github.to_status.as_deref(), Some("Review"));

        let together = describe(
            &activity(
                "work_item_updated",
                "Work Item updated",
                json!({
                    "changedFields": ["title", "status", "priority"],
                    "previousStatusId": ids[1], "statusId": ids[0]
                }),
            ),
            &lookups,
        );
        assert_eq!(
            together.text,
            "Changed status from In Progress to Ready, and title and priority"
        );
    }

    #[test]
    fn older_entries_without_status_ids_still_read() {
        let entry = describe(
            &activity(
                "work_item_updated",
                "Work Item updated",
                json!({"changedFields": ["title", "status", "blockedReason"]}),
            ),
            &Lookups::default(),
        );
        assert_eq!(entry.text, "Changed status, title and blocker");
        assert!(entry.from_status.is_none());
        let claim = describe(
            &activity(
                "work_item_updated",
                "Started working on this",
                json!({"claim": true}),
            ),
            &Lookups::default(),
        );
        assert_eq!(claim.text, "Started working on this");
        let unknown = describe(
            &activity("something_new", "Something new happened", json!({})),
            &Lookups::default(),
        );
        assert_eq!(unknown.kind, "something_new");
        assert_eq!(unknown.text, "Something new happened");
        let empty = describe(
            &activity("work_item_updated", "Work Item updated", json!({})),
            &Lookups::default(),
        );
        assert_eq!(empty.text, "Work Item updated");
    }

    #[test]
    fn labels_parents_and_places_read_as_one_change() {
        let mut lookups = Lookups::default();
        let (mobile, sync, archived) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        lookups.labels.insert(mobile, "Mobile".to_owned());
        lookups.labels.insert(sync, "Sync".to_owned());
        let parent = Uuid::now_v7();
        lookups.tasks.insert(parent, "WORK-1".to_owned());
        let entry = describe(
            &activity(
                "work_item_updated",
                "Work Item updated through MCP",
                json!({
                    "changedFields": ["priority", "parent", "position", "labels"],
                    "labelsAdded": [mobile, sync, archived],
                    "labelsRemoved": [archived],
                    "parent": {"from": null, "to": parent},
                    "position": 2
                }),
            ),
            &lookups,
        );
        assert_eq!(
            entry.text,
            "Changed priority; added labels Mobile and Sync; put it under WORK-1; put it at position 2 in its column"
        );
        let top = describe(
            &activity(
                "work_item_updated",
                "Task reordered on the board through MCP",
                json!({"changedFields": ["parent"], "parent": {"from": parent, "to": null}}),
            ),
            &lookups,
        );
        assert_eq!(top.text, "Made it a top-level task");
        let hidden = describe(
            &activity(
                "work_item_updated",
                "Work Item updated",
                json!({"changedFields": ["labels"], "labelsRemoved": [archived]}),
            ),
            &lookups,
        );
        assert_eq!(hidden.text, "Changed labels");
        let reordered = describe(
            &activity(
                "work_item_updated",
                "Task reordered on the board through MCP",
                json!({"changedFields": ["position"], "position": 1}),
            ),
            &lookups,
        );
        assert_eq!(reordered.text, "Put it at position 1 in its column");
    }

    #[test]
    fn moves_between_projects_name_only_what_the_reader_reaches() {
        let original = Uuid::now_v7();
        let mut lookups = Lookups {
            moved_from: Some(original),
            ..Lookups::default()
        };
        let created = describe(
            &activity(
                "work_item_created",
                "Work Item moved here from AW-7",
                json!({"movedFrom": "AW-7", "number": 3}),
            ),
            &lookups,
        );
        assert_eq!(created.text, "Moved here from AW-7");
        assert_eq!(created.from_task_id, Some(original.to_string()));
        lookups.moved_from = None;
        let unreachable = describe(
            &activity(
                "work_item_created",
                "Work Item moved here from AW-7",
                json!({"movedFrom": "AW-7"}),
            ),
            &lookups,
        );
        assert!(unreachable.from_task_id.is_none());

        let archived = |lookups: &Lookups| {
            describe(
                &activity(
                    "work_item_archived",
                    "Work Item moved to AWD-3",
                    json!({"movedTo": "AWD-3", "previousVersion": 4}),
                ),
                lookups,
            )
            .text
        };
        assert_eq!(
            archived(&lookups),
            "Moved to another project; this copy was archived"
        );
        lookups.moved_to = Some("AWD-3".to_owned());
        assert_eq!(archived(&lookups), "Moved to AWD-3; this copy was archived");
        assert_eq!(
            describe(
                &activity(
                    "work_item_archived",
                    "Archived",
                    json!({"previousVersion": 2})
                ),
                &lookups
            )
            .text,
            "Archived"
        );
    }

    #[test]
    fn private_threads_and_decisions_stay_unnamed() {
        let mut lookups = Lookups::default();
        let (seen, private) = (Uuid::now_v7(), Uuid::now_v7());
        lookups
            .conversations
            .insert(seen, Some("#offline-plan".to_owned()));
        let linked = |id: Uuid| {
            describe(
                &activity(
                    "thread_linked",
                    "Thread linked through MCP",
                    json!({"conversationId": id}),
                ),
                &lookups,
            )
            .text
        };
        assert_eq!(linked(seen), "Linked thread #offline-plan");
        assert_eq!(linked(private), "Linked a thread you can't see");
        let decision = describe(
            &activity(
                "work_item_updated",
                "Decision unlinked through MCP",
                json!({"link": {"kind": "decision", "id": private}, "linked": false}),
            ),
            &lookups,
        );
        assert_eq!(decision.text, "Unlinked a decision you can't see");
        let worker = describe(
            &activity(
                "worker_invoked",
                "Work with Worker started",
                json!({"conversationId": private, "repositoryId": Uuid::now_v7()}),
            ),
            &lookups,
        );
        assert_eq!(worker.text, "Started a run in a thread you can't see");
        let pull_request = Uuid::now_v7();
        lookups
            .pull_requests
            .insert(pull_request, (42, "Offline mode".to_owned()));
        let unlinked = describe(
            &activity(
                "work_item_updated",
                "Pull request unlinked through MCP",
                json!({"pullRequestId": pull_request, "linked": false}),
            ),
            &lookups,
        );
        assert_eq!(unlinked.text, "Unlinked pull request #42 Offline mode");
    }

    #[test]
    fn criteria_read_with_their_current_text() {
        let mut lookups = Lookups::default();
        let (kept, gone) = (Uuid::now_v7(), Uuid::now_v7());
        lookups
            .criteria
            .insert(kept, ("Mirrors prepare".to_owned(), 2));
        let text = |kind: &str, payload: Value| {
            describe(&activity(kind, "Acceptance criterion", payload), &lookups)
        };
        let added = text(
            "criterion_created",
            json!({"criterionId": kept, "position": 1}),
        );
        assert_eq!(added.text, "Added “Mirrors prepare”");
        assert_eq!(added.criterion_id, Some(kept.to_string()));
        assert_eq!(
            text(
                "criterion_updated",
                json!({"criterionId": kept, "position": 2, "fields": ["position"]})
            )
            .text,
            "Moved “Mirrors prepare” to position 2"
        );
        assert_eq!(
            text("criterion_updated", json!({"criterionId": kept})).text,
            "Edited “Mirrors prepare”"
        );
        assert_eq!(
            text("criterion_completed", json!({"criterionId": kept})).text,
            "Checked “Mirrors prepare”"
        );
        assert_eq!(
            text(
                "criterion_created",
                json!({"criterionId": gone, "position": 3})
            )
            .text,
            "Added a criterion (since removed)"
        );
        assert_eq!(
            text(
                "criterion_removed",
                json!({"criterionId": gone, "position": 3})
            )
            .text,
            "Removed criterion 3"
        );
    }

    #[test]
    fn comments_are_cut_on_a_character_boundary() {
        let mut row = activity("", "", Value::Null);
        row.kind = "comment".to_owned();
        row.activity_kind = None;
        row.comment_plain_text = Some("界".repeat(1_200));
        row.via = Some("Codex".to_owned());
        let entry = describe(&row, &Lookups::default());
        assert_eq!(entry.kind, "comment");
        assert_eq!(entry.text.chars().count(), 1_000);
        assert!(entry.text_truncated);
        assert_eq!(entry.via.as_deref(), Some("Codex"));
        row.comment_plain_text = Some("Repro attached".to_owned());
        let entry = describe(&row, &Lookups::default());
        assert_eq!(entry.text, "Repro attached");
        assert!(!entry.text_truncated);
    }
    #[test]
    fn changes_read_as_before_and_after() {
        let entry = describe(
            &activity(
                "work_item_updated",
                "Work Item updated through MCP",
                json!({
                    "changedFields": ["priority", "dueDate", "estimate", "milestone"],
                    "changes": {
                        "priority": {"from": "high", "to": "urgent"},
                        "dueDate": {"from": null, "to": "2026-10-20"},
                        "estimate": {"from": 3.0, "to": 2.5},
                        "milestone": {"from": "Beta", "to": null}
                    }
                }),
            ),
            &Lookups::default(),
        );
        assert_eq!(
            entry.text,
            "Priority: High → Urgent; Milestone: Beta → none; Due: none → 2026-10-20; Estimate: 3 → 2.5"
        );
        assert_eq!(entry.changed_fields.len(), 4);

        let mut lookups = Lookups::default();
        let ids = statuses(&mut lookups, &["Ready", "Doing"]);
        let mixed = describe(
            &activity(
                "work_item_updated",
                "Work Item updated",
                json!({
                    "changedFields": ["status", "title", "description"],
                    "previousStatusId": ids[0], "statusId": ids[1],
                    "changes": {
                        "title": {"from": "Old", "to": "New"},
                        "description": {"diff": "-one\n+two", "truncated": false}
                    }
                }),
            ),
            &lookups,
        );
        assert_eq!(
            mixed.text,
            "Moved from Ready to Doing; Title: “Old” → “New”\nEdited the description:\n-one\n+two"
        );
    }

    #[test]
    fn dependencies_and_relations_name_only_reachable_tasks() {
        let (this, seen, hidden) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
        let mut lookups = Lookups {
            this_task: Some(this),
            ..Lookups::default()
        };
        lookups.linked_tasks.insert(seen, "WORK-2".to_owned());
        let text = |kind: &str, payload: Value| {
            describe(&activity(kind, "Linked", payload), &lookups).text
        };
        assert_eq!(
            text(
                "dependency_added",
                json!({"blockingWorkItemId": seen, "blockedWorkItemId": this})
            ),
            "Blocked by WORK-2"
        );
        assert_eq!(
            text(
                "dependency_removed",
                json!({"blockingWorkItemId": this, "blockedWorkItemId": hidden})
            ),
            "No longer blocks a task you can't see"
        );
        assert_eq!(
            text(
                "relation_added",
                json!({"relation": "duplicates", "workItemId": this, "otherWorkItemId": seen})
            ),
            "Marked a duplicate of WORK-2"
        );
        assert_eq!(
            text(
                "relation_added",
                json!({"relation": "duplicates", "workItemId": seen, "otherWorkItemId": this})
            ),
            "Duplicated by WORK-2"
        );
        assert_eq!(
            text(
                "relation_removed",
                json!({"relation": "relates_to", "workItemId": seen, "otherWorkItemId": this})
            ),
            "No longer related to WORK-2"
        );
    }
}
