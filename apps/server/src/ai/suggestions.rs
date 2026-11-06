//! AI2 and AI4: the review queue.
//!
//! A suggestion is about one task (`work_item_id`) and, for link kinds,
//! another (`other_work_item_id`): "blocks" means the other task blocks
//! this one, "duplicates" that this one duplicates the other, "parent" that
//! the other is this one's parent. Field kinds carry a value.
//!
//! Who sees what: every read and every accept checks
//! `ai_suggestion_visible_to`, which rechecks the reader's access to both
//! tasks' projects and to every task and message the suggestion cites, at
//! the moment of reading. A suggestion drawn from something only one person
//! can see has that person as its audience and is shown to nobody else.
//!
//! Accepting writes through the same commands a person's own change uses
//! (`change_dependencies`, `change_relation`, `update_locked_work_item`,
//! `change_criteria`), so history, timeline and live updates stay the same,
//! and marks the history entries "via AI".

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, PgExecutor, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    error::AppError,
    policy::Action,
    work_item_commands::{
        AuditVersionMetadata, CommandActor, CriteriaCommand, CriteriaOperation, CriteriaOutcome,
        TaskFieldChanges, UpdateWorkItemCommand, UpdateWorkItemOutcome, authorize_mutation,
        lock_membership, lock_work_item, normalize_criterion_text, update_locked_work_item,
    },
    work_item_labels::LabelDelta,
    work_item_planning::{self as planning, Direction, Reader, RelationKind, TaskRef},
};

/// Most suggestions one page lists.
pub const MAX_PAGE: i64 = 100;
/// Suggestions below this confidence are dropped, not queued.
pub const MIN_CONFIDENCE: f32 = 0.5;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Blocks,
    RelatesTo,
    Duplicates,
    Parent,
    BlockedReason,
    Label,
    Priority,
    Type,
    Criteria,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocks => "blocks",
            Self::RelatesTo => "relates_to",
            Self::Duplicates => "duplicates",
            Self::Parent => "parent",
            Self::BlockedReason => "blocked_reason",
            Self::Label => "label",
            Self::Priority => "priority",
            Self::Type => "type",
            Self::Criteria => "criteria",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [
            Self::Blocks,
            Self::RelatesTo,
            Self::Duplicates,
            Self::Parent,
            Self::BlockedReason,
            Self::Label,
            Self::Priority,
            Self::Type,
            Self::Criteria,
        ]
        .into_iter()
        .find(|kind| kind.as_str() == value)
    }

    pub fn is_link(self) -> bool {
        matches!(
            self,
            Self::Blocks | Self::RelatesTo | Self::Duplicates | Self::Parent
        )
    }
}

/// Where a suggestion came from. Rechecked on every read.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceRef {
    Task {
        id: Uuid,
    },
    Message {
        #[serde(rename = "messageId")]
        message_id: Uuid,
        #[serde(rename = "conversationId")]
        conversation_id: Uuid,
    },
    /// A cheap signal that picked the pair: references, similar, milestone,
    /// labels.
    Signal {
        name: String,
    },
}

/// What made a suggestion.
#[derive(Clone, Debug)]
pub struct Provenance {
    pub provider: String,
    pub model: String,
    pub prompt_id: &'static str,
    pub prompt_version: i32,
}

/// A suggestion to queue.
#[derive(Clone, Debug)]
pub struct NewSuggestion {
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub work_item_id: Uuid,
    pub kind: Kind,
    /// (project, task) for link kinds.
    pub other: Option<(Uuid, Uuid)>,
    pub value: Value,
    pub confidence: f32,
    pub reason: String,
    pub sources: Vec<SourceRef>,
    /// Only this person may see it.
    pub audience: Option<Uuid>,
    pub origin: &'static str,
    pub provenance: Provenance,
    pub requested_by: Option<Uuid>,
}

/// One key per pair of tasks for every link kind and either direction, so a
/// dismissed pair never comes back as another kind; one per task, field and
/// value otherwise.
pub fn pair_key(kind: Kind, work_item_id: Uuid, other: Option<Uuid>, value: &Value) -> String {
    match other {
        Some(other) if kind.is_link() => {
            let (first, second) = if work_item_id < other {
                (work_item_id, other)
            } else {
                (other, work_item_id)
            };
            format!("link:{first}:{second}")
        }
        _ => {
            let detail = match kind {
                Kind::Label => value["labelId"].as_str().unwrap_or_default().to_owned(),
                Kind::Priority => value["priority"].as_str().unwrap_or_default().to_owned(),
                Kind::Type => value["typeId"].as_str().unwrap_or_default().to_owned(),
                // One open blocked-reason or criteria suggestion per task.
                _ => String::new(),
            };
            format!("{}:{work_item_id}:{detail}", kind.as_str())
        }
    }
}

