//! Task templates and recurring tasks (docs/plans/roadmap.md, W11, T10).
//!
//! A template is what a new task starts with: title, description, type,
//! priority, labels, acceptance criteria and subtasks. Applying one creates
//! the task and its subtasks through `work_item_commands::create_work_item`,
//! so each gets its activity, audit row and event like any other task.
//!
//! A recurring task applies a template on a schedule. The schedule is kept
//! in the person's local time and zone; the database turns local times into
//! instants, so daylight saving moves with the zone. A system job looks for
//! due schedules every minute and queues one job per occurrence, which acts
//! as the person who last set the schedule up and checks they still may
//! create tasks there before it does (principle 9). Delivery is at least
//! once, so each occurrence is recorded with the task it made and a repeat
//! makes nothing.

use std::time::Duration as StdDuration;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use chrono::{DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::{
    authorize_work_item, capabilities,
    planning::{Started, begin, invalid, refusal, start_write},
    project_membership,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiJson, ApiPath, AppError},
    idempotency::resource_bound_scope,
    jobs::{JobContext, JobError, JobRegistry, NewJob, enqueue},
    policy::{Action, ActorKind, Decision},
    work_item_commands::{
        AllocatedNumberMetadata, AuditAttribution, CommandActor, CreateWorkItemCommand,
        CreateWorkItemOutcome, NewTaskFields, authorize_mutation, create_work_item,
        lock_membership, normalize_criterion_text, normalize_description, normalize_title,
        record_audit_for, valid_estimate, validate_priority,
    },
    work_item_labels::{self, MAX_LABELS_PER_TASK},
};

const MAX_TEMPLATES: i64 = 200;
const MAX_CRITERIA: usize = 50;
const MAX_SUBTASKS: usize = 50;
const MAX_NAME_CHARACTERS: usize = 80;
const MAX_TEMPLATE_DESCRIPTION: usize = 20_000;
const MAX_RECURRING: i64 = 100;

pub(crate) const SWEEP_KIND: &str = "tasks.recurring_sweep";
pub(crate) const OCCURRENCE_KIND: &str = "tasks.recurring_create";

// ---------------------------------------------------------------------------
// Task templates

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SubtaskDraft {
    pub title: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, FromRow)]
pub(crate) struct TemplateRow {
    pub id: Uuid,
    pub name: String,
    pub title: String,
    pub description: String,
    pub type_id: Option<Uuid>,
    pub type_name: Option<String>,
    pub priority: String,
    pub labels: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub subtasks: Value,
    pub estimate: Option<f64>,
    pub archived: bool,
    pub version: i64,
}

const TEMPLATE_SELECT: &str = r#"
    SELECT template.id, template.name, template.title,
           template.description, template.type_id, item_type.name AS type_name,
           template.priority, template.labels, template.acceptance_criteria,
           template.subtasks, template.estimate,
           template.archived_at IS NOT NULL AS archived, template.version
    FROM task_templates template
    LEFT JOIN work_item_types item_type
      ON item_type.project_id = template.project_id AND item_type.id = template.type_id
"#;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskTemplateView {
    pub id: Uuid,
    pub name: String,
    pub title: String,
    pub description: String,
    pub type_id: Option<Uuid>,
    pub type_name: Option<String>,
    pub priority: String,
    pub labels: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub subtasks: Vec<SubtaskDraft>,
    pub estimate: Option<f64>,
    pub archived: bool,
    pub version: i64,
}

impl From<TemplateRow> for TaskTemplateView {
    fn from(row: TemplateRow) -> Self {
        Self {
            subtasks: serde_json::from_value(row.subtasks).unwrap_or_default(),
            id: row.id,
            name: row.name,
            title: row.title,
            description: row.description,
            type_id: row.type_id,
            type_name: row.type_name,
            priority: row.priority,
            labels: row.labels,
            acceptance_criteria: row.acceptance_criteria,
            estimate: row.estimate,
            archived: row.archived,
            version: row.version,
        }
    }
}

pub(crate) async fn list_templates(
    connection: &mut PgConnection,
    project_id: Uuid,
    include_archived: bool,
) -> Result<Vec<TemplateRow>, AppError> {
    Ok(sqlx::query_as::<_, TemplateRow>(&format!(
        r#"{TEMPLATE_SELECT}
        WHERE template.project_id = $1 AND ($2 OR template.archived_at IS NULL)
        ORDER BY template.archived_at IS NOT NULL, lower(template.name), template.id
        LIMIT 500"#
    ))
    .bind(project_id)
    .bind(include_archived)
    .fetch_all(&mut *connection)
    .await?)
}

pub(crate) async fn template_for_update(
    connection: &mut PgConnection,
    project_id: Uuid,
    template_id: Uuid,
) -> Result<Option<TemplateRow>, AppError> {
    Ok(sqlx::query_as::<_, TemplateRow>(&format!(
        "{TEMPLATE_SELECT} WHERE template.project_id = $1 AND template.id = $2 FOR UPDATE OF template"
    ))
    .bind(project_id)
    .bind(template_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// What a template holds, checked and normalized.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TemplateFields {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub type_id: Option<Uuid>,
    #[serde(default = "no_priority")]
    pub priority: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub subtasks: Vec<SubtaskDraft>,
    pub estimate: Option<f64>,
}

fn no_priority() -> String {
    "no_priority".to_owned()
}

pub(crate) fn normalize_template_name(value: &str) -> Option<String> {
    let name = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty() && name.chars().count() <= MAX_NAME_CHARACTERS).then_some(name)
}

/// Checks and tidies a template's fields. The error names the field and
/// says what to change.
pub(crate) fn normalize_template(
    mut fields: TemplateFields,
) -> Result<TemplateFields, (&'static str, String)> {
    fields.name = normalize_template_name(&fields.name).ok_or((
        "name",
        "Give the template a name of 1 to 80 characters.".to_owned(),
    ))?;
    fields.title = normalize_title(&fields.title).map_err(|_| {
        (
            "title",
            "Give tasks a title of 1 to 240 characters.".to_owned(),
        )
    })?;
    fields.description = normalize_description(&fields.description)
        .ok()
        .filter(|text| text.chars().count() <= MAX_TEMPLATE_DESCRIPTION)
        .ok_or((
            "description",
            "Keep the description under 20,000 characters.".to_owned(),
        ))?;
    validate_priority(&fields.priority).map_err(|_| {
        (
            "priority",
            "Use no_priority, low, medium, high or urgent.".to_owned(),
        )
    })?;
    let mut labels = Vec::new();
    for label in &fields.labels {
        let name = work_item_labels::normalize_name(label)
            .ok_or(("labels", format!("“{}” isn't a label name.", label.trim())))?;
        if !labels
            .iter()
            .any(|kept: &String| kept.eq_ignore_ascii_case(&name))
        {
            labels.push(name);
        }
    }
    if labels.len() > MAX_LABELS_PER_TASK {
        return Err((
            "labels",
            format!("Use at most {MAX_LABELS_PER_TASK} labels."),
        ));
    }
    fields.labels = labels;
    if fields.acceptance_criteria.len() > MAX_CRITERIA {
        return Err((
            "acceptanceCriteria",
            format!("Use at most {MAX_CRITERIA} acceptance criteria."),
        ));
    }
    fields.acceptance_criteria = fields
        .acceptance_criteria
        .iter()
        .filter(|text| !text.trim().is_empty())
        .map(|text| normalize_criterion_text(text))
        .collect::<Result<_, _>>()
        .map_err(|_| {
            (
                "acceptanceCriteria",
                "Keep each criterion to one line of at most 1,000 characters.".to_owned(),
            )
        })?;
    if fields.subtasks.len() > MAX_SUBTASKS {
        return Err(("subtasks", format!("Use at most {MAX_SUBTASKS} subtasks.")));
    }
    let mut subtasks = Vec::new();
    for subtask in fields.subtasks {
        if subtask.title.trim().is_empty() && subtask.description.trim().is_empty() {
            continue;
        }
        subtasks.push(SubtaskDraft {
            title: normalize_title(&subtask.title).map_err(|_| {
                (
                    "subtasks",
                    "Give each subtask a title of 1 to 240 characters.".to_owned(),
                )
            })?,
            description: normalize_description(&subtask.description)
                .ok()
                .filter(|text| text.chars().count() <= 5_000)
                .ok_or((
                    "subtasks",
                    "Keep each subtask's description under 5,000 characters.".to_owned(),
                ))?,
        });
    }
    fields.subtasks = subtasks;
    if fields
        .estimate
        .is_some_and(|estimate| !valid_estimate(estimate))
    {
        return Err((
            "estimate",
            "Use an estimate from 0 to 100,000, in steps of 0.25 at most.".to_owned(),
        ));
    }
    Ok(fields)
}

async fn type_is_enabled(
    connection: &mut PgConnection,
    project_id: Uuid,
    type_id: Option<Uuid>,
) -> Result<bool, AppError> {
    let Some(type_id) = type_id else {
        return Ok(true);
    };
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM work_item_types WHERE project_id = $1 AND id = $2 AND enabled)",
    )
    .bind(project_id)
    .bind(type_id)
    .fetch_one(&mut *connection)
    .await?)
}

