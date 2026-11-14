//! Project templates (docs/plans/roadmap.md, W11, T10).
//!
//! A project template is a snapshot of one project's setup: its workflow
//! (statuses, types, transitions, estimate unit), labels, channel names,
//! project settings and task templates. It belongs to the workspace, so any
//! member can start a new project from it. Only owners and admins of the
//! source project can save one, since it copies the project's setup; the
//! channel names it keeps are the ones the saver can see. Settings that
//! name something (a repository, an environment) stay behind, because they
//! point into the source project.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    planning::{invalid, refusal},
    templates::{
        SubtaskDraft, TaskTemplateView, TemplateFields, insert_template, list_templates,
        normalize_template, normalize_template_name,
    },
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    error::{ApiJson, ApiPath, AppError},
    policy::{Permission, RoleGrant},
    settings::{self, SettingType, Target},
    work_item_commands::{AuditAttribution, CommandActor},
    work_item_labels::{self, NewLabel},
    work_item_workflow::{
        StatusDraft, TransitionDraft, TransitionsDraft, TypeDraft, WorkflowDocument,
        WorkflowRefusal, current_document, replace_workflow,
    },
};

const MAX_PROJECT_TEMPLATES: i64 = 100;
const MAX_DESCRIPTION_CHARACTERS: usize = 500;
const MAX_CHANNELS: i64 = 50;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TemplateContent {
    #[serde(default)]
    pub workflow: Option<WorkflowContent>,
    #[serde(default)]
    pub labels: Vec<LabelContent>,
    #[serde(default)]
    pub channels: Vec<String>,
    #[serde(default)]
    pub settings: serde_json::Map<String, Value>,
    #[serde(default)]
    pub task_templates: Vec<TaskTemplateContent>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorkflowContent {
    pub estimate_unit: String,
    pub statuses: Vec<StatusContent>,
    pub types: Vec<TypeContent>,
    /// Pairs of status names; none means any status can move to any other.
    pub transitions: Option<Vec<(String, String)>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StatusContent {
    pub name: String,
    pub semantic_category: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TypeContent {
    pub name: String,
    pub is_default: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LabelContent {
    pub name: String,
    pub color: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TaskTemplateContent {
    pub name: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    /// The type by name, matched in the new project.
    pub type_name: Option<String>,
    pub priority: String,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    #[serde(default)]
    pub subtasks: Vec<SubtaskDraft>,
    pub estimate: Option<f64>,
}

impl From<TaskTemplateView> for TaskTemplateContent {
    fn from(view: TaskTemplateView) -> Self {
        Self {
            name: view.name,
            title: view.title,
            description: view.description,
            type_name: view.type_name,
            priority: view.priority,
            labels: view.labels,
            acceptance_criteria: view.acceptance_criteria,
            subtasks: view.subtasks,
            estimate: view.estimate,
        }
    }
}

/// What a template holds, counted, for a picker.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContentSummary {
    pub statuses: Vec<String>,
    pub types: Vec<String>,
    pub labels: usize,
    pub channels: Vec<String>,
    pub settings: usize,
    pub task_templates: usize,
}

impl TemplateContent {
    fn summary(&self) -> ContentSummary {
        ContentSummary {
            statuses: self
                .workflow
                .iter()
                .flat_map(|workflow| workflow.statuses.iter().map(|status| status.name.clone()))
                .collect(),
            types: self
                .workflow
                .iter()
                .flat_map(|workflow| workflow.types.iter().map(|kind| kind.name.clone()))
                .collect(),
            labels: self.labels.len(),
            channels: self.channels.clone(),
            settings: self.settings.len(),
            task_templates: self.task_templates.len(),
        }
    }
}

#[derive(Debug, FromRow)]
struct ProjectTemplateRow {
    id: Uuid,
    organization_id: Uuid,
    name: String,
    description: String,
    content: Value,
    source_project_id: Option<Uuid>,
    source_project_name: Option<String>,
    created_by_actor_id: Uuid,
    created_by_name: Option<String>,
    archived: bool,
    version: i64,
}

const PROJECT_TEMPLATE_SELECT: &str = r#"
    SELECT template.id, template.organization_id, template.name, template.description,
           template.content, template.source_project_id,
           source.name AS source_project_name, template.created_by_actor_id,
           creator.display_name AS created_by_name,
           template.archived_at IS NOT NULL AS archived, template.version
    FROM project_templates template
    LEFT JOIN projects source ON source.id = template.source_project_id
    LEFT JOIN users creator ON creator.actor_id = template.created_by_actor_id
"#;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProjectTemplateView {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub name: String,
    pub description: String,
    pub contains: ContentSummary,
    /// Set only for readers who are members of the source project.
    pub source_project_id: Option<Uuid>,
    pub source_project_name: Option<String>,
    pub created_by_name: Option<String>,
    pub archived: bool,
    pub can_edit: bool,
    pub version: i64,
}

fn template_view(
    row: ProjectTemplateRow,
    can_edit: bool,
    source_visible: bool,
) -> ProjectTemplateView {
    let content: TemplateContent = serde_json::from_value(row.content).unwrap_or_default();
    ProjectTemplateView {
        id: row.id,
        organization_id: row.organization_id,
        name: row.name,
        description: row.description,
        contains: content.summary(),
        source_project_id: row.source_project_id.filter(|_| source_visible),
        source_project_name: row.source_project_name.filter(|_| source_visible),
        created_by_name: row.created_by_name,
        archived: row.archived,
        can_edit,
        version: row.version,
    }
}

/// The caller's role in the workspace if they're an active member.
async fn workspace_role(
    connection: &mut PgConnection,
    actor_id: Uuid,
    organization_id: Uuid,
) -> Result<Option<RoleGrant>, AppError> {
    let stored = sqlx::query_as::<_, (String, Option<Vec<String>>)>(
        r#"
        SELECT membership.role, workspace_role_permissions(membership.role_id)
        FROM organization_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id AND actor.status = 'active'
        WHERE membership.organization_id = $1 AND membership.actor_id = $2
          AND membership.enabled
        "#,
    )
    .bind(organization_id)
    .bind(actor_id)
    .fetch_optional(&mut *connection)
    .await?;
    stored
        .map(|(role, permissions)| Ok(RoleGrant::stored(&role, permissions.as_deref())?))
        .transpose()
}

/// Whoever made the template, or someone who manages the workspace's
/// settings.
fn may_edit(reader: TemplateReader, role: &RoleGrant, row: &ProjectTemplateRow) -> bool {
    reader.human
        && (row.created_by_actor_id == reader.actor_id
            || role.permissions().contains(Permission::SettingsManage))
}

/// Who reads or changes project templates: their actor, and whether they're
/// a person (agents never edit one).
#[derive(Clone, Copy, Debug)]
pub(crate) struct TemplateReader {
    pub actor_id: Uuid,
    pub human: bool,
}

impl TemplateReader {
    fn browser(principal: &AuthenticatedActor) -> Self {
        Self {
            actor_id: principal.actor_id,
            human: principal.actor_kind == "human",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ProjectTemplatesResponse {
    templates: Vec<ProjectTemplateView>,
}

/// The workspace's project templates, newest first.
pub(crate) async fn templates_for(
    connection: &mut PgConnection,
    reader: TemplateReader,
    organization_id: Uuid,
    include_archived: bool,
) -> Result<Vec<ProjectTemplateView>, AppError> {
    let role = workspace_role(connection, reader.actor_id, organization_id)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    let rows = sqlx::query_as::<_, ProjectTemplateRow>(&format!(
        r#"{PROJECT_TEMPLATE_SELECT}
        WHERE template.organization_id = $1 AND ($2 OR template.archived_at IS NULL)
        ORDER BY template.archived_at IS NOT NULL, template.created_at DESC, template.id
        LIMIT 200"#
    ))
    .bind(organization_id)
    .bind(include_archived)
    .fetch_all(&mut *connection)
    .await?;
    let visible_sources = sqlx::query_scalar::<_, Uuid>(
        "SELECT project_id FROM project_memberships WHERE actor_id = $1 AND enabled",
    )
    .bind(reader.actor_id)
    .fetch_all(&mut *connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let can_edit = may_edit(reader, &role, &row);
            let visible = row
                .source_project_id
                .is_some_and(|id| visible_sources.contains(&id));
            template_view(row, can_edit, visible)
        })
        .collect())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ListQuery {
    #[serde(default)]
    include_archived: bool,
}

pub(super) async fn list_project_templates(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(organization_id): ApiPath<Uuid>,
    crate::error::ApiQuery(query): crate::error::ApiQuery<ListQuery>,
) -> Result<Json<ProjectTemplatesResponse>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let templates = templates_for(
        &mut connection,
        TemplateReader::browser(&principal),
        organization_id,
        query.include_archived,
    )
    .await?;
    Ok(Json(ProjectTemplatesResponse { templates }))
}

/// Reads one project's setup as template content. The caller has checked
/// they may manage the project; channels are the ones they can see.
pub(crate) async fn snapshot(
    connection: &mut PgConnection,
    project_id: Uuid,
    reader_actor_id: Uuid,
) -> Result<TemplateContent, AppError> {
    let document = current_document(connection, project_id).await?;
    let names = document
        .statuses
        .iter()
        .filter_map(|status| status.id.map(|id| (id.to_string(), status.name.clone())))
        .collect::<std::collections::HashMap<_, _>>();
    let transitions = match &document.transitions {
        TransitionsDraft::All(_) => None,
        TransitionsDraft::List(list) => {
            let pairs = list
                .iter()
                .filter_map(|transition| {
                    Some((
                        names.get(&transition.from)?.clone(),
                        names.get(&transition.to)?.clone(),
                    ))
                })
                .collect::<Vec<_>>();
            let count = document.statuses.len();
            (pairs.len() < count * count.saturating_sub(1)).then_some(pairs)
        }
    };
    let workflow = WorkflowContent {
        estimate_unit: document
            .estimate_unit
            .clone()
            .unwrap_or_else(|| "points".to_owned()),
        statuses: document
            .statuses
            .iter()
            .map(|status| StatusContent {
                name: status.name.clone(),
                semantic_category: status.semantic_category.clone(),
                is_default: status.is_default,
            })
            .collect(),
        types: document
            .types
            .iter()
            .map(|kind| TypeContent {
                name: kind.name.clone(),
                is_default: kind.is_default,
            })
            .collect(),
        transitions,
    };
    let labels = sqlx::query_as::<_, (String, String, String)>(
        r#"
        SELECT name, color, description FROM project_labels
        WHERE project_id = $1 AND archived_at IS NULL
        ORDER BY lower(name), id
        "#,
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    .into_iter()
    .map(|(name, color, description)| LabelContent {
        name,
        color,
        description,
    })
    .collect();
    let channels = sqlx::query_scalar::<_, String>(
        r#"
        SELECT name FROM conversations
        WHERE project_id = $1 AND kind = 'channel' AND archived_at IS NULL
          AND name IS NOT NULL AND conversation_visible_to(id, $2)
        ORDER BY created_at, id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(reader_actor_id)
    .bind(MAX_CHANNELS)
    .fetch_all(&mut *connection)
    .await?;
    let mut settings_map = serde_json::Map::new();
    for (key, value) in sqlx::query_as::<_, (String, Value)>(
        "SELECT key, value FROM settings WHERE scope = 'project' AND project_id = $1 ORDER BY key",
    )
    .bind(project_id)
    .fetch_all(&mut *connection)
    .await?
    {
        if copyable_setting(&key, &value) {
            settings_map.insert(key, value);
        }
    }
    let task_templates = list_templates(connection, project_id, false)
        .await?
        .into_iter()
        .map(|row| TaskTemplateContent::from(TaskTemplateView::from(row)))
        .collect();
    Ok(TemplateContent {
        workflow: Some(workflow),
        labels,
        channels,
        settings: settings_map,
        task_templates,
    })
}

/// Settings a template carries: known project settings that don't name
/// something in the source project.
fn copyable_setting(key: &str, value: &Value) -> bool {
    settings::definition(key).is_some_and(|definition| {
        definition.scopes.contains(&settings::Scope::Project)
            && !matches!(definition.kind, SettingType::Identifier { .. })
            && definition.validate(value).is_ok()
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SaveAsTemplateRequest {
    name: String,
    #[serde(default)]
    description: String,
}

fn normalize_description(value: &str) -> Option<String> {
    let text = value.trim();
    (text.chars().count() <= MAX_DESCRIPTION_CHARACTERS
        && !text
            .chars()
            .any(|character| character.is_control() && character != '\n'))
    .then(|| text.to_owned())
}

fn name_taken() -> Response {
    refusal(
        StatusCode::CONFLICT,
        "name_taken",
        "The workspace already has a project template with that name.".to_owned(),
        json!({ "field": "name" }),
    )
}

/// What saving or changing a project template came to.
pub(crate) enum ProjectTemplateOutcome {
    /// The template as it is now, and whether this change archived it.
    Done(Box<ProjectTemplateView>, bool),
    Invalid(&'static str, &'static str),
    NameTaken,
    LimitReached,
    TooLarge,
}

pub(super) async fn save_as_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<SaveAsTemplateRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if let Some(refused) = check_name_and_description(&request.name, &request.description) {
        return Ok(outcome_response(refused, StatusCode::CREATED));
    }
    let mut transaction = state.pool.begin().await?;
    let organization_id =
        settings::authorize_project_manage(&mut *transaction, &principal, project_id).await?;
    let outcome = save_project_template(
        &mut transaction,
        principal.actor_id,
        organization_id,
        project_id,
        &request.name,
        &request.description,
    )
    .await?;
    if let ProjectTemplateOutcome::Done(view, _) = &outcome {
        settings::audit(
            &mut transaction,
            &headers,
            &principal,
            Some(project_id),
            "project_template.created",
            "project_template",
            Some(view.id),
            json!({ "organizationId": organization_id }),
        )
        .await?;
        transaction.commit().await?;
    }
    Ok(outcome_response(outcome, StatusCode::CREATED))
}

/// A name or description the template can't take, checked before anything
/// is locked.
fn check_name_and_description(name: &str, description: &str) -> Option<ProjectTemplateOutcome> {
    if normalize_template_name(name).is_none() {
        return Some(ProjectTemplateOutcome::Invalid(
            "name",
            "Give the template a name of 1 to 80 characters.",
        ));
    }
    if normalize_description(description).is_none() {
        return Some(ProjectTemplateOutcome::Invalid(
            "description",
            "Keep the description to 500 characters.",
        ));
    }
    None
}

/// The web's answer for an outcome: `done` is the status of a success.
fn outcome_response(outcome: ProjectTemplateOutcome, done: StatusCode) -> Response {
    match outcome {
        ProjectTemplateOutcome::Done(view, _) => (done, Json(*view)).into_response(),
        ProjectTemplateOutcome::Invalid(field, message) => invalid(field, message),
        ProjectTemplateOutcome::NameTaken => name_taken(),
        ProjectTemplateOutcome::LimitReached => refusal(
            StatusCode::CONFLICT,
            "limit_reached",
            format!(
                "A workspace can keep {MAX_PROJECT_TEMPLATES} project templates. Archive one first."
            ),
            json!({}),
        ),
        ProjectTemplateOutcome::TooLarge => refusal(
            StatusCode::UNPROCESSABLE_ENTITY,
            "too_large",
            "This project's setup is too large to save as a template.".to_owned(),
            json!({}),
        ),
    }
}

/// Save `project_id`'s setup as a new template of `organization_id`, its
/// workspace. The caller has checked `actor_id` may manage the project and
/// records the audit row.
pub(crate) async fn save_project_template(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    organization_id: Uuid,
    project_id: Uuid,
    name: &str,
    description: &str,
) -> Result<ProjectTemplateOutcome, AppError> {
    if let Some(refused) = check_name_and_description(name, description) {
        return Ok(refused);
    }
    let name = normalize_template_name(name).unwrap_or_default();
    let description = normalize_description(description).unwrap_or_default();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("project-templates:{organization_id}"))
        .execute(&mut **transaction)
        .await?;
    let (count, taken) = sqlx::query_as::<_, (i64, bool)>(
        r#"
        SELECT count(*), COALESCE(bool_or(lower(name) = lower($2)), false)
        FROM project_templates
        WHERE organization_id = $1 AND archived_at IS NULL
        "#,
    )
    .bind(organization_id)
    .bind(&name)
    .fetch_one(&mut **transaction)
    .await?;
    if taken {
        return Ok(ProjectTemplateOutcome::NameTaken);
    }
    if count >= MAX_PROJECT_TEMPLATES {
        return Ok(ProjectTemplateOutcome::LimitReached);
    }
    let content = snapshot(transaction, project_id, actor_id).await?;
    let content_value =
        serde_json::to_value(&content).map_err(|_| AppError::Internal("serialize template"))?;
    if content_value.to_string().len() > 262_144 {
        return Ok(ProjectTemplateOutcome::TooLarge);
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO project_templates (
            id, organization_id, name, description, content, source_project_id,
            created_by_actor_id
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(id)
    .bind(organization_id)
    .bind(&name)
    .bind(&description)
    .bind(&content_value)
    .bind(project_id)
    .bind(actor_id)
    .execute(&mut **transaction)
    .await?;
    let row = template_row(transaction, organization_id, id)
        .await?
        .ok_or(AppError::Internal("project template vanished"))?;
    Ok(ProjectTemplateOutcome::Done(
        Box::new(template_view(row, true, true)),
        false,
    ))
}

async fn template_row(
    connection: &mut PgConnection,
    organization_id: Uuid,
    template_id: Uuid,
) -> Result<Option<ProjectTemplateRow>, AppError> {
    Ok(sqlx::query_as::<_, ProjectTemplateRow>(&format!(
        "{PROJECT_TEMPLATE_SELECT} WHERE template.organization_id = $1 AND template.id = $2"
    ))
    .bind(organization_id)
    .bind(template_id)
    .fetch_optional(&mut *connection)
    .await?)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateProjectTemplateRequest {
    expected_version: i64,
    name: Option<String>,
    description: Option<String>,
    archived: Option<bool>,
}

pub(super) async fn update_project_template(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((organization_id, template_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateProjectTemplateRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let mut transaction = state.pool.begin().await?;
    let outcome = change_project_template(
        &mut transaction,
        TemplateReader::browser(&principal),
        organization_id,
        template_id,
        TemplateChange {
            expected_version: request.expected_version,
            name: request.name.as_deref(),
            description: request.description.as_deref(),
            archived: request.archived,
        },
    )
    .await?;
    if let ProjectTemplateOutcome::Done(_, archived_now) = &outcome {
        settings::audit(
            &mut transaction,
            &headers,
            &principal,
            None,
            if *archived_now {
                "project_template.archived"
            } else {
                "project_template.updated"
            },
            "project_template",
            Some(template_id),
            json!({ "organizationId": organization_id }),
        )
        .await?;
        transaction.commit().await?;
    }
    Ok(outcome_response(outcome, StatusCode::OK))
}

/// A change to a project template: what's left out stays as it is.
pub(crate) struct TemplateChange<'a> {
    pub expected_version: i64,
    pub name: Option<&'a str>,
    pub description: Option<&'a str>,
    pub archived: Option<bool>,
}

/// Rename, describe, archive or restore a template, for whoever made it or
/// someone who manages the workspace's settings. Someone outside the
/// workspace, or a template that isn't there, is "not found"; the caller
/// records the audit row.
pub(crate) async fn change_project_template(
    transaction: &mut Transaction<'_, Postgres>,
    reader: TemplateReader,
    organization_id: Uuid,
    template_id: Uuid,
    change: TemplateChange<'_>,
) -> Result<ProjectTemplateOutcome, AppError> {
    let role = workspace_role(transaction, reader.actor_id, organization_id)
        .await?
        .ok_or(AppError::NotFound("workspace"))?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("project-templates:{organization_id}"))
        .execute(&mut **transaction)
        .await?;
    let row = template_row(transaction, organization_id, template_id)
        .await?
        .ok_or(AppError::NotFound("project_template"))?;
    if !may_edit(reader, &role, &row) {
        return Err(AppError::Forbidden);
    }
    if row.version != change.expected_version {
        return Err(AppError::VersionConflict {
            resource: "project_template",
            current_version: row.version,
        });
    }
    let name = match change.name {
        Some(value) => match normalize_template_name(value) {
            Some(name) => name,
            None => {
                return Ok(ProjectTemplateOutcome::Invalid(
                    "name",
                    "Give the template a name of 1 to 80 characters.",
                ));
            }
        },
        None => row.name.clone(),
    };
    let description = match change.description {
        Some(value) => match normalize_description(value) {
            Some(description) => description,
            None => {
                return Ok(ProjectTemplateOutcome::Invalid(
                    "description",
                    "Keep the description to 500 characters.",
                ));
            }
        },
        None => row.description.clone(),
    };
    let archived = change.archived.unwrap_or(row.archived);
    if !archived {
        let taken = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM project_templates
                WHERE organization_id = $1 AND id <> $2 AND archived_at IS NULL
                  AND lower(name) = lower($3)
            )
            "#,
        )
        .bind(organization_id)
        .bind(template_id)
        .bind(&name)
        .fetch_one(&mut **transaction)
        .await?;
        if taken {
            return Ok(ProjectTemplateOutcome::NameTaken);
        }
    }
    sqlx::query(
        r#"
        UPDATE project_templates
        SET name = $3, description = $4,
            archived_at = CASE WHEN $5 THEN COALESCE(archived_at, now()) ELSE NULL END,
            version = version + 1, updated_at = now()
        WHERE organization_id = $1 AND id = $2
        "#,
    )
    .bind(organization_id)
    .bind(template_id)
    .bind(&name)
    .bind(&description)
    .bind(archived)
    .execute(&mut **transaction)
    .await?;
    let archived_now = archived && !row.archived;
    let row = template_row(transaction, organization_id, template_id)
        .await?
        .ok_or(AppError::Internal("project template vanished"))?;
    let visible = match row.source_project_id {
        Some(source) => sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM project_memberships WHERE project_id = $1 AND actor_id = $2 AND enabled)",
        )
        .bind(source)
        .bind(reader.actor_id)
        .fetch_one(&mut **transaction)
        .await?,
        None => false,
    };
    Ok(ProjectTemplateOutcome::Done(
        Box::new(template_view(row, true, visible)),
        archived_now,
    ))
}

/// What applying a template set up, for the audit row.
#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AppliedTemplate {
    pub template_id: Uuid,
    pub statuses: usize,
    pub labels: usize,
    pub channels: usize,
    pub settings: usize,
    pub task_templates: usize,
}

/// Sets up a project made a moment ago in this transaction from a template
/// in its workspace. The caller created the project and is its owner.
/// Pieces that no longer fit (a label limit, a setting since removed) are
/// left out rather than failing the project.
pub(crate) async fn apply_to_new_project(
    transaction: &mut Transaction<'_, Postgres>,
    actor: CommandActor,
    organization_id: Uuid,
    template_id: Uuid,
) -> Result<AppliedTemplate, AppError> {
    let project_id = actor.project_id;
    let actor_id = actor.actor_id;
    let content = sqlx::query_scalar::<_, Value>(
        r#"
        SELECT content FROM project_templates
        WHERE organization_id = $1 AND id = $2 AND archived_at IS NULL
        "#,
    )
    .bind(organization_id)
    .bind(template_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::NotFound("project_template"))?;
    let content: TemplateContent = serde_json::from_value(content)
        .map_err(|_| AppError::Internal("stored project template"))?;
    let mut applied = AppliedTemplate {
        template_id,
        ..AppliedTemplate::default()
    };

    if let Some(workflow) = &content.workflow {
        let document = workflow_document(&mut *transaction, project_id, workflow).await?;
        match replace_workflow(transaction, &actor, &document).await? {
            Ok(_) => applied.statuses = workflow.statuses.len(),
            Err(WorkflowRefusal::Invalid { .. } | WorkflowRefusal::VersionConflict { .. }) => {
                // An older template may not fit today's rules; the project
                // keeps the default workflow.
            }
        }
    }

    for label in &content.labels {
        let Some(name) = work_item_labels::normalize_name(&label.name) else {
            continue;
        };
        let color = work_item_labels::normalize_color(&label.color);
        let description =
            work_item_labels::normalize_label_description(&label.description).unwrap_or_default();
        let outcome = work_item_labels::create_label(
            &mut *transaction,
            project_id,
            actor_id,
            NewLabel {
                name: &name,
                color: color.as_deref(),
                description: &description,
            },
        )
        .await?;
        if matches!(outcome, work_item_labels::CreateLabelOutcome::Created(_)) {
            applied.labels += 1;
        }
    }

    let mut channel_names: Vec<String> = Vec::new();
    for channel in content.channels.iter().take(MAX_CHANNELS as usize) {
        let Ok(name) = crate::conversations::normalize_conversation_name(channel) else {
            continue;
        };
        if channel_names
            .iter()
            .any(|kept| kept.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        crate::conversations::insert_channel(transaction, project_id, actor_id, &name).await?;
        channel_names.push(name);
    }
    applied.channels = channel_names.len();

    for (key, value) in &content.settings {
        if !copyable_setting(key, value) {
            continue;
        }
        settings::set(
            transaction,
            Target::Project {
                organization_id,
                project_id,
            },
            key,
            value.clone(),
            None,
            actor_id,
        )
        .await?;
        applied.settings += 1;
    }

    let types = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id, name FROM work_item_types WHERE project_id = $1 AND enabled",
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let mut template_names: Vec<String> = Vec::new();
    for template in content.task_templates.iter().take(200) {
        let type_id = template.type_name.as_deref().and_then(|wanted| {
            types
                .iter()
                .find(|(_, name)| name.eq_ignore_ascii_case(wanted))
                .map(|(id, _)| *id)
        });
        let Ok(fields) = normalize_template(TemplateFields {
            name: template.name.clone(),
            title: template.title.clone(),
            description: template.description.clone(),
            type_id,
            priority: template.priority.clone(),
            labels: template.labels.clone(),
            acceptance_criteria: template.acceptance_criteria.clone(),
            subtasks: template.subtasks.clone(),
            estimate: template.estimate,
        }) else {
            continue;
        };
        if template_names
            .iter()
            .any(|kept| kept.eq_ignore_ascii_case(&fields.name))
        {
            continue;
        }
        insert_template(&mut *transaction, project_id, actor_id, &fields).await?;
        template_names.push(fields.name);
    }
    applied.task_templates = template_names.len();
    Ok(applied)
}

/// The template's workflow as a document for the new project, keeping the
/// default statuses and types whose names match so nothing is retired that
/// the template still has.
async fn workflow_document(
    connection: &mut PgConnection,
    project_id: Uuid,
    workflow: &WorkflowContent,
) -> Result<WorkflowDocument, AppError> {
    let current = current_document(connection, project_id).await?;
    let statuses = workflow
        .statuses
        .iter()
        .enumerate()
        .map(|(index, status)| {
            let existing = current
                .statuses
                .iter()
                .find(|old| old.name.eq_ignore_ascii_case(&status.name))
                .and_then(|old| old.id);
            StatusDraft {
                id: existing,
                key: existing.is_none().then(|| format!("status-{index}")),
                name: status.name.clone(),
                semantic_category: status.semantic_category.clone(),
                is_default: status.is_default,
            }
        })
        .collect::<Vec<_>>();
    let reference = |name: &str| {
        statuses
            .iter()
            .find(|status| status.name.eq_ignore_ascii_case(name))
            .map(|status| match (status.id, &status.key) {
                (Some(id), _) => id.to_string(),
                (None, Some(key)) => key.clone(),
                (None, None) => status.name.clone(),
            })
    };
    let transitions = match &workflow.transitions {
        None => TransitionsDraft::All(crate::work_item_workflow::AllTransitions::All),
        Some(pairs) => TransitionsDraft::List(
            pairs
                .iter()
                .filter_map(|(from, to)| {
                    Some(TransitionDraft {
                        from: reference(from)?,
                        to: reference(to)?,
                    })
                })
                .collect(),
        ),
    };
    let types = workflow
        .types
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let existing = current
                .types
                .iter()
                .find(|old| old.name.eq_ignore_ascii_case(&kind.name))
                .and_then(|old| old.id);
            TypeDraft {
                id: existing,
                key: existing.is_none().then(|| format!("type-{index}")),
                name: kind.name.clone(),
                is_default: kind.is_default,
            }
        })
        .collect();
    Ok(WorkflowDocument {
        expected_version: current.expected_version,
        estimate_unit: Some(workflow.estimate_unit.clone()),
        statuses,
        types,
        transitions,
        replacements: Default::default(),
    })
}

/// The command actor for setting up a new project as its creator.
pub(crate) fn creator_actor(
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<CommandActor, AppError> {
    Ok(CommandActor {
        actor_id: principal.actor_id,
        actor_kind: super::actor_kind(&principal.actor_kind)?,
        project_id,
        audit: AuditAttribution::Browser {
            session_id: principal.session_id,
            request_id: None,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_settings_stay_behind() {
        assert!(!copyable_setting("no.such.setting", &json!(true)));
        let identifier = settings::all().iter().find(|definition| {
            matches!(definition.kind, SettingType::Identifier { .. })
                && definition.scopes.contains(&settings::Scope::Project)
        });
        if let Some(definition) = identifier {
            assert!(!copyable_setting(definition.key, &json!("anything")));
        }
    }

    #[test]
    fn descriptions_are_checked() {
        assert_eq!(normalize_description("  hi \n"), Some("hi".to_owned()));
        assert_eq!(normalize_description(&"x".repeat(501)), None);
        assert_eq!(normalize_description("a\u{7}b"), None);
    }
}