/// Cleans the model's one-line reason: one line, at most 300 characters.
pub fn clean_reason(reason: &str) -> String {
    let line = reason
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .take(300)
        .collect::<String>();
    if line.is_empty() {
        "Suggested by AI.".to_owned()
    } else {
        line
    }
}

/// Queues a suggestion unless the same pair (or task, field and value) was
/// already suggested to this audience or dismissed by anyone. Returns its id
/// when it was queued.
pub async fn insert<'e, E: PgExecutor<'e>>(
    executor: E,
    suggestion: &NewSuggestion,
) -> Result<Option<Uuid>, AppError> {
    if suggestion.confidence < MIN_CONFIDENCE {
        return Ok(None);
    }
    let key = pair_key(
        suggestion.kind,
        suggestion.work_item_id,
        suggestion.other.map(|(_, id)| id),
        &suggestion.value,
    );
    let id = Uuid::now_v7();
    let inserted = sqlx::query_scalar::<_, Uuid>(
        r#"
        INSERT INTO ai_suggestions (
            id, organization_id, project_id, work_item_id, kind,
            other_project_id, other_work_item_id, value, confidence, reason,
            sources, pair_key, audience_actor_id, origin, provider, model,
            prompt_id, prompt_version, requested_by_actor_id
        )
        SELECT $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15,
               $16, $17, $18, $19
        WHERE NOT EXISTS (
            SELECT 1 FROM ai_suggestions
            WHERE project_id = $3 AND pair_key = $12 AND status = 'dismissed'
        )
        ON CONFLICT DO NOTHING
        RETURNING id
        "#,
    )
    .bind(id)
    .bind(suggestion.organization_id)
    .bind(suggestion.project_id)
    .bind(suggestion.work_item_id)
    .bind(suggestion.kind.as_str())
    .bind(suggestion.other.map(|(project, _)| project))
    .bind(suggestion.other.map(|(_, task)| task))
    .bind(&suggestion.value)
    .bind(suggestion.confidence.clamp(0.0, 1.0))
    .bind(clean_reason(&suggestion.reason))
    .bind(serde_json::to_value(&suggestion.sources).unwrap_or_else(|_| json!([])))
    .bind(&key)
    .bind(suggestion.audience)
    .bind(suggestion.origin)
    .bind(&suggestion.provenance.provider)
    .bind(
        suggestion
            .provenance
            .model
            .chars()
            .take(200)
            .collect::<String>(),
    )
    .bind(suggestion.provenance.prompt_id)
    .bind(suggestion.provenance.prompt_version)
    .bind(suggestion.requested_by)
    .fetch_optional(executor)
    .await?;
    Ok(inserted)
}

/// A source, as the reader sees it.
#[derive(Clone, Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum SourceView {
    Task {
        task: TaskRef,
    },
    Message {
        message_id: Uuid,
        conversation_id: Uuid,
        conversation_name: String,
        href: String,
    },
    Signal {
        name: String,
    },
}

/// A suggestion as one reader sees it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionView {
    pub id: Uuid,
    pub kind: Kind,
    pub task: TaskRef,
    pub other: Option<TaskRef>,
    pub value: Value,
    pub confidence: f32,
    pub reason: String,
    pub sources: Vec<SourceView>,
    pub status: String,
    pub origin: String,
    /// Only the reader can see it.
    pub personal: bool,
    pub provider: String,
    pub model: String,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

#[derive(FromRow)]
struct Row {
    id: Uuid,
    project_id: Uuid,
    work_item_id: Uuid,
    kind: String,
    other_work_item_id: Option<Uuid>,
    value: Value,
    confidence: f32,
    reason: String,
    sources: Value,
    status: String,
    origin: String,
    audience_actor_id: Option<Uuid>,
    provider: String,
    model: String,
    created_at: DateTime<Utc>,
    decided_at: Option<DateTime<Utc>>,
}

/// Which suggestions to list.
#[derive(Clone, Debug, Default)]
pub struct Filter {
    /// `pending` (the default), `accepted`, `dismissed`, `applied` or `all`.
    pub status: Option<String>,
    /// Only those about this task, either end.
    pub work_item_id: Option<Uuid>,
    pub kinds: Vec<Kind>,
    pub limit: i64,
    /// From the previous page.
    pub cursor: Option<String>,
}