/// Inserts a template whose fields are already normalized and whose type
/// belongs to the project. Shared with project templates.
pub(crate) async fn insert_template(
    connection: &mut PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    fields: &TemplateFields,
) -> Result<Uuid, AppError> {
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO task_templates (
            id, project_id, name, title, description, type_id, priority, labels,
            acceptance_criteria, subtasks, estimate, created_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(&fields.name)
    .bind(&fields.title)
    .bind(&fields.description)
    .bind(fields.type_id)
    .bind(&fields.priority)
    .bind(&fields.labels)
    .bind(&fields.acceptance_criteria)
    .bind(json!(fields.subtasks))
    .bind(fields.estimate)
    .bind(actor_id)
    .execute(&mut *connection)
    .await?;
    Ok(id)
}

async fn active_name_taken(
    connection: &mut PgConnection,
    project_id: Uuid,
    name: &str,
    except: Option<Uuid>,
) -> Result<bool, AppError> {
    Ok(sqlx::query_scalar::<_, bool>(
        r#"SELECT EXISTS (SELECT 1 FROM task_templates
           WHERE project_id = $1 AND archived_at IS NULL AND lower(name) = lower($2)
             AND id IS DISTINCT FROM $3)"#,
    )
    .bind(project_id)
    .bind(name)
    .bind(except)
    .fetch_one(&mut *connection)
    .await?)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TemplatesResponse {
    templates: Vec<TaskTemplateView>,
    recurring: Vec<RecurringView>,
    can_edit: bool,
}

pub(super) async fn get_templates(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<TemplatesResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let mut connection = state.pool.acquire().await?;
    let templates = list_templates(&mut connection, project_id, true).await?;
    let recurring = list_recurring(&mut connection, project_id).await?;
    Ok(Json(TemplatesResponse {
        templates: templates.into_iter().map(Into::into).collect(),
        recurring,
        can_edit: capabilities(&membership)?.edit && membership.project_archived_at.is_none(),
    }))
}

/// What a template write came to, when it wasn't an error: the web and
/// template_write each make their own answer from it.
pub(crate) enum TemplateOutcome {
    Done(Box<TaskTemplateView>),
    Invalid(&'static str, &'static str),
    TooMany,
    VersionConflict(Box<TaskTemplateView>),
}

const TYPE_MESSAGE: &str = "Pick one of the project's task types.";
const NAME_TAKEN: &str = "Another template already has that name.";

/// Saves a new template from normalized fields, as `actor_id`.
pub(crate) async fn create_task_template(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    project_id: Uuid,
    actor_id: Uuid,
    fields: &TemplateFields,
) -> Result<TemplateOutcome, AppError> {
    if !type_is_enabled(transaction, project_id, fields.type_id).await? {
        return Ok(TemplateOutcome::Invalid("typeId", TYPE_MESSAGE));
    }
    if active_name_taken(transaction, project_id, &fields.name, None).await? {
        return Ok(TemplateOutcome::Invalid("name", NAME_TAKEN));
    }
    let count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM task_templates WHERE project_id = $1 AND archived_at IS NULL",
    )
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?;
    if count >= MAX_TEMPLATES {
        return Ok(TemplateOutcome::TooMany);
    }
    let id = insert_template(transaction, project_id, actor_id, fields).await?;
    record_audit_for(
        transaction,
        actor,
        "task_template",
        "task_template.created",
        id,
        json!({}),
    )
    .await?;
    let row = template_for_update(transaction, project_id, id)
        .await?
        .ok_or(AppError::Internal("template just made is missing"))?;
    Ok(TemplateOutcome::Done(Box::new(row.into())))
}