/// A page of suggestions.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub suggestions: Vec<SuggestionView>,
    pub next_cursor: Option<String>,
    /// Pending suggestions the reader can see in the project.
    pub pending_count: i64,
}

/// Suggestions that still make sense: both tasks open and not archived, the
/// link not already there.
const STILL_USEFUL: &str = r#"
    item.archived_at IS NULL
    AND item_status.semantic_category NOT IN ('completed', 'canceled')
    AND (suggestion.other_work_item_id IS NULL OR (
        other.archived_at IS NULL
        AND (suggestion.kind <> 'blocks'
             OR other_status.semantic_category NOT IN ('completed', 'canceled'))
        AND NOT EXISTS (
            SELECT 1 FROM work_item_dependencies dependency
            WHERE (dependency.blocked_work_item_id = suggestion.work_item_id
                   AND dependency.blocking_work_item_id = suggestion.other_work_item_id)
               OR (dependency.blocked_work_item_id = suggestion.other_work_item_id
                   AND dependency.blocking_work_item_id = suggestion.work_item_id))
        AND NOT EXISTS (
            SELECT 1 FROM work_item_relations relation
            WHERE (relation.work_item_id = suggestion.work_item_id
                   AND relation.other_work_item_id = suggestion.other_work_item_id)
               OR (relation.work_item_id = suggestion.other_work_item_id
                   AND relation.other_work_item_id = suggestion.work_item_id))
    ))
"#;

const JOINS: &str = r#"
    JOIN work_items item
      ON item.project_id = suggestion.project_id AND item.id = suggestion.work_item_id
    JOIN work_item_statuses item_status
      ON item_status.project_id = item.project_id AND item_status.id = item.status_id
    LEFT JOIN work_items other
      ON other.project_id = suggestion.other_project_id AND other.id = suggestion.other_work_item_id
    LEFT JOIN work_item_statuses other_status
      ON other_status.project_id = other.project_id AND other_status.id = other.status_id
"#;

// An app acting for the reader sees only suggestions whose projects its
// current limit lets it see tasks in (`app_limit_projects`; NULL outside
// such a call). The role's "see tasks" is in ai_suggestion_visible_to.
const WITHIN_APP_LIMIT_4: &str = "$4::uuid[] IS NULL OR (suggestion.project_id = ANY($4) AND (suggestion.other_project_id IS NULL OR suggestion.other_project_id = ANY($4)))";
const WITHIN_APP_LIMIT_5: &str = "$5::uuid[] IS NULL OR (suggestion.project_id = ANY($5) AND (suggestion.other_project_id IS NULL OR suggestion.other_project_id = ANY($5)))";
const WITHIN_APP_LIMIT_11: &str = "$11::uuid[] IS NULL OR (suggestion.project_id = ANY($11) AND (suggestion.other_project_id IS NULL OR suggestion.other_project_id = ANY($11)))";

/// While an app acts for the reader: the projects its limit lets it see
/// tasks in. None otherwise.
fn app_limit_projects(reader: Reader) -> Option<Vec<Uuid>> {
    crate::policy::agent_limit_projects_allowing(
        reader.actor_id,
        crate::policy::Permission::TasksView,
    )
}

fn parse_cursor(cursor: &str) -> Option<(f32, Uuid)> {
    let (confidence, id) = cursor.split_once('_')?;
    Some((confidence.parse().ok()?, id.parse().ok()?))
}

/// One page of the suggestions `reader` can see in `project_id`, most
/// confident first.
pub async fn list(
    connection: &mut PgConnection,
    reader: Reader,
    project_id: Uuid,
    filter: &Filter,
) -> Result<Page, AppError> {
    let limit = filter.limit.clamp(1, MAX_PAGE);
    let status = filter
        .status
        .clone()
        .unwrap_or_else(|| "pending".to_owned());
    if !matches!(
        status.as_str(),
        "pending" | "accepted" | "dismissed" | "applied" | "all"
    ) {
        return Err(AppError::Validation("status"));
    }
    let cursor = match &filter.cursor {
        Some(cursor) => Some(parse_cursor(cursor).ok_or(AppError::Validation("cursor"))?),
        None => None,
    };
    let kinds = filter
        .kinds
        .iter()
        .map(|kind| kind.as_str().to_owned())
        .collect::<Vec<_>>();
    let pending_only = status == "pending";
    let rows = sqlx::query_as::<_, Row>(&format!(
        r#"
        SELECT suggestion.id, suggestion.project_id, suggestion.work_item_id, suggestion.kind,
               suggestion.other_work_item_id, suggestion.value, suggestion.confidence,
               suggestion.reason, suggestion.sources, suggestion.status, suggestion.origin,
               suggestion.audience_actor_id, suggestion.provider, suggestion.model,
               suggestion.created_at, suggestion.decided_at
        FROM ai_suggestions suggestion
        {JOINS}
        WHERE suggestion.project_id = $1
          AND ($2 = 'all' OR suggestion.status = $2)
          AND suggestion.status <> 'stale'
          AND ($3::uuid IS NULL OR suggestion.work_item_id = $3 OR suggestion.other_work_item_id = $3)
          AND (cardinality($4::text[]) = 0 OR suggestion.kind = ANY($4))
          AND ($5::real IS NULL OR (suggestion.confidence, suggestion.id) < ($5, $6))
          AND (NOT $9 OR ({STILL_USEFUL}))
          AND ai_suggestion_visible_to(suggestion.id, $7, $8)
          AND ({WITHIN_APP_LIMIT_11})
        ORDER BY suggestion.confidence DESC, suggestion.id DESC
        LIMIT $10
        "#
    ))
    .bind(project_id)
    .bind(&status)
    .bind(filter.work_item_id)
    .bind(&kinds)
    .bind(cursor.map(|(confidence, _)| confidence))
    .bind(cursor.map(|(_, id)| id))
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .bind(pending_only)
    .bind(limit + 1)
    .bind(app_limit_projects(reader))
    .fetch_all(&mut *connection)
    .await?;
    let has_more = rows.len() as i64 > limit;
    let rows = rows.into_iter().take(limit as usize).collect::<Vec<_>>();
    let next_cursor = has_more
        .then(|| {
            rows.last()
                .map(|row| format!("{}_{}", row.confidence, row.id))
        })
        .flatten();
    let pending_count = sqlx::query_scalar::<_, i64>(&format!(
        r#"
        SELECT count(*)
        FROM ai_suggestions suggestion
        {JOINS}
        WHERE suggestion.project_id = $1 AND suggestion.status = 'pending'
          AND ({STILL_USEFUL})
          AND ai_suggestion_visible_to(suggestion.id, $2, $3)
          AND ({WITHIN_APP_LIMIT_4})
        "#
    ))
    .bind(project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .bind(app_limit_projects(reader))
    .fetch_one(&mut *connection)
    .await?;
    let suggestions = views(connection, reader, rows).await?;
    Ok(Page {
        suggestions,
        next_cursor,
        pending_count,
    })
}

/// One suggestion, if `reader` may see it.
pub async fn get(
    connection: &mut PgConnection,
    reader: Reader,
    project_id: Uuid,
    suggestion_id: Uuid,
) -> Result<Option<SuggestionView>, AppError> {
    let rows = sqlx::query_as::<_, Row>(&format!(
        r#"
        SELECT suggestion.id, suggestion.project_id, suggestion.work_item_id, suggestion.kind,
               suggestion.other_work_item_id, suggestion.value, suggestion.confidence,
               suggestion.reason, suggestion.sources, suggestion.status, suggestion.origin,
               suggestion.audience_actor_id, suggestion.provider, suggestion.model,
               suggestion.created_at, suggestion.decided_at
        FROM ai_suggestions suggestion
        WHERE suggestion.id = $1 AND suggestion.project_id = $2
          AND ai_suggestion_visible_to(suggestion.id, $3, $4)
          AND ({WITHIN_APP_LIMIT_5})
        "#
    ))
    .bind(suggestion_id)
    .bind(project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .bind(app_limit_projects(reader))
    .fetch_all(&mut *connection)
    .await?;
    Ok(views(connection, reader, rows).await?.into_iter().next())
}

async fn task_refs(
    connection: &mut PgConnection,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, TaskRef>, AppError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let refs = sqlx::query_as::<_, TaskRef>(
        r#"
        SELECT item.id, item.project_id, project.project_key || '-' || item.number AS key,
               item.title, status.name AS status_name, status.semantic_category AS status_category,
               status.semantic_category IN ('completed', 'canceled') AS finished,
               item.archived_at IS NOT NULL AS archived
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.id = ANY($1)
        "#,
    )
    .bind(ids)
    .fetch_all(&mut *connection)
    .await?;
    Ok(refs
        .into_iter()
        .map(|mut task| {
            task.href = format!("/projects/{}/work-items/{}", task.project_id, task.id);
            (task.id, task)
        })
        .collect())
}

async fn views(
    connection: &mut PgConnection,
    reader: Reader,
    rows: Vec<Row>,
) -> Result<Vec<SuggestionView>, AppError> {
    let mut task_ids = Vec::new();
    let mut conversation_ids = Vec::new();
    let mut parsed_sources = Vec::with_capacity(rows.len());
    for row in &rows {
        task_ids.push(row.work_item_id);
        task_ids.extend(row.other_work_item_id);
        let sources =
            serde_json::from_value::<Vec<SourceRef>>(row.sources.clone()).unwrap_or_default();
        for source in &sources {
            match source {
                SourceRef::Task { id } => task_ids.push(*id),
                SourceRef::Message {
                    conversation_id, ..
                } => conversation_ids.push(*conversation_id),
                SourceRef::Signal { .. } => {}
            }
        }
        parsed_sources.push(sources);
    }
    task_ids.sort_unstable();
    task_ids.dedup();
    conversation_ids.sort_unstable();
    conversation_ids.dedup();
    let tasks = task_refs(connection, &task_ids).await?;
    // Every row already passed ai_suggestion_visible_to for this reader, so
    // these conversations are visible to them; the check is repeated anyway.
    let conversations = if conversation_ids.is_empty() {
        HashMap::new()
    } else {
        sqlx::query_as::<_, (Uuid, Uuid, String)>(
            r#"
            SELECT conversation.id, conversation.project_id, conversation.name
            FROM conversations conversation
            WHERE conversation.id = ANY($1)
              AND conversation_visible_to(conversation.id, $2)
              AND actor_can_access_project($2, $3, conversation.project_id)
            "#,
        )
        .bind(&conversation_ids)
        .bind(reader.actor_id)
        .bind(reader.connection_id)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|(id, project_id, name)| (id, (project_id, name)))
        .collect::<HashMap<_, _>>()
    };
    let mut views = Vec::with_capacity(rows.len());
    for (row, sources) in rows.into_iter().zip(parsed_sources) {
        let (Some(kind), Some(task)) = (Kind::parse(&row.kind), tasks.get(&row.work_item_id))
        else {
            continue;
        };
        let other = match row.other_work_item_id {
            Some(other) => match tasks.get(&other) {
                Some(task) => Some(task.clone()),
                None => continue,
            },
            None => None,
        };
        let sources = sources
            .into_iter()
            .filter_map(|source| match source {
                SourceRef::Task { id } => tasks
                    .get(&id)
                    .map(|task| SourceView::Task { task: task.clone() }),
                SourceRef::Message {
                    message_id,
                    conversation_id,
                } => conversations
                    .get(&conversation_id)
                    .map(|(project_id, name)| SourceView::Message {
                        message_id,
                        conversation_id,
                        conversation_name: name.clone(),
                        href: format!("/projects/{project_id}/conversations/{conversation_id}"),
                    }),
                SourceRef::Signal { name } => Some(SourceView::Signal { name }),
            })
            .collect();
        views.push(SuggestionView {
            id: row.id,
            kind,
            task: task.clone(),
            other,
            value: row.value,
            confidence: row.confidence,
            reason: row.reason,
            sources,
            status: row.status,
            origin: row.origin,
            personal: row.audience_actor_id.is_some(),
            provider: row.provider,
            model: row.model,
            created_at: row.created_at,
            decided_at: row.decided_at,
        });
        let _ = row.project_id;
    }
    Ok(views)
}

// ---------------------------------------------------------------------------
// Deciding

/// Why a suggestion couldn't be accepted or dismissed, for the person.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Refusal {
    NotFound,
    /// Already accepted, dismissed or applied.
    AlreadyDecided(String),
    /// The caller may not change tasks in this project.
    Forbidden,
    /// The change itself was refused (a cycle, an archived task); the
    /// message says why.
    Refused(String),
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Self::NotFound => "That suggestion wasn't found, or you can't see it.".to_owned(),
            Self::AlreadyDecided(status) => format!("That suggestion was already {status}."),
            Self::Forbidden => "You can't change tasks in this project.".to_owned(),
            Self::Refused(message) => message.clone(),
        }
    }
}