/// Changes a template: its fields replaced by normalized `fields` when
/// given, and archived or restored. Archiving pauses the schedules that
/// make tasks from it.
pub(crate) async fn change_task_template(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    project_id: Uuid,
    template_id: Uuid,
    expected_version: Option<i64>,
    fields: Option<&TemplateFields>,
    archived: Option<bool>,
) -> Result<TemplateOutcome, AppError> {
    let row = template_for_update(transaction, project_id, template_id)
        .await?
        .ok_or(AppError::NotFound("task_template"))?;
    if expected_version.is_some_and(|expected| expected != row.version) {
        return Ok(TemplateOutcome::VersionConflict(Box::new(row.into())));
    }
    let archived = archived.unwrap_or(row.archived);
    let name = fields.map_or(row.name.as_str(), |fields| &fields.name);
    if !archived && active_name_taken(transaction, project_id, name, Some(template_id)).await? {
        return Ok(TemplateOutcome::Invalid("name", NAME_TAKEN));
    }
    if let Some(fields) = fields {
        if !type_is_enabled(transaction, project_id, fields.type_id).await? {
            return Ok(TemplateOutcome::Invalid("typeId", TYPE_MESSAGE));
        }
        sqlx::query(
            r#"
            UPDATE task_templates
            SET name = $3, title = $4, description = $5, type_id = $6, priority = $7,
                labels = $8, acceptance_criteria = $9, subtasks = $10, estimate = $11
            WHERE project_id = $1 AND id = $2
            "#,
        )
        .bind(project_id)
        .bind(template_id)
        .bind(&fields.name)
        .bind(&fields.title)
        .bind(&fields.description)
        .bind(fields.type_id)
        .bind(&fields.priority)
        .bind(&fields.labels)
        .bind(&fields.acceptance_criteria)
        .bind(json!(fields.subtasks))
        .bind(fields.estimate)
        .execute(&mut **transaction)
        .await?;
    }
    sqlx::query(
        r#"
        UPDATE task_templates
        SET archived_at = CASE WHEN $3 THEN COALESCE(archived_at, now()) ELSE NULL END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(project_id)
    .bind(template_id)
    .bind(archived)
    .execute(&mut **transaction)
    .await?;
    if archived && !row.archived {
        // A schedule with nothing to make stops, and says why.
        sqlx::query(
            r#"
            UPDATE recurring_tasks
            SET enabled = false, next_run_at = NULL, version = version + 1, updated_at = now(),
                paused_reason = 'Paused because its template was archived.'
            WHERE project_id = $1 AND template_id = $2 AND enabled
            "#,
        )
        .bind(project_id)
        .bind(template_id)
        .execute(&mut **transaction)
        .await?;
    }
    record_audit_for(
        transaction,
        actor,
        "task_template",
        "task_template.updated",
        template_id,
        json!({ "archived": archived }),
    )
    .await?;
    let row = template_for_update(transaction, project_id, template_id)
        .await?
        .ok_or(AppError::Internal("template just changed is missing"))?;
    Ok(TemplateOutcome::Done(Box::new(row.into())))
}

/// The web's answer to a template write that didn't go through.
fn template_refusal(outcome: TemplateOutcome) -> Result<Response, AppError> {
    Ok(match outcome {
        TemplateOutcome::Done(_) => {
            return Err(AppError::Internal("a template write wasn't refused"));
        }
        TemplateOutcome::Invalid(field, message) => invalid(field, message),
        TemplateOutcome::TooMany => refusal(
            StatusCode::CONFLICT,
            "too_many_templates",
            "This project has 200 templates. Archive one you no longer use first.".to_owned(),
            json!({}),
        ),
        TemplateOutcome::VersionConflict(current) => refusal(
            StatusCode::CONFLICT,
            "version_conflict",
            "Someone changed this template first. You’re now looking at the latest version."
                .to_owned(),
            json!({ "resource": "task_template", "currentVersion": current.version, "current": current }),
        ),
    })
}

pub(super) async fn create_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<TemplateFields>,
) -> Result<Response, AppError> {
    let fields = match normalize_template(request.clone()) {
        Ok(fields) => fields,
        Err((field, message)) => return Ok(invalid(field, message)),
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            None,
            resource_bound_scope("projects", project_id, "task_templates_create")?,
            &request,
        )
        .await
    );
    let actor = write.actor.clone();
    match create_task_template(
        &mut write.transaction,
        &actor,
        project_id,
        principal.actor_id,
        &fields,
    )
    .await?
    {
        TemplateOutcome::Done(view) => write.finish(StatusCode::CREATED, &view).await,
        refused => template_refusal(refused),
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateTemplateRequest {
    expected_version: i64,
    #[serde(flatten)]
    fields: Option<TemplateFields>,
    archived: Option<bool>,
}

pub(super) async fn update_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, template_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateTemplateRequest>,
) -> Result<Response, AppError> {
    let fields = match request.fields.clone().map(normalize_template).transpose() {
        Ok(fields) => fields,
        Err((field, message)) => return Ok(invalid(field, message)),
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemEdit,
            None,
            resource_bound_scope("task_templates", template_id, "update")?,
            &request,
        )
        .await
    );
    let actor = write.actor.clone();
    match change_task_template(
        &mut write.transaction,
        &actor,
        project_id,
        template_id,
        Some(request.expected_version),
        fields.as_ref(),
        request.archived,
    )
    .await?
    {
        TemplateOutcome::Done(view) => write.finish(StatusCode::OK, &view).await,
        refused => template_refusal(refused),
    }
}

/// What applying a template may change from it.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ApplyOptions {
    pub title: Option<String>,
    pub assignee_actor_id: Option<Uuid>,
    pub milestone_id: Option<Uuid>,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Applied {
    pub work_item_id: Uuid,
    pub key: String,
    pub subtask_count: usize,
    /// Labels the template named that the project had to make.
    pub created_labels: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) enum ApplyOutcome {
    Created(Applied),
    /// Nothing was made: the field and what to do about it.
    Refused(&'static str, String),
}

/// Creates a task, and its subtasks, from a template. The caller has locked
/// the project and checked the actor may create tasks.
pub(crate) async fn apply_template(
    transaction: &mut Transaction<'_, Postgres>,
    actor: &CommandActor,
    template_id: Uuid,
    options: &ApplyOptions,
) -> Result<ApplyOutcome, AppError> {
    let project_id = actor.project_id;
    let Some(template) = template_for_update(transaction, project_id, template_id).await? else {
        return Ok(ApplyOutcome::Refused(
            "templateId",
            "That template isn't in this project.".to_owned(),
        ));
    };
    if template.archived {
        return Ok(ApplyOutcome::Refused(
            "templateId",
            "That template is archived. Restore it first.".to_owned(),
        ));
    }
    let title = match options
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
    {
        Some(title) => match normalize_title(title) {
            Ok(title) => title,
            Err(_) => {
                return Ok(ApplyOutcome::Refused(
                    "title",
                    "Use a title of 1 to 240 characters.".to_owned(),
                ));
            }
        },
        None => template.title.clone(),
    };
    let wanted = template
        .labels
        .iter()
        .map(|name| (name.clone(), String::new()))
        .collect::<Vec<_>>();
    let labels =
        work_item_labels::ensure_by_name(transaction, project_id, actor.actor_id, &wanted).await?;
    // A type retired since the template was saved falls back to the default.
    let type_id = if type_is_enabled(transaction, project_id, template.type_id).await? {
        template.type_id
    } else {
        None
    };
    let outcome = create_work_item(
        transaction,
        actor,
        CreateWorkItemCommand {
            field_values: None,
            title: &title,
            description: &template.description,
            type_id,
            status_id: None,
            priority: &template.priority,
            assignee_actor_id: options.assignee_actor_id,
            parent_work_item_id: None,
            label_ids: &labels.ids,
            acceptance_criteria: &template.acceptance_criteria,
            board_placement: None,
            fields: NewTaskFields {
                milestone_id: options.milestone_id,
                start_date: options.start_date,
                due_date: options.due_date,
                estimate: template.estimate,
            },
            activity_summary: "Work Item created",
            activity_metadata: json!({}),
            activity_number_metadata: AllocatedNumberMetadata::Omit,
            audit_action: "work_item.created",
            audit_metadata: json!({ "taskTemplateId": template.id }),
        },
    )
    .await?;
    let (work_item_id, number) = match outcome {
        CreateWorkItemOutcome::Created {
            work_item_id,
            number,
            ..
        } => (work_item_id, number),
        CreateWorkItemOutcome::InvalidAssignee => {
            return Ok(ApplyOutcome::Refused(
                "assigneeActorId",
                "That person or agent can't be assigned tasks in this project.".to_owned(),
            ));
        }
        CreateWorkItemOutcome::InvalidMilestone => {
            return Ok(ApplyOutcome::Refused(
                "milestoneId",
                "Pick one of the project's active milestones.".to_owned(),
            ));
        }
        CreateWorkItemOutcome::InvalidDates => {
            return Ok(ApplyOutcome::Refused(
                "dueDate",
                "The start date has to come before the due date.".to_owned(),
            ));
        }
        CreateWorkItemOutcome::ParentNotFound
        | CreateWorkItemOutcome::ParentArchived(_)
        | CreateWorkItemOutcome::ParentTooDeep
        | CreateWorkItemOutcome::InvalidPlacement(_) => {
            return Err(AppError::Internal(
                "template task had a parent or placement",
            ));
        }
    };
    let subtasks: Vec<SubtaskDraft> =
        serde_json::from_value(template.subtasks.clone()).unwrap_or_default();
    for subtask in &subtasks {
        let outcome = create_work_item(
            transaction,
            actor,
            CreateWorkItemCommand {
                field_values: None,
                title: &subtask.title,
                description: &subtask.description,
                type_id,
                status_id: None,
                priority: "no_priority",
                assignee_actor_id: options.assignee_actor_id,
                parent_work_item_id: Some(work_item_id),
                label_ids: &[],
                acceptance_criteria: &[],
                board_placement: None,
                fields: NewTaskFields {
                    milestone_id: options.milestone_id,
                    start_date: None,
                    due_date: None,
                    estimate: None,
                },
                activity_summary: "Work Item created",
                activity_metadata: json!({}),
                activity_number_metadata: AllocatedNumberMetadata::Omit,
                audit_action: "work_item.created",
                audit_metadata: json!({ "taskTemplateId": template.id }),
            },
        )
        .await?;
        if !matches!(outcome, CreateWorkItemOutcome::Created { .. }) {
            return Err(AppError::Internal("template subtask was refused"));
        }
    }
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut **transaction)
            .await?;
    Ok(ApplyOutcome::Created(Applied {
        work_item_id,
        key: format!("{project_key}-{number}"),
        subtask_count: subtasks.len(),
        created_labels: labels.created.into_iter().map(|label| label.name).collect(),
    }))
}