#[derive(FromRow)]
struct Locked {
    id: Uuid,
    work_item_id: Uuid,
    kind: String,
    other_project_id: Option<Uuid>,
    other_work_item_id: Option<Uuid>,
    value: Value,
    status: String,
    visible: bool,
}

/// Locks the suggestion and checks the caller may see it. The caller has
/// locked the project's membership first, as every task write does.
async fn lock(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    reader: Reader,
    suggestion_id: Uuid,
) -> Result<Result<Locked, Refusal>, AppError> {
    let Some(locked) = sqlx::query_as::<_, Locked>(&format!(
        r#"
        SELECT suggestion.id, suggestion.work_item_id, suggestion.kind,
               suggestion.other_project_id, suggestion.other_work_item_id, suggestion.value,
               suggestion.status,
               ai_suggestion_visible_to(suggestion.id, $3, $4) AND ({WITHIN_APP_LIMIT_5}) AS visible
        FROM ai_suggestions suggestion
        WHERE suggestion.id = $1 AND suggestion.project_id = $2
        FOR UPDATE
        "#
    ))
    .bind(suggestion_id)
    .bind(actor.project_id)
    .bind(reader.actor_id)
    .bind(reader.connection_id)
    .bind(app_limit_projects(reader))
    .fetch_optional(&mut **transaction)
    .await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    if !locked.visible {
        return Ok(Err(Refusal::NotFound));
    }
    if locked.status != "pending" {
        return Ok(Err(Refusal::AlreadyDecided(locked.status)));
    }
    Ok(Ok(locked))
}

async fn check_can_edit(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
) -> Result<Result<(), Refusal>, AppError> {
    let Some(membership) = lock_membership(transaction, actor.actor_id, actor.project_id).await?
    else {
        return Ok(Err(Refusal::NotFound));
    };
    if membership.project_archived_at.is_some() {
        return Ok(Err(Refusal::Refused(
            "This project is archived.".to_owned(),
        )));
    }
    if !authorize_mutation(actor, &membership, Action::WorkItemEdit, None)?.is_allowed() {
        return Ok(Err(Refusal::Forbidden));
    }
    Ok(Ok(()))
}

/// Accepts a suggestion as `actor`, applying it through the task commands.
/// `status` is `accepted` for a person's decision and `applied` for the
/// automatic switch. The caller commits.
pub async fn accept(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    reader: Reader,
    suggestion_id: Uuid,
    status: &'static str,
) -> Result<Result<(), Refusal>, AppError> {
    if let Err(refusal) = check_can_edit(transaction, actor).await? {
        return Ok(Err(refusal));
    }
    let locked = match lock(transaction, actor, reader, suggestion_id).await? {
        Ok(locked) => locked,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let kind = Kind::parse(&locked.kind).ok_or(AppError::Internal("stored suggestion kind"))?;
    if let Err(refusal) = apply(transaction, actor, &locked, kind).await? {
        return Ok(Err(refusal));
    }
    mark_via_ai(
        transaction,
        actor,
        &[Some(locked.work_item_id), locked.other_work_item_id],
        locked.id,
    )
    .await?;
    sqlx::query(
        r#"
        UPDATE ai_suggestions
        SET status = $2, decided_by_actor_id = $3,
            decided_at = CASE WHEN $2 = 'accepted' THEN now() ELSE NULL END,
            updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(locked.id)
    .bind(status)
    .bind(actor.actor_id)
    .execute(&mut **transaction)
    .await?;
    crate::work_item_commands::record_audit_for(
        transaction,
        actor,
        "ai_suggestion",
        if status == "accepted" {
            "ai_suggestion.accepted"
        } else {
            "ai_suggestion.applied"
        },
        locked.id,
        json!({"kind": kind.as_str(), "workItemId": locked.work_item_id}),
    )
    .await?;
    Ok(Ok(()))
}

/// Dismisses a suggestion for good: the same pair is never suggested again.
pub async fn dismiss(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    reader: Reader,
    suggestion_id: Uuid,
) -> Result<Result<(), Refusal>, AppError> {
    if let Err(refusal) = check_can_edit(transaction, actor).await? {
        return Ok(Err(refusal));
    }
    let locked = match lock(transaction, actor, reader, suggestion_id).await? {
        Ok(locked) => locked,
        Err(refusal) => return Ok(Err(refusal)),
    };
    sqlx::query(
        r#"
        UPDATE ai_suggestions
        SET status = 'dismissed', decided_by_actor_id = $2, decided_at = now(), updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(locked.id)
    .bind(actor.actor_id)
    .execute(&mut **transaction)
    .await?;
    Ok(Ok(()))
}

/// Marks the history entries this transaction wrote for these tasks as made
/// through an AI suggestion. `now()` is the transaction's start time, so it
/// picks out exactly this change's entries.
async fn mark_via_ai(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    tasks: &[Option<Uuid>],
    suggestion_id: Uuid,
) -> Result<(), AppError> {
    let ids = tasks.iter().flatten().copied().collect::<Vec<_>>();
    sqlx::query(
        r#"
        UPDATE work_item_activity
        SET summary = left(summary, 480) || ' (via AI)',
            payload = payload || jsonb_build_object('via', 'ai', 'suggestionId', $3::text)
        WHERE work_item_id = ANY($1) AND actor_id = $2 AND created_at = now()
          AND NOT (payload ? 'via')
        "#,
    )
    .bind(&ids)
    .bind(actor.actor_id)
    .bind(suggestion_id.to_string())
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

fn refused(message: impl Into<String>) -> Result<Result<(), Refusal>, AppError> {
    Ok(Err(Refusal::Refused(message.into())))
}

async fn apply(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    locked: &Locked,
    kind: Kind,
) -> Result<Result<(), Refusal>, AppError> {
    let task_id = locked.work_item_id;
    match kind {
        Kind::Blocks => {
            let Some(other) = locked.other_work_item_id else {
                return Err(AppError::Internal("link suggestion without a task"));
            };
            match planning::change_dependencies(
                transaction,
                actor,
                task_id,
                &[(other, Direction::BlockedBy)],
                false,
            )
            .await?
            {
                Ok(_) => Ok(Ok(())),
                Err((_, refusal)) => refused(refusal.message("the other task")),
            }
        }
        Kind::RelatesTo | Kind::Duplicates => {
            let Some(other) = locked.other_work_item_id else {
                return Err(AppError::Internal("link suggestion without a task"));
            };
            let relation = if kind == Kind::RelatesTo {
                RelationKind::RelatesTo
            } else {
                RelationKind::Duplicates
            };
            match planning::change_relation(
                transaction,
                actor,
                task_id,
                other,
                relation,
                false,
                false,
            )
            .await?
            {
                Ok(_) => Ok(Ok(())),
                Err(refusal) => refused(refusal.message("the other task")),
            }
        }
        Kind::Parent => {
            let (Some(other), Some(other_project)) =
                (locked.other_work_item_id, locked.other_project_id)
            else {
                return Err(AppError::Internal("link suggestion without a task"));
            };
            if other_project != actor.project_id {
                return refused("A subtask must be in the same project as its parent.");
            }
            update_task(
                transaction,
                actor,
                task_id,
                TaskUpdate {
                    parent: Some(Some(other)),
                    ..TaskUpdate::default()
                },
            )
            .await
        }
        Kind::BlockedReason => {
            let reason = locked.value["reason"]
                .as_str()
                .map(clean_reason)
                .unwrap_or_default();
            update_task(
                transaction,
                actor,
                task_id,
                TaskUpdate {
                    fields: TaskFieldChanges {
                        blocked_reason: Some(Some(reason)),
                        ..TaskFieldChanges::default()
                    },
                    ..TaskUpdate::default()
                },
            )
            .await
        }
        Kind::Priority => {
            let Some(priority) = locked.value["priority"].as_str() else {
                return refused("The suggested priority is missing.");
            };
            if crate::work_item_commands::validate_priority(priority).is_err() {
                return refused("The suggested priority isn't one this project uses.");
            }
            update_task(
                transaction,
                actor,
                task_id,
                TaskUpdate {
                    priority: Some(priority.to_owned()),
                    ..TaskUpdate::default()
                },
            )
            .await
        }
        Kind::Type => {
            let type_id = locked.value["typeId"]
                .as_str()
                .and_then(|id| id.parse::<Uuid>().ok());
            let active = match type_id {
                Some(type_id) => sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM work_item_types WHERE project_id = $1 AND id = $2 AND enabled)",
                )
                .bind(actor.project_id)
                .bind(type_id)
                .fetch_one(&mut **transaction)
                .await?,
                None => false,
            };
            if !active {
                return refused("That task type no longer exists in this project.");
            }
            update_task(
                transaction,
                actor,
                task_id,
                TaskUpdate {
                    type_id,
                    ..TaskUpdate::default()
                },
            )
            .await
        }
        Kind::Label => {
            let label_id = locked.value["labelId"]
                .as_str()
                .and_then(|id| id.parse::<Uuid>().ok());
            let active = match label_id {
                Some(label_id) => sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS (SELECT 1 FROM project_labels WHERE project_id = $1 AND id = $2 AND archived_at IS NULL)",
                )
                .bind(actor.project_id)
                .bind(label_id)
                .fetch_one(&mut **transaction)
                .await?,
                None => false,
            };
            let (true, Some(label_id)) = (active, label_id) else {
                return refused("That label no longer exists in this project.");
            };
            update_task(
                transaction,
                actor,
                task_id,
                TaskUpdate {
                    add_label: Some(label_id),
                    ..TaskUpdate::default()
                },
            )
            .await
        }
        Kind::Criteria => {
            let criteria = locked.value["criteria"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter_map(|text| normalize_criterion_text(text).ok())
                .collect::<Vec<_>>();
            if criteria.is_empty() {
                return refused("The suggestion has no criteria to add.");
            }
            let Some(current) = lock_work_item(transaction, actor.project_id, task_id).await?
            else {
                return Ok(Err(Refusal::NotFound));
            };
            let operations = criteria
                .into_iter()
                .map(|text| CriteriaOperation::Add {
                    text,
                    position: None,
                })
                .collect::<Vec<_>>();
            let outcome = crate::work_item_commands::change_criteria(
                transaction,
                actor,
                task_id,
                CriteriaCommand {
                    expected_version: current.version,
                    operations: &operations,
                    audit_action: "work_item.ai_criteria_added",
                },
            )
            .await?;
            match outcome {
                CriteriaOutcome::Applied { .. } | CriteriaOutcome::Unchanged { .. } => Ok(Ok(())),
                CriteriaOutcome::NotFound => Ok(Err(Refusal::NotFound)),
                CriteriaOutcome::Conflict(_) => refused("The task is archived."),
                CriteriaOutcome::Plan { .. } => {
                    refused("The task has as many criteria as it can hold.")
                }
            }
        }
    }
}

#[derive(Default)]
struct TaskUpdate {
    parent: Option<Option<Uuid>>,
    priority: Option<String>,
    type_id: Option<Uuid>,
    add_label: Option<Uuid>,
    fields: TaskFieldChanges,
}

async fn update_task(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    task_id: Uuid,
    update: TaskUpdate,
) -> Result<Result<(), Refusal>, AppError> {
    let Some(current) = lock_work_item(transaction, actor.project_id, task_id).await? else {
        return Ok(Err(Refusal::NotFound));
    };
    let expected_version = current.version;
    let add = update.add_label.into_iter().collect::<Vec<_>>();
    let outcome = update_locked_work_item(
        transaction,
        actor,
        current,
        UpdateWorkItemCommand {
            expected_version,
            title: None,
            description: None,
            type_id: update.type_id,
            status_id: None,
            priority: update.priority.as_deref(),
            assignee_actor_id: None,
            parent_work_item_id: update.parent,
            board_placement: None,
            labels: (!add.is_empty()).then_some(LabelDelta {
                add: &add,
                remove: &[],
            }),
            fields: update.fields,
            summary: "Changed through an AI suggestion",
            audit_action: "work_item.ai_suggestion_applied",
            audit_version_metadata: AuditVersionMetadata::Include,
        },
    )
    .await?;
    match outcome {
        UpdateWorkItemOutcome::Applied { .. } | UpdateWorkItemOutcome::Noop(_) => Ok(Ok(())),
        UpdateWorkItemOutcome::NotFound => Ok(Err(Refusal::NotFound)),
        UpdateWorkItemOutcome::Conflict(_) => refused("The task is archived."),
        UpdateWorkItemOutcome::TooManyLabels { limit, .. } => {
            refused(format!("A task can have at most {limit} labels."))
        }
        UpdateWorkItemOutcome::InvalidParent(_) => {
            refused("That parent would make a loop or nest too deep.")
        }
        _ => refused("The task couldn't be changed that way."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pair_has_one_key_whichever_way_round_and_whatever_the_kind() {
        let (a, b) = (Uuid::now_v7(), Uuid::now_v7());
        let key = pair_key(Kind::Blocks, a, Some(b), &Value::Null);
        assert_eq!(key, pair_key(Kind::Blocks, b, Some(a), &Value::Null));
        assert_eq!(key, pair_key(Kind::Duplicates, b, Some(a), &Value::Null));
        let high = json!({"priority": "high"});
        assert_ne!(
            pair_key(Kind::Priority, a, None, &high),
            pair_key(Kind::Priority, a, None, &json!({"priority": "low"}))
        );
    }

    #[test]
    fn reasons_are_one_short_line() {
        assert_eq!(clean_reason("  two\n lines\t here "), "two lines here");
        assert_eq!(clean_reason(""), "Suggested by AI.");
        assert_eq!(clean_reason(&"x".repeat(400)).chars().count(), 300);
    }

    #[test]
    fn sources_round_trip_in_the_stored_shape() {
        let message = SourceRef::Message {
            message_id: Uuid::nil(),
            conversation_id: Uuid::nil(),
        };
        let stored = serde_json::to_value(&message).expect("json");
        assert_eq!(stored["kind"], "message");
        assert!(stored["messageId"].is_string());
        assert_eq!(
            serde_json::from_value::<SourceRef>(stored).expect("back"),
            message
        );
        assert_eq!(
            serde_json::to_value(SourceRef::Task { id: Uuid::nil() }).expect("json")["kind"],
            "task"
        );
    }
}