pub(super) async fn apply_template_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, template_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<ApplyOptions>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemCreate,
            None,
            resource_bound_scope("task_templates", template_id, "apply")?,
            &request,
        )
        .await
    );
    let actor = write.actor.clone();
    match apply_template(&mut write.transaction, &actor, template_id, &request).await? {
        ApplyOutcome::Created(applied) => write.finish(StatusCode::CREATED, &applied).await,
        ApplyOutcome::Refused(field, message) => Ok(invalid(field, message)),
    }
}

// ---------------------------------------------------------------------------
// Schedules

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Frequency {
    Daily,
    Weekly,
    Monthly,
}

impl Frequency {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "daily" => Some(Self::Daily),
            "weekly" => Some(Self::Weekly),
            "monthly" => Some(Self::Monthly),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Weekly => "weekly",
            Self::Monthly => "monthly",
        }
    }
}

/// When a recurring task runs, in local time.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Schedule {
    pub frequency: Frequency,
    /// Every this many days, weeks or months.
    pub every: u32,
    /// Weekly: ISO weekdays, 1 (Monday) to 7 (Sunday).
    pub weekdays: Vec<u32>,
    /// Monthly: the day of the month; a shorter month uses its last day.
    pub month_day: Option<u32>,
    pub time: NaiveTime,
    /// The first day it may run.
    pub starts_on: NaiveDate,
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first| first.pred_opt())
        .map_or(28, |last| last.day())
}

impl Schedule {
    /// Whether the schedule runs on `date`.
    pub fn occurs_on(&self, date: NaiveDate) -> bool {
        if date < self.starts_on {
            return false;
        }
        let every = i64::from(self.every.max(1));
        match self.frequency {
            Frequency::Daily => (date - self.starts_on).num_days() % every == 0,
            Frequency::Weekly => {
                let monday = |day: NaiveDate| {
                    day - Duration::days(i64::from(day.weekday().num_days_from_monday()))
                };
                let weeks = (monday(date) - monday(self.starts_on)).num_days() / 7;
                weeks % every == 0 && self.weekdays.contains(&date.weekday().number_from_monday())
            }
            Frequency::Monthly => {
                let months = (i64::from(date.year()) * 12 + i64::from(date.month()))
                    - (i64::from(self.starts_on.year()) * 12 + i64::from(self.starts_on.month()));
                let day = self
                    .month_day
                    .unwrap_or(1)
                    .min(days_in_month(date.year(), date.month()));
                months % every == 0 && date.day() == day
            }
        }
    }

    /// The first local time the schedule runs strictly after `after`.
    pub fn next_after(&self, after: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut date = after.date().max(self.starts_on);
        // The longest gap is 52 months; look a little further than that.
        for _ in 0..1_700 {
            let at = date.and_time(self.time);
            if at > after && self.occurs_on(date) {
                return Some(at);
            }
            date = date.succ_opt()?;
        }
        None
    }

    fn check(&self) -> Result<(), (&'static str, &'static str)> {
        if !(1..=52).contains(&self.every) {
            return Err(("every", "Repeat every 1 to 52 days, weeks or months."));
        }
        match self.frequency {
            Frequency::Weekly
                if self.weekdays.is_empty()
                    || self.weekdays.iter().any(|day| !(1..=7).contains(day)) =>
            {
                Err(("weekdays", "Pick at least one day of the week."))
            }
            Frequency::Monthly if !self.month_day.is_some_and(|day| (1..=31).contains(&day)) => {
                Err(("monthDay", "Pick a day of the month from 1 to 31."))
            }
            _ => Ok(()),
        }
    }
}

async fn valid_timezone(connection: &mut PgConnection, timezone: &str) -> Result<bool, AppError> {
    if timezone.is_empty() || timezone.len() > 64 {
        return Ok(false);
    }
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_timezone_names WHERE name = $1)",
    )
    .bind(timezone)
    .fetch_one(&mut *connection)
    .await?)
}

/// `at` as a local time in `timezone`.
async fn local_time(
    connection: &mut PgConnection,
    at: DateTime<Utc>,
    timezone: &str,
) -> Result<NaiveDateTime, AppError> {
    Ok(
        sqlx::query_scalar::<_, NaiveDateTime>("SELECT ($1::timestamptz AT TIME ZONE $2)")
            .bind(at)
            .bind(timezone)
            .fetch_one(&mut *connection)
            .await?,
    )
}

/// A local time in `timezone` as an instant.
async fn instant(
    connection: &mut PgConnection,
    local: NaiveDateTime,
    timezone: &str,
) -> Result<DateTime<Utc>, AppError> {
    Ok(
        sqlx::query_scalar::<_, DateTime<Utc>>("SELECT ($1::timestamp AT TIME ZONE $2)")
            .bind(local)
            .bind(timezone)
            .fetch_one(&mut *connection)
            .await?,
    )
}

/// The next run strictly after `after` and after now.
async fn next_run(
    connection: &mut PgConnection,
    schedule: &Schedule,
    timezone: &str,
    after: DateTime<Utc>,
) -> Result<Option<DateTime<Utc>>, AppError> {
    let after = after.max(Utc::now());
    let local = local_time(connection, after, timezone).await?;
    match schedule.next_after(local) {
        Some(next) => Ok(Some(instant(connection, next, timezone).await?)),
        None => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Recurring tasks

#[derive(Clone, Debug, FromRow)]
struct RecurringRow {
    id: Uuid,
    project_id: Uuid,
    template_id: Uuid,
    template_name: String,
    created_by_actor_id: Uuid,
    owner_name: Option<String>,
    frequency: String,
    repeat_every: i32,
    weekdays: Vec<i16>,
    month_day: Option<i16>,
    time_of_day: NaiveTime,
    timezone: String,
    starts_on: NaiveDate,
    assignee_actor_id: Option<Uuid>,
    assignee_name: Option<String>,
    due_in_days: Option<i32>,
    enabled: bool,
    paused_reason: Option<String>,
    next_run_at: Option<DateTime<Utc>>,
    last_run_at: Option<DateTime<Utc>>,
    last_work_item_id: Option<Uuid>,
    last_work_item_key: Option<String>,
    version: i64,
}

impl RecurringRow {
    fn schedule(&self) -> Schedule {
        Schedule {
            frequency: Frequency::parse(&self.frequency).unwrap_or(Frequency::Weekly),
            every: u32::try_from(self.repeat_every).unwrap_or(1),
            weekdays: self
                .weekdays
                .iter()
                .filter_map(|day| u32::try_from(*day).ok())
                .collect(),
            month_day: self.month_day.and_then(|day| u32::try_from(day).ok()),
            time: self.time_of_day,
            starts_on: self.starts_on,
        }
    }
}

const RECURRING_SELECT: &str = r#"
    SELECT recurring.id, recurring.project_id, recurring.template_id,
           template.name AS template_name, recurring.created_by_actor_id,
           owner.display_name AS owner_name,
           recurring.frequency, recurring.repeat_every, recurring.weekdays, recurring.month_day,
           recurring.time_of_day, recurring.timezone, recurring.starts_on,
           recurring.assignee_actor_id, assignee.display_name AS assignee_name,
           recurring.due_in_days, recurring.enabled, recurring.paused_reason,
           recurring.next_run_at, recurring.last_run_at, recurring.last_work_item_id,
           (SELECT project.project_key || '-' || item.number::text
              FROM work_items item JOIN projects project ON project.id = item.project_id
             WHERE item.project_id = recurring.project_id AND item.id = recurring.last_work_item_id
           ) AS last_work_item_key,
           recurring.version
    FROM recurring_tasks recurring
    JOIN task_templates template
      ON template.project_id = recurring.project_id AND template.id = recurring.template_id
    LEFT JOIN project_memberships owner
      ON owner.project_id = recurring.project_id AND owner.actor_id = recurring.created_by_actor_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = recurring.project_id AND assignee.actor_id = recurring.assignee_actor_id
"#;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecurringView {
    pub id: Uuid,
    pub template_id: Uuid,
    pub template_name: String,
    pub frequency: String,
    pub every: i32,
    pub weekdays: Vec<i16>,
    pub month_day: Option<i16>,
    /// HH:MM, local to `timezone`.
    pub time_of_day: String,
    pub timezone: String,
    pub starts_on: NaiveDate,
    pub assignee_actor_id: Option<Uuid>,
    pub assignee_name: Option<String>,
    pub due_in_days: Option<i32>,
    pub enabled: bool,
    pub paused_reason: Option<String>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub last_work_item_id: Option<Uuid>,
    pub last_work_item_key: Option<String>,
    /// Whose access it acts with: whoever last set it up.
    pub acts_as: Option<String>,
    pub version: i64,
}

impl From<RecurringRow> for RecurringView {
    fn from(row: RecurringRow) -> Self {
        Self {
            id: row.id,
            template_id: row.template_id,
            template_name: row.template_name,
            frequency: row.frequency,
            every: row.repeat_every,
            weekdays: row.weekdays,
            month_day: row.month_day,
            time_of_day: row.time_of_day.format("%H:%M").to_string(),
            timezone: row.timezone,
            starts_on: row.starts_on,
            assignee_actor_id: row.assignee_actor_id,
            assignee_name: row.assignee_name,
            due_in_days: row.due_in_days,
            enabled: row.enabled,
            paused_reason: row.paused_reason,
            next_run_at: row.next_run_at,
            last_run_at: row.last_run_at,
            last_work_item_id: row.last_work_item_id,
            last_work_item_key: row.last_work_item_key,
            acts_as: row.owner_name,
            version: row.version,
        }
    }
}

pub(crate) async fn list_recurring(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Vec<RecurringView>, AppError> {
    Ok(sqlx::query_as::<_, RecurringRow>(&format!(
        "{RECURRING_SELECT} WHERE recurring.project_id = $1 ORDER BY recurring.created_at, recurring.id LIMIT 200"
    ))
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(Into::into)
    .collect())
}

async fn recurring_for_update(
    connection: &mut PgConnection,
    project_id: Option<Uuid>,
    recurring_id: Uuid,
) -> Result<Option<RecurringRow>, AppError> {
    Ok(sqlx::query_as::<_, RecurringRow>(&format!(
        "{RECURRING_SELECT} WHERE recurring.id = $1 AND ($2::uuid IS NULL OR recurring.project_id = $2) FOR UPDATE OF recurring"
    ))
    .bind(recurring_id)
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ScheduleRequest {
    frequency: String,
    #[serde(default = "one")]
    every: u32,
    #[serde(default)]
    weekdays: Vec<u32>,
    month_day: Option<u32>,
    /// HH:MM.
    #[serde(default = "nine")]
    time_of_day: String,
    #[serde(default = "utc")]
    timezone: String,
    /// The first day it may run; today when left out.
    starts_on: Option<NaiveDate>,
}

fn one() -> u32 {
    1
}

fn nine() -> String {
    "09:00".to_owned()
}

fn utc() -> String {
    "UTC".to_owned()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateRecurringRequest {
    template_id: Uuid,
    #[serde(flatten)]
    schedule: ScheduleRequest,
    assignee_actor_id: Option<Uuid>,
    due_in_days: Option<i32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateRecurringRequest {
    expected_version: i64,
    #[serde(flatten)]
    schedule: Option<ScheduleRequest>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    assignee_actor_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    due_in_days: Option<Option<i32>>,
    enabled: Option<bool>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DeleteRecurringRequest {
    expected_version: Option<i64>,
}

/// A schedule from a request, checked; the error names the field.
async fn read_schedule(
    connection: &mut PgConnection,
    request: &ScheduleRequest,
) -> Result<Result<(Schedule, String), Response>, AppError> {
    let Some(frequency) = Frequency::parse(&request.frequency) else {
        return Ok(Err(invalid(
            "frequency",
            "Repeat daily, weekly or monthly.",
        )));
    };
    let Ok(time) = NaiveTime::parse_from_str(&request.time_of_day, "%H:%M") else {
        return Ok(Err(invalid("timeOfDay", "Use a time such as 09:00.")));
    };
    let timezone = request.timezone.trim().to_owned();
    if !valid_timezone(connection, &timezone).await? {
        return Ok(Err(invalid(
            "timezone",
            "Use a time zone such as Europe/Amsterdam or UTC.",
        )));
    }
    let today = local_time(connection, Utc::now(), &timezone).await?.date();
    let mut weekdays = request.weekdays.clone();
    weekdays.sort_unstable();
    weekdays.dedup();
    let schedule = Schedule {
        frequency,
        every: request.every,
        weekdays: if frequency == Frequency::Weekly {
            weekdays
        } else {
            Vec::new()
        },
        month_day: if frequency == Frequency::Monthly {
            request.month_day
        } else {
            None
        },
        time,
        starts_on: request.starts_on.unwrap_or(today),
    };
    if let Err((field, message)) = schedule.check() {
        return Ok(Err(invalid(field, message)));
    }
    Ok(Ok((schedule, timezone)))
}

fn check_due_in_days(value: Option<i32>) -> Option<Response> {
    value
        .filter(|days| !(0..=365).contains(days))
        .map(|_| invalid("dueInDays", "Make it due 0 to 365 days after it's created."))
}

async fn assignable(
    connection: &mut PgConnection,
    project_id: Uuid,
    assignee: Option<Uuid>,
) -> Result<bool, AppError> {
    crate::work_item_commands::validate_assignee(connection, project_id, assignee).await
}

fn weekdays_column(schedule: &Schedule) -> Vec<i16> {
    schedule
        .weekdays
        .iter()
        .filter_map(|day| i16::try_from(*day).ok())
        .collect()
}

pub(super) async fn create_recurring(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateRecurringRequest>,
) -> Result<Response, AppError> {
    if let Some(refused) = check_due_in_days(request.due_in_days) {
        return Ok(refused);
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemCreate,
            None,
            resource_bound_scope("projects", project_id, "recurring_tasks_create")?,
            &request,
        )
        .await
    );
    let (schedule, timezone) =
        match read_schedule(&mut write.transaction, &request.schedule).await? {
            Ok(read) => read,
            Err(refused) => return Ok(refused),
        };
    let Some(template) =
        template_for_update(&mut write.transaction, project_id, request.template_id).await?
    else {
        return Ok(invalid(
            "templateId",
            "That template isn't in this project.",
        ));
    };
    if template.archived {
        return Ok(invalid(
            "templateId",
            "That template is archived. Restore it first.",
        ));
    }
    if !assignable(
        &mut write.transaction,
        project_id,
        request.assignee_actor_id,
    )
    .await?
    {
        return Ok(invalid(
            "assigneeActorId",
            "That person or agent can't be assigned tasks in this project.",
        ));
    }
    let count =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM recurring_tasks WHERE project_id = $1")
            .bind(project_id)
            .fetch_one(&mut *write.transaction)
            .await?;
    if count >= MAX_RECURRING {
        return Ok(refusal(
            StatusCode::CONFLICT,
            "too_many_recurring_tasks",
            "This project has 100 recurring tasks. Delete one you no longer need first.".to_owned(),
            json!({}),
        ));
    }
    let after = instant(
        &mut write.transaction,
        schedule.starts_on.and_time(NaiveTime::MIN),
        &timezone,
    )
    .await?
        - Duration::seconds(1);
    let next = next_run(&mut write.transaction, &schedule, &timezone, after).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO recurring_tasks (
            id, project_id, template_id, created_by_actor_id, frequency, repeat_every,
            weekdays, month_day, time_of_day, timezone, starts_on, assignee_actor_id,
            due_in_days, next_run_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(request.template_id)
    .bind(principal.actor_id)
    .bind(schedule.frequency.as_str())
    .bind(schedule.every as i32)
    .bind(weekdays_column(&schedule))
    .bind(schedule.month_day.map(|day| day as i16))
    .bind(schedule.time)
    .bind(&timezone)
    .bind(schedule.starts_on)
    .bind(request.assignee_actor_id)
    .bind(request.due_in_days)
    .bind(next)
    .execute(&mut *write.transaction)
    .await?;
    let actor = write.actor.clone();
    record_audit_for(
        &mut write.transaction,
        &actor,
        "recurring_task",
        "recurring_task.created",
        id,
        json!({}),
    )
    .await?;
    let row = recurring_for_update(&mut write.transaction, Some(project_id), id)
        .await?
        .ok_or(AppError::Internal("recurring task just made is missing"))?;
    write
        .finish(StatusCode::CREATED, &RecurringView::from(row))
        .await
}

fn recurring_conflict(row: RecurringRow) -> Response {
    let version = row.version;
    refusal(
        StatusCode::CONFLICT,
        "version_conflict",
        "Someone changed this schedule first. You’re now looking at the latest version.".to_owned(),
        json!({ "resource": "recurring_task", "currentVersion": version, "current": RecurringView::from(row) }),
    )
}

pub(super) async fn update_recurring(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, recurring_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateRecurringRequest>,
) -> Result<Response, AppError> {
    if let Some(refused) = check_due_in_days(request.due_in_days.flatten()) {
        return Ok(refused);
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemCreate,
            None,
            resource_bound_scope("recurring_tasks", recurring_id, "update")?,
            &request,
        )
        .await
    );
    let row = recurring_for_update(&mut write.transaction, Some(project_id), recurring_id)
        .await?
        .ok_or(AppError::NotFound("recurring_task"))?;
    if row.version != request.expected_version {
        return Ok(recurring_conflict(row));
    }
    let (schedule, timezone) = match &request.schedule {
        Some(schedule) => match read_schedule(&mut write.transaction, schedule).await? {
            Ok(read) => read,
            Err(refused) => return Ok(refused),
        },
        None => (row.schedule(), row.timezone.clone()),
    };
    let assignee = request.assignee_actor_id.unwrap_or(row.assignee_actor_id);
    if request.assignee_actor_id.is_some()
        && !assignable(&mut write.transaction, project_id, assignee).await?
    {
        return Ok(invalid(
            "assigneeActorId",
            "That person or agent can't be assigned tasks in this project.",
        ));
    }
    let enabled = request.enabled.unwrap_or(row.enabled);
    if enabled {
        let template_archived = sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NOT NULL FROM task_templates WHERE project_id = $1 AND id = $2",
        )
        .bind(project_id)
        .bind(row.template_id)
        .fetch_one(&mut *write.transaction)
        .await?;
        if template_archived {
            return Ok(invalid(
                "enabled",
                "Its template is archived. Restore the template to start it again.",
            ));
        }
    }
    let after = match row.last_run_at {
        Some(last) => last,
        None => {
            instant(
                &mut write.transaction,
                schedule.starts_on.and_time(NaiveTime::MIN),
                &timezone,
            )
            .await?
                - Duration::seconds(1)
        }
    };
    let next = if enabled {
        next_run(&mut write.transaction, &schedule, &timezone, after).await?
    } else {
        None
    };
    // Whoever sets a schedule up again is who it acts as from now on.
    sqlx::query(
        r#"
        UPDATE recurring_tasks
        SET created_by_actor_id = $3, frequency = $4, repeat_every = $5, weekdays = $6,
            month_day = $7, time_of_day = $8, timezone = $9, starts_on = $10,
            assignee_actor_id = $11, due_in_days = $12, enabled = $13, next_run_at = $14,
            paused_reason = CASE WHEN $13 THEN NULL ELSE paused_reason END,
            version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(project_id)
    .bind(recurring_id)
    .bind(principal.actor_id)
    .bind(schedule.frequency.as_str())
    .bind(schedule.every as i32)
    .bind(weekdays_column(&schedule))
    .bind(schedule.month_day.map(|day| day as i16))
    .bind(schedule.time)
    .bind(&timezone)
    .bind(schedule.starts_on)
    .bind(assignee)
    .bind(request.due_in_days.unwrap_or(row.due_in_days))
    .bind(enabled)
    .bind(next)
    .execute(&mut *write.transaction)
    .await?;
    let actor = write.actor.clone();
    record_audit_for(
        &mut write.transaction,
        &actor,
        "recurring_task",
        "recurring_task.updated",
        recurring_id,
        json!({ "enabled": enabled }),
    )
    .await?;
    let row = recurring_for_update(&mut write.transaction, Some(project_id), recurring_id)
        .await?
        .ok_or(AppError::Internal("recurring task just changed is missing"))?;
    write
        .finish(StatusCode::OK, &RecurringView::from(row))
        .await
}

pub(super) async fn delete_recurring(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, recurring_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<DeleteRecurringRequest>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemCreate,
            None,
            resource_bound_scope("recurring_tasks", recurring_id, "delete")?,
            &request,
        )
        .await
    );
    let row = recurring_for_update(&mut write.transaction, Some(project_id), recurring_id)
        .await?
        .ok_or(AppError::NotFound("recurring_task"))?;
    if request
        .expected_version
        .is_some_and(|expected| expected != row.version)
    {
        return Ok(recurring_conflict(row));
    }
    // The tasks it made stay; only the record of which occurrence made them
    // goes with it.
    sqlx::query("DELETE FROM recurring_tasks WHERE project_id = $1 AND id = $2")
        .bind(project_id)
        .bind(recurring_id)
        .execute(&mut *write.transaction)
        .await?;
    let actor = write.actor.clone();
    record_audit_for(
        &mut write.transaction,
        &actor,
        "recurring_task",
        "recurring_task.deleted",
        recurring_id,
        json!({}),
    )
    .await?;
    write
        .finish(StatusCode::OK, &json!({ "deleted": true }))
        .await
}

// ---------------------------------------------------------------------------
// Jobs

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OccurrencePayload {
    recurring_task_id: Uuid,
    occurrence: DateTime<Utc>,
}

pub(crate) fn register_jobs(registry: &mut JobRegistry) {
    registry.register(SWEEP_KIND, |context: JobContext| async move {
        queue_due(&context.state.pool).await?;
        Ok(())
    });
    registry.every(SWEEP_KIND, StdDuration::from_secs(60));
    registry.register(OCCURRENCE_KIND, |context: JobContext| async move {
        create_occurrence(&context).await
    });
}

/// Queues one job for each schedule that is due. The unique key holds one
/// job per occurrence however often this runs.
pub(crate) async fn queue_due(pool: &PgPool) -> Result<usize, AppError> {
    let due = sqlx::query_as::<_, (Uuid, Uuid, Uuid, DateTime<Utc>)>(
        r#"
        SELECT recurring.id, recurring.project_id, project.organization_id, recurring.next_run_at
        FROM recurring_tasks recurring
        JOIN projects project ON project.id = recurring.project_id
        WHERE recurring.enabled AND recurring.next_run_at <= now()
        ORDER BY recurring.next_run_at
        LIMIT 500
        "#,
    )
    .fetch_all(pool)
    .await?;
    let count = due.len();
    for (id, project_id, organization_id, occurrence) in due {
        enqueue(
            pool,
            NewJob::new(OCCURRENCE_KIND)
                .project(organization_id, project_id)
                .payload(OccurrencePayload {
                    recurring_task_id: id,
                    occurrence,
                })
                .unique(format!(
                    "recurring-task:{id}:{}",
                    occurrence.timestamp_micros()
                )),
        )
        .await?;
    }
    Ok(count)
}

async fn pause(
    transaction: &mut Transaction<'_, Postgres>,
    recurring_id: Uuid,
    reason: &str,
) -> Result<(), AppError> {
    sqlx::query(
        r#"
        UPDATE recurring_tasks
        SET enabled = false, next_run_at = NULL, paused_reason = $2,
            version = version + 1, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(recurring_id)
    .bind(reason)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

/// Why the person a schedule acts as can no longer create tasks with it, if
/// they can't: they left the project or workspace, lost the role, or the
/// project was archived.
async fn acting_actor(
    transaction: &mut Transaction<'_, Postgres>,
    row: &RecurringRow,
    job_id: Uuid,
) -> Result<Result<CommandActor, String>, AppError> {
    let who = row
        .owner_name
        .clone()
        .unwrap_or_else(|| "Its owner".to_owned());
    let Some(membership) =
        lock_membership(transaction, row.created_by_actor_id, row.project_id).await?
    else {
        return Ok(Err(format!(
            "Paused because {who} is no longer in this project. Start it again to have it act as you."
        )));
    };
    if membership.project_archived_at.is_some() {
        return Ok(Err("Paused because the project is archived.".to_owned()));
    }
    let kind = sqlx::query_scalar::<_, String>("SELECT kind FROM actors WHERE id = $1")
        .bind(row.created_by_actor_id)
        .fetch_one(&mut **transaction)
        .await?;
    let actor_kind = match kind.as_str() {
        "human" => ActorKind::Human,
        "external_agent" => ActorKind::ExternalAgent,
        _ => return Ok(Err(format!("Paused because {who} can't create tasks."))),
    };
    let actor = CommandActor {
        actor_id: row.created_by_actor_id,
        actor_kind,
        project_id: row.project_id,
        audit: AuditAttribution::Job {
            job_id,
            source: "recurring_task",
        },
    };
    match authorize_mutation(&actor, &membership, Action::WorkItemCreate, None)? {
        Decision::Allow => Ok(Ok(actor)),
        Decision::Deny(_) => Ok(Err(format!(
            "Paused because {who} can no longer create tasks in this project. Start it again to have it act as you."
        ))),
    }
}

async fn create_occurrence(context: &JobContext) -> Result<(), JobError> {
    let payload: OccurrencePayload = context.job.payload_as()?;
    let pool = &context.state.pool;
    let mut transaction = pool.begin().await?;
    let Some(row) = recurring_for_update(&mut transaction, None, payload.recurring_task_id).await?
    else {
        return Ok(());
    };
    if context.job.project_id != Some(row.project_id) {
        return Err(JobError::permanent("the job names another project"));
    }
    // Moved, paused or already done by an earlier attempt.
    if !row.enabled || row.next_run_at != Some(payload.occurrence) {
        return Ok(());
    }
    let done = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM recurring_task_occurrences WHERE recurring_task_id = $1 AND occurrence_at = $2)",
    )
    .bind(row.id)
    .bind(payload.occurrence)
    .fetch_one(&mut *transaction)
    .await?;
    let schedule = row.schedule();
    let next = next_run(
        &mut transaction,
        &schedule,
        &row.timezone,
        payload.occurrence,
    )
    .await?;
    if done {
        sqlx::query("UPDATE recurring_tasks SET next_run_at = $2 WHERE id = $1")
            .bind(row.id)
            .bind(next)
            .execute(&mut *transaction)
            .await?;
        transaction.commit().await?;
        return Ok(());
    }
    let actor = match acting_actor(&mut transaction, &row, context.job.id).await? {
        Ok(actor) => actor,
        Err(reason) => {
            pause(&mut transaction, row.id, &reason).await?;
            transaction.commit().await?;
            return Ok(());
        }
    };
    let local = local_time(&mut transaction, payload.occurrence, &row.timezone).await?;
    let due_date = row
        .due_in_days
        .map(|days| local.date() + Duration::days(i64::from(days)));
    let mut options = ApplyOptions {
        assignee_actor_id: row.assignee_actor_id,
        due_date,
        ..ApplyOptions::default()
    };
    // Someone who has since left can't be assigned; the task is still made.
    if !assignable(&mut transaction, row.project_id, options.assignee_actor_id).await? {
        options.assignee_actor_id = None;
    }
    let applied = match apply_template(&mut transaction, &actor, row.template_id, &options).await? {
        ApplyOutcome::Created(applied) => applied,
        ApplyOutcome::Refused(_, message) => {
            pause(&mut transaction, row.id, &format!("Paused: {message}")).await?;
            transaction.commit().await?;
            return Ok(());
        }
    };
    sqlx::query(
        r#"
        INSERT INTO recurring_task_occurrences (id, project_id, recurring_task_id, occurrence_at, work_item_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(row.project_id)
    .bind(row.id)
    .bind(payload.occurrence)
    .bind(applied.work_item_id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query(
        r#"
        UPDATE recurring_tasks
        SET next_run_at = $2, last_run_at = $3, last_work_item_id = $4, updated_at = now()
        WHERE id = $1
        "#,
    )
    .bind(row.id)
    .bind(next)
    .bind(payload.occurrence)
    .bind(applied.work_item_id)
    .execute(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("test date")
    }

    fn at(day: NaiveDate, hour: u32) -> NaiveDateTime {
        day.and_hms_opt(hour, 0, 0).expect("test time")
    }

    fn schedule(frequency: Frequency, every: u32) -> Schedule {
        Schedule {
            frequency,
            every,
            weekdays: vec![1],
            month_day: Some(31),
            time: NaiveTime::from_hms_opt(9, 0, 0).expect("test time"),
            // A Wednesday.
            starts_on: date(2026, 10, 14),
        }
    }

    #[test]
    fn weekly_runs_on_its_weekdays_every_n_weeks() {
        let weekly = schedule(Frequency::Weekly, 1);
        // The first Monday on or after the start, at nine.
        assert_eq!(
            weekly.next_after(at(date(2026, 10, 14), 0)),
            Some(at(date(2026, 10, 19), 9))
        );
        // Strictly after: nine on a Monday moves to the next Monday.
        assert_eq!(
            weekly.next_after(at(date(2026, 10, 19), 9)),
            Some(at(date(2026, 10, 26), 9))
        );
        let fortnightly = Schedule {
            weekdays: vec![1, 5],
            ..schedule(Frequency::Weekly, 2)
        };
        // Week of the start (Oct 12) counts as week 0: Fri Oct 16, then the
        // week of Oct 26.
        assert_eq!(
            fortnightly.next_after(at(date(2026, 10, 14), 0)),
            Some(at(date(2026, 10, 16), 9))
        );
        assert_eq!(
            fortnightly.next_after(at(date(2026, 10, 16), 9)),
            Some(at(date(2026, 10, 26), 9))
        );
    }

    #[test]
    fn monthly_uses_the_last_day_of_short_months() {
        let monthly = schedule(Frequency::Monthly, 1);
        assert_eq!(
            monthly.next_after(at(date(2026, 10, 14), 0)),
            Some(at(date(2026, 10, 31), 9))
        );
        assert_eq!(
            monthly.next_after(at(date(2026, 10, 31), 9)),
            Some(at(date(2026, 11, 30), 9))
        );
        assert_eq!(
            monthly.next_after(at(date(2027, 1, 31), 10)),
            Some(at(date(2027, 2, 28), 9))
        );
        let quarterly = schedule(Frequency::Monthly, 3);
        assert_eq!(
            quarterly.next_after(at(date(2026, 10, 31), 9)),
            Some(at(date(2027, 1, 31), 9))
        );
    }

    #[test]
    fn daily_counts_from_the_start() {
        let every_other_day = schedule(Frequency::Daily, 2);
        assert_eq!(
            every_other_day.next_after(at(date(2026, 10, 14), 10)),
            Some(at(date(2026, 10, 16), 9))
        );
        assert_eq!(
            every_other_day.next_after(at(date(2026, 10, 1), 0)),
            Some(at(date(2026, 10, 14), 9))
        );
    }

    #[test]
    fn schedules_say_what_is_missing() {
        let mut weekly = schedule(Frequency::Weekly, 1);
        weekly.weekdays.clear();
        assert_eq!(weekly.check().map_err(|(field, _)| field), Err("weekdays"));
        let mut monthly = schedule(Frequency::Monthly, 1);
        monthly.month_day = None;
        assert_eq!(monthly.check().map_err(|(field, _)| field), Err("monthDay"));
        assert_eq!(
            schedule(Frequency::Daily, 53)
                .check()
                .map_err(|(field, _)| field),
            Err("every")
        );
        assert!(schedule(Frequency::Daily, 1).check().is_ok());
    }

    #[test]
    fn templates_are_tidied_and_checked() {
        let fields = normalize_template(TemplateFields {
            name: "  Weekly   review ".to_owned(),
            title: "Review the week".to_owned(),
            labels: vec!["ops".to_owned(), "OPS".to_owned()],
            acceptance_criteria: vec!["Notes posted".to_owned(), "  ".to_owned()],
            subtasks: vec![
                SubtaskDraft {
                    title: "Collect numbers".to_owned(),
                    description: String::new(),
                },
                SubtaskDraft::default(),
            ],
            priority: "high".to_owned(),
            ..TemplateFields::default()
        })
        .expect("valid template");
        assert_eq!(fields.name, "Weekly review");
        assert_eq!(fields.labels, vec!["ops".to_owned()]);
        assert_eq!(fields.acceptance_criteria, vec!["Notes posted".to_owned()]);
        assert_eq!(fields.subtasks.len(), 1);
        let refused = normalize_template(TemplateFields {
            name: "x".to_owned(),
            title: "y".to_owned(),
            priority: "someday".to_owned(),
            ..TemplateFields::default()
        });
        assert_eq!(refused.map_err(|(field, _)| field), Err("priority"));
    }
}
