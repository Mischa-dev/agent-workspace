use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header::LOCATION},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::ActorView,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, RequestHash, complete, reserve, resource_bound_scope,
    },
    message_content::{MessageContent, MessageContentError, MessageNode},
    outbox::{
        ConversationCreatedPayload, OutboxConversationKind, OutboxEventPayload, enqueue_typed,
    },
    policy::{Action, ActorKind, Decision, MembershipRole},
    work_item_commands::{
        AllocatedNumberMetadata, ArchiveCommand, ArchiveOutcome, AuditAttribution,
        AuditVersionMetadata, BoardPlacement, ChangeFinalization, CommandActor, ConflictKind,
        CreateWorkItemCommand, CreateWorkItemOutcome, CriteriaCommand, CriteriaOperation,
        CriteriaOutcome, CriteriaPlanError, CriterionTarget, LinkWording,
        MAX_BLOCKED_REASON_CHARACTERS, NewTaskFields, PlacementProblem, ProjectMembership,
        RepositoryLinkOutcome, TaskFieldChanges, ThreadLinkOutcome, UpdateWorkItemCommand,
        UpdateWorkItemOutcome, authorize_mutation, change_criteria, change_repository_link,
        change_thread_link, conflict_kind, create_work_item as execute_create_work_item,
        finalize_change, finalize_comment, insert_activity, lock_membership, lock_work_item,
        place_in_column, set_archived, update_work_item_by_id, valid_estimate,
    },
    work_item_labels::LabelDelta,
};

#[cfg(test)]
const MAX_DESCRIPTION_CHARACTERS: usize = crate::work_item_commands::MAX_DESCRIPTION_CHARACTERS;
const MAX_SEARCH_CHARACTERS: usize = 120;

const DEFAULT_TYPES: [(&str, i32, bool); 6] = [
    ("Feature", 1, false),
    ("Bug", 2, false),
    ("Task", 3, true),
    ("Request", 4, false),
    ("Research", 5, false),
    ("Improvement", 6, false),
];

const DEFAULT_STATUSES: [(&str, &str, i32, bool); 7] = [
    ("Backlog", "backlog", 1, true),
    ("Ready", "unstarted", 2, false),
    ("In Progress", "started", 3, false),
    ("Review", "review", 4, false),
    ("Verification", "verification", 5, false),
    ("Done", "completed", 6, false),
    ("Canceled", "canceled", 7, false),
];

const DEFAULT_TRANSITIONS: [(&str, &str); 17] = [
    ("Backlog", "Ready"),
    ("Backlog", "Canceled"),
    ("Ready", "Backlog"),
    ("Ready", "In Progress"),
    ("Ready", "Canceled"),
    ("In Progress", "Ready"),
    ("In Progress", "Review"),
    ("In Progress", "Canceled"),
    ("Review", "In Progress"),
    ("Review", "Verification"),
    ("Review", "Done"),
    ("Review", "Canceled"),
    ("Verification", "In Progress"),
    ("Verification", "Done"),
    ("Verification", "Canceled"),
    ("Done", "In Progress"),
    ("Canceled", "Backlog"),
];

pub(crate) mod import_parse;
pub(crate) mod imports;
mod list;
mod planning;
pub(crate) mod project_templates;
pub(crate) mod templates;
pub(crate) mod views;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/work-items",
            get(list::list_work_items).post(create_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/board",
            get(list::get_work_item_board),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/groups",
            get(list::list_work_item_groups),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}",
            get(get_work_item).patch(update_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/archive",
            axum::routing::post(archive_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/move",
            axum::routing::post(move_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/work-with-worker",
            axum::routing::post(work_with_worker),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/restore",
            axum::routing::post(restore_work_item),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/by-key/{key}",
            get(get_work_item_by_key),
        )
        .route(
            "/api/v1/projects/{project_id}/work-item-assignees",
            get(search_assignable_actors),
        )
        .route(
            "/api/v1/projects/{project_id}/work-item-references",
            get(search_work_item_references),
        )
        .route(
            "/api/v1/projects/{project_id}/work-item-workflow",
            get(planning::get_workflow).put(planning::replace_workflow),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/bulk",
            axum::routing::post(planning::bulk_update),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/dependencies",
            axum::routing::post(planning::change_dependencies),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/relations",
            axum::routing::post(planning::change_relation),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/watch",
            axum::routing::post(planning::watch),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/release-claim",
            axum::routing::post(planning::release_claim),
        )
        .route(
            "/api/v1/projects/{project_id}/labels",
            get(planning::list_labels).post(planning::create_label),
        )
        .route(
            "/api/v1/projects/{project_id}/labels/{label_id}",
            axum::routing::patch(planning::update_label),
        )
        .route(
            "/api/v1/projects/{project_id}/milestones",
            get(planning::list_milestones).post(planning::create_milestone),
        )
        .route(
            "/api/v1/projects/{project_id}/milestones/{milestone_id}",
            axum::routing::patch(planning::update_milestone),
        )
        .route(
            "/api/v1/projects/{project_id}/views",
            get(views::list_views).post(views::create_view),
        )
        .route(
            "/api/v1/projects/{project_id}/views/{view_id}",
            axum::routing::patch(views::update_view).delete(views::delete_view),
        )
        .route(
            "/api/v1/projects/{project_id}/views/{view_id}/pin",
            axum::routing::post(views::pin_view),
        )
        .route(
            "/api/v1/projects/{project_id}/task-templates",
            get(templates::get_templates).post(templates::create_template),
        )
        .route(
            "/api/v1/projects/{project_id}/task-templates/{template_id}",
            axum::routing::patch(templates::update_template),
        )
        .route(
            "/api/v1/projects/{project_id}/task-templates/{template_id}/apply",
            axum::routing::post(templates::apply_template_route),
        )
        .route(
            "/api/v1/projects/{project_id}/imports",
            get(imports::list_imports).post(imports::create_import).layer(
                axum::extract::DefaultBodyLimit::max(imports::MAX_IMPORT_BODY_BYTES),
            ),
        )
        .route(
            "/api/v1/projects/{project_id}/imports/{import_id}",
            get(imports::get_import).patch(imports::update_import),
        )
        .route(
            "/api/v1/projects/{project_id}/imports/{import_id}/commit",
            axum::routing::post(imports::commit_import),
        )
        .route(
            "/api/v1/projects/{project_id}/export",
            get(imports::export_project),
        )
        .route(
            "/api/v1/projects/{project_id}/save-as-template",
            axum::routing::post(project_templates::save_as_template),
        )
        .route(
            "/api/v1/organizations/{organization_id}/project-templates",
            get(project_templates::list_project_templates),
        )
        .route(
            "/api/v1/organizations/{organization_id}/project-templates/{template_id}",
            axum::routing::patch(project_templates::update_project_template),
        )
        .route(
            "/api/v1/projects/{project_id}/recurring-tasks",
            axum::routing::post(templates::create_recurring),
        )
        .route(
            "/api/v1/projects/{project_id}/recurring-tasks/{recurring_id}",
            axum::routing::patch(templates::update_recurring).delete(templates::delete_recurring),
        )
        .route("/api/v1/my-work", get(views::get_my_work))
        .route(
            "/api/v1/projects/{project_id}/timeline",
            get(views::get_timeline),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria",
            get(list_acceptance_criteria).post(create_acceptance_criterion),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/acceptance-criteria/{criterion_id}",
            axum::routing::patch(update_acceptance_criterion)
                .delete(remove_acceptance_criterion),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/timeline",
            get(get_work_item_timeline),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/comments",
            axum::routing::post(create_work_item_comment),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/threads/{conversation_id}",
            axum::routing::post(link_work_item_thread).delete(unlink_work_item_thread),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}",
            axum::routing::post(link_work_item_repository)
                .delete(unlink_work_item_repository),
        )
        .route(
            "/api/v1/projects/{project_id}/work-items/{work_item_id}/repositories/{repository_id}/primary",
            axum::routing::post(set_primary_work_item_repository),
        )
}

/// Reports whether the M4 workflow schema is present in the transaction's
/// current search path. Historical migration tests deliberately seed an M0-M3
/// snapshot before applying later migrations, so the development seed keeps
/// that upgrade path usable while provisioning the workflow on every current
/// schema.
pub async fn schema_is_available(
    transaction: &mut Transaction<'_, Postgres>,
) -> Result<bool, AppError> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT to_regclass('work_item_types') IS NOT NULL")
            .fetch_one(&mut **transaction)
            .await?,
    )
}

#[derive(Debug, FromRow)]
struct ProjectMembershipRow {
    project_id: Uuid,
    project_archived_at: Option<String>,
    role: String,
    membership_enabled: bool,
}

#[derive(Clone, Debug, FromRow)]
struct WorkflowStatusRow {
    id: Uuid,
    name: String,
    semantic_category: String,
}

#[derive(Debug, FromRow)]
struct WorkItemRow {
    id: Uuid,
    project_id: Uuid,
    project_key: String,
    number: i64,
    title: String,
    description: String,
    type_id: Uuid,
    type_name: String,
    status_id: Uuid,
    status_name: String,
    semantic_category: String,
    priority: String,
    assignee_actor_id: Option<Uuid>,
    assignee_display_name: Option<String>,
    assignee_handle: Option<String>,
    assignee_kind: Option<String>,
    assignee_active: Option<bool>,
    version: i64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    labels: serde_json::Value,
    acceptance_criteria: serde_json::Value,
    linked_threads: serde_json::Value,
    linked_repositories: serde_json::Value,
    linked_pull_requests: serde_json::Value,
    parent: Option<serde_json::Value>,
    milestone: Option<serde_json::Value>,
    start_date: Option<String>,
    due_date: Option<String>,
    estimate: Option<f64>,
    blocked: Option<serde_json::Value>,
    blocked_by: serde_json::Value,
    blocks: serde_json::Value,
    relations: serde_json::Value,
    subtasks: serde_json::Value,
    claim: Option<serde_json::Value>,
    watchers: serde_json::Value,
    watching: bool,
    estimate_unit: String,
    ready: bool,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateWorkItemRequest {
    title: String,
    #[serde(default)]
    description: String,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    #[serde(default = "default_priority")]
    priority: String,
    assignee_actor_id: Option<Uuid>,
    parent_work_item_id: Option<Uuid>,
    milestone_id: Option<Uuid>,
    #[serde(default)]
    label_ids: Vec<Uuid>,
    start_date: Option<String>,
    due_date: Option<String>,
    estimate: Option<f64>,
    #[serde(default)]
    acceptance_criteria: Vec<String>,
    /// W21: made from this message, which is linked back to (it must be one
    /// in this project the creator can see).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source_message_id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateWorkItemRequest {
    expected_version: i64,
    title: Option<String>,
    description: Option<String>,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    priority: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    assignee_actor_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    parent_work_item_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    milestone_id: Option<Option<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    start_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    due_date: Option<Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    estimate: Option<Option<f64>>,
    #[serde(default, deserialize_with = "deserialize_optional_field")]
    blocked_reason: Option<Option<String>>,
    #[serde(default)]
    add_label_ids: Vec<Uuid>,
    #[serde(default)]
    remove_label_ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkItemVersionRequest {
    expected_version: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssigneeSearchQuery {
    q: Option<String>,
    limit: Option<u16>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkItemReferenceSearchQuery {
    q: Option<String>,
    limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemView {
    id: Uuid,
    project_id: Uuid,
    key: String,
    number: i64,
    title: String,
    description: String,
    item_type: WorkItemTypeView,
    status: WorkItemStatusView,
    priority: String,
    assignee: Option<AssigneeView>,
    version: i64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    /// Active labels, by name. Replays stored before labels existed have
    /// none.
    #[serde(default)]
    labels: Vec<WorkItemLabelView>,
    acceptance_criteria: Vec<AcceptanceCriterionView>,
    linked_threads: Vec<LinkedThreadView>,
    linked_repositories: Vec<LinkedRepositoryView>,
    linked_pull_requests: Vec<LinkedPullRequestView>,
    capabilities: WorkItemCapabilities,
    /// Planning fields. Replays stored before they existed have none.
    #[serde(flatten)]
    planning: TaskPlanningView,
}

/// A task's place in the plan: parent, milestone, dates, estimate,
/// dependencies, links, subtasks, claim and watchers. Other tasks appear as
/// `TaskRef`s and only when the viewer can see their project.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
struct TaskPlanningView {
    parent: Option<serde_json::Value>,
    milestone: Option<serde_json::Value>,
    start_date: Option<String>,
    due_date: Option<String>,
    estimate: Option<f64>,
    blocked: Option<serde_json::Value>,
    blocked_by: Vec<serde_json::Value>,
    blocks: Vec<serde_json::Value>,
    ready: bool,
    relations: Vec<serde_json::Value>,
    subtasks: Vec<serde_json::Value>,
    claim: Option<serde_json::Value>,
    watchers: Vec<serde_json::Value>,
    watching: bool,
    estimate_unit: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemLabelView {
    id: Uuid,
    name: String,
    color: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedThreadView {
    id: Uuid,
    name: String,
    parent_channel_id: Uuid,
    parent_channel_name: String,
    archived_at: Option<String>,
    newest_run_state: Option<String>,
    href: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedRepositoryView {
    id: Uuid,
    owner: String,
    name: String,
    clone_url: String,
    role: String,
    preparation_state: String,
    trusted: bool,
    available: bool,
    archived_at: Option<String>,
    href: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct LinkedPullRequestView {
    id: Uuid,
    number: i64,
    title: String,
    url: String,
    state: String,
    draft: bool,
    source: String,
    confirmed: bool,
    checks_state: String,
    review_state: String,
    mergeability_state: String,
    sync_state: String,
    sync_failure_category: Option<String>,
    automation_state: String,
    automation_reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkRepositoryRequest {
    expected_version: i64,
    role: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EmptyLinkRequest {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AcceptanceCriterionView {
    id: Uuid,
    text: String,
    position: i32,
    completed: bool,
    completed_by: Option<ActorView>,
    completed_at: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateCriterionRequest {
    expected_version: i64,
    text: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCriterionRequest {
    expected_version: i64,
    text: Option<String>,
    position: Option<i32>,
    completed: Option<bool>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RemoveCriterionRequest {
    expected_version: i64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateCommentRequest {
    content: serde_json::Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemReferenceView {
    id: Uuid,
    key: String,
    title: String,
    status_category: String,
    archived_at: Option<String>,
    href: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemReferenceSearchResponse {
    items: Vec<WorkItemReferenceView>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemCommentView {
    id: Uuid,
    author: ActorView,
    content: serde_json::Value,
    plain_text: String,
    mentioned_actors: Vec<ActorView>,
    referenced_work_items: Vec<WorkItemReferenceView>,
    created_at: String,
    /// The connected app it was written through, when the author shows it.
    /// Stored comment replays predate it, so it has a default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    via: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct CommentAppendResponse {
    work_item_id: Uuid,
    sequence: i64,
    comment: WorkItemCommentView,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkItemTimelineQuery {
    cursor: Option<String>,
    limit: Option<u16>,
}

/// One entry of a task's timeline: an activity or a comment, with who made
/// it and the connected app they made it through, if they show it.
#[derive(Clone, Debug, FromRow)]
pub(crate) struct WorkItemTimelineRow {
    pub(crate) sequence: i64,
    /// "activity" or "comment".
    pub(crate) kind: String,
    pub(crate) created_at: String,
    pub(crate) activity_id: Option<Uuid>,
    pub(crate) activity_kind: Option<String>,
    pub(crate) activity_summary: Option<String>,
    pub(crate) activity_payload: Option<serde_json::Value>,
    pub(crate) comment_id: Option<Uuid>,
    pub(crate) comment_content: Option<serde_json::Value>,
    pub(crate) comment_plain_text: Option<String>,
    pub(crate) actor_id: Uuid,
    pub(crate) actor_kind: String,
    pub(crate) actor_display_name: String,
    pub(crate) actor_handle: String,
    /// The connected app's name, when the person shows it (migration 0042).
    pub(crate) via: Option<String>,
}

/// A page of a task's timeline, newest first.
pub(crate) struct TimelinePage {
    pub(crate) rows: Vec<WorkItemTimelineRow>,
    /// Whether there are entries before the oldest row here.
    pub(crate) has_more: bool,
}

#[derive(Debug, FromRow)]
struct WorkItemReferenceRow {
    id: Uuid,
    key: String,
    title: String,
    status_category: String,
    archived_at: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemTimelineResponse {
    entries: Vec<WorkItemTimelineEntryView>,
    next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemTimelineEntryView {
    sequence: i64,
    kind: String,
    created_at: String,
    activity: Option<WorkItemActivityView>,
    comment: Option<WorkItemCommentView>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemActivityView {
    id: Uuid,
    actor: ActorView,
    kind: String,
    summary: String,
    payload: serde_json::Value,
    /// The connected app it was done through, when the actor shows it.
    #[serde(skip_serializing_if = "Option::is_none")]
    via: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemTypeView {
    id: Uuid,
    name: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemStatusView {
    id: Uuid,
    name: String,
    semantic_category: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssigneeView {
    actor_id: Uuid,
    display_name: String,
    handle: String,
    kind: String,
    active: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct AssigneeSearchResponse {
    actors: Vec<ActorView>,
}

#[derive(Debug, FromRow)]
struct AssigneeSearchRow {
    actor_id: Uuid,
    display_name: String,
    handle: String,
    kind: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemCapabilities {
    view: bool,
    edit: bool,
    archive: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct WorkWithWorkerRequest {
    expected_version: i64,
    channel_id: Uuid,
    prompt: String,
    repository_id: Option<Uuid>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkWithWorkerResponse {
    thread_id: Uuid,
    run: crate::api::StartRunResponse,
}

#[derive(Debug, FromRow)]
struct WorkerInvocationRow {
    conversation_id: Uuid,
    repository_id: Uuid,
    request_hash: Vec<u8>,
    response_body: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MoveWorkItemRequest {
    expected_version: i64,
    target_status_id: Uuid,
    /// 1-based among the column's open tasks. Give exactly one of this,
    /// `before_id` or `after_id`; anchors stay right on a filtered board.
    target_position: Option<i32>,
    before_id: Option<Uuid>,
    after_id: Option<Uuid>,
}

#[derive(Debug, Serialize)]
struct WorkItemConflictEnvelope {
    error: WorkItemConflictBody,
}

#[derive(Debug, Serialize)]
struct WorkItemConflictBody {
    code: &'static str,
    message: &'static str,
    details: WorkItemConflictDetails,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkItemConflictDetails {
    resource: &'static str,
    current_version: i64,
    current: WorkItemView,
}

pub async fn provision_missing_default_workflows(pool: &PgPool) -> Result<u64, AppError> {
    let project_ids = sqlx::query_scalar::<_, Uuid>("SELECT id FROM projects ORDER BY id")
        .fetch_all(pool)
        .await?;
    let mut provisioned = 0;
    for project_id in project_ids {
        let mut transaction = pool.begin().await?;
        if ensure_default_workflow(&mut transaction, project_id).await? {
            provisioned += 1;
        }
        transaction.commit().await?;
    }
    Ok(provisioned)
}

pub async fn ensure_default_workflow(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
) -> Result<bool, AppError> {
    sqlx::query("SELECT id FROM projects WHERE id = $1 FOR UPDATE")
        .bind(project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or(AppError::NotFound("project"))?;

    let types =
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM work_item_types WHERE project_id = $1")
            .bind(project_id)
            .fetch_all(&mut **transaction)
            .await?;
    let statuses = sqlx::query_scalar::<_, bool>(
        "SELECT enabled FROM work_item_statuses WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_all(&mut **transaction)
    .await?;
    let transition_count = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM work_item_status_transitions WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut **transaction)
    .await?;

    if types.is_empty() && statuses.is_empty() && transition_count == 0 {
        let mut status_ids = std::collections::BTreeMap::new();
        for (name, position, is_default) in DEFAULT_TYPES {
            sqlx::query(
                "INSERT INTO work_item_types (id, project_id, name, position, is_default) VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(Uuid::now_v7())
            .bind(project_id)
            .bind(name)
            .bind(position)
            .bind(is_default)
            .execute(&mut **transaction)
            .await?;
        }
        for (name, category, position, is_default) in DEFAULT_STATUSES {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO work_item_statuses (id, project_id, name, semantic_category, position, is_default) VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(id)
            .bind(project_id)
            .bind(name)
            .bind(category)
            .bind(position)
            .bind(is_default)
            .execute(&mut **transaction)
            .await?;
            status_ids.insert(name, id);
        }
        for (from, to) in DEFAULT_TRANSITIONS {
            sqlx::query(
                "INSERT INTO work_item_status_transitions (project_id, from_status_id, to_status_id) VALUES ($1, $2, $3)",
            )
            .bind(project_id)
            .bind(status_ids[from])
            .bind(status_ids[to])
            .execute(&mut **transaction)
            .await?;
        }
        return Ok(true);
    }

    // A project that has a workflow keeps it: owners change statuses, types
    // and transitions (work_item_workflow), so it needn't match the default.
    if !types.contains(&true) || !statuses.contains(&true) {
        return Err(AppError::Conflict(
            "work_item_workflow",
            "the project's workflow has no active status or type",
        ));
    }
    Ok(false)
}

async fn validate_list_filter_ids(
    pool: &PgPool,
    project_id: Uuid,
    type_id: Option<Uuid>,
    status_id: Option<Uuid>,
    assignee_actor_id: Option<Uuid>,
) -> Result<(), AppError> {
    if let Some(type_id) = type_id {
        let belongs_to_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_types WHERE project_id = $1 AND id = $2)",
        )
        .bind(project_id)
        .bind(type_id)
        .fetch_one(pool)
        .await?;
        if !belongs_to_project {
            return Err(AppError::Validation("typeId"));
        }
    }
    if let Some(status_id) = status_id {
        let belongs_to_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_statuses WHERE project_id = $1 AND id = $2)",
        )
        .bind(project_id)
        .bind(status_id)
        .fetch_one(pool)
        .await?;
        if !belongs_to_project {
            return Err(AppError::Validation("statusId"));
        }
    }
    if let Some(assignee_actor_id) = assignee_actor_id {
        let belongs_to_project = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM project_memberships WHERE project_id = $1 AND actor_id = $2)",
        )
        .bind(project_id)
        .bind(assignee_actor_id)
        .fetch_one(pool)
        .await?;
        if !belongs_to_project {
            return Err(AppError::Validation("assigneeActorId"));
        }
    }
    Ok(())
}

async fn get_work_item(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<WorkItemView>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemView,
        Some(work_item_id),
    )?;
    let row = fetch_work_item(&state.pool, project_id, work_item_id, principal.actor_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    Ok(Json(work_item_view(row, capabilities(&membership)?)?))
}

async fn get_work_item_by_key(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, key)): ApiPath<(Uuid, String)>,
) -> Result<Json<WorkItemView>, AppError> {
    validate_readable_key(&key)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let work_item_id = sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT item.id
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = $1
          AND project.project_key || '-' || item.number::text = $2
        "#,
    )
    .bind(project_id)
    .bind(key)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    let row = fetch_work_item(&state.pool, project_id, work_item_id, principal.actor_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    Ok(Json(work_item_view(row, capabilities(&membership)?)?))
}

async fn update_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<UpdateWorkItemRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let fields = planning_field_changes(&request)?;
    let labels_change = !request.add_label_ids.is_empty() || !request.remove_label_ids.is_empty();
    if request.expected_version < 1
        || (request.title.is_none()
            && request.description.is_none()
            && request.type_id.is_none()
            && request.status_id.is_none()
            && request.priority.is_none()
            && request.assignee_actor_id.is_none()
            && request.parent_work_item_id.is_none()
            && fields.is_empty()
            && !labels_change)
    {
        return Err(AppError::Validation("Work Item update"));
    }
    if request.add_label_ids.len() + request.remove_label_ids.len() > MAX_LABEL_CHANGES
        || request
            .add_label_ids
            .iter()
            .any(|id| request.remove_label_ids.contains(id))
    {
        return Err(AppError::Validation("labelIds"));
    }
    request.title = request.title.as_deref().map(normalize_title).transpose()?;
    request.description = request
        .description
        .as_deref()
        .map(normalize_description)
        .transpose()?;
    if let Some(priority) = request.priority.as_deref() {
        validate_priority(priority)?;
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "update")?;
    let request_hash = RequestHash::for_value(&serde_json::json!({
        "expectedVersion": request.expected_version,
        "title": request.title,
        "description": request.description,
        "typeId": request.type_id,
        "statusId": request.status_id,
        "priority": request.priority,
        "assigneeSpecified": request.assignee_actor_id.is_some(),
        "assigneeActorId": request.assignee_actor_id.flatten(),
        "parentWorkItemId": request.parent_work_item_id,
        "milestoneId": request.milestone_id,
        "startDate": request.start_date,
        "dueDate": request.due_date,
        "estimate": request.estimate,
        "blockedReason": request.blocked_reason,
        "addLabelIds": request.add_label_ids,
        "removeLabelIds": request.remove_label_ids,
    }))?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let capabilities = capabilities(&membership)?;
    if labels_change {
        let all = [
            request.add_label_ids.as_slice(),
            request.remove_label_ids.as_slice(),
        ]
        .concat();
        if !labels_are_active(&mut transaction, project_id, &all).await? {
            return Err(AppError::Validation("labelIds"));
        }
    }
    let outcome = update_work_item_by_id(
        &mut transaction,
        &browser_actor(&headers, &principal, project_id)?,
        work_item_id,
        UpdateWorkItemCommand {
            expected_version: request.expected_version,
            title: request.title.as_deref(),
            description: request.description.as_deref(),
            type_id: request.type_id,
            status_id: request.status_id,
            priority: request.priority.as_deref(),
            assignee_actor_id: request.assignee_actor_id,
            parent_work_item_id: request.parent_work_item_id,
            board_placement: None,
            labels: labels_change.then_some(LabelDelta {
                add: &request.add_label_ids,
                remove: &request.remove_label_ids,
            }),
            fields,
            summary: "Work Item updated",
            audit_action: "work_item.updated",
            audit_version_metadata: AuditVersionMetadata::Omit,
        },
    )
    .await?;
    match outcome {
        UpdateWorkItemOutcome::NotFound => return Err(AppError::NotFound("work_item")),
        UpdateWorkItemOutcome::InvalidAssignee => {
            return Err(AppError::Validation("assigneeActorId"));
        }
        UpdateWorkItemOutcome::InvalidMilestone => {
            return Err(AppError::Validation("milestoneId"));
        }
        UpdateWorkItemOutcome::InvalidDates => return Err(AppError::Validation("dueDate")),
        UpdateWorkItemOutcome::InvalidTransition { .. } => {
            return Err(AppError::Conflict("work_item_status", "transition"));
        }
        UpdateWorkItemOutcome::Conflict(current) => {
            if current.version == request.expected_version && current.archived {
                return Err(AppError::Conflict("work_item", "archived"));
            }
            let row = fetch_work_item_in_transaction(
                &mut transaction,
                project_id,
                work_item_id,
                principal.actor_id,
            )
            .await?
            .ok_or(AppError::NotFound("work_item"))?;
            return work_item_version_conflict_response(work_item_view(row, capabilities)?);
        }
        UpdateWorkItemOutcome::TooManyLabels { .. } => {
            return Err(AppError::Validation("addLabelIds"));
        }
        UpdateWorkItemOutcome::InvalidParent(_) => {
            return Err(AppError::Validation("parentWorkItemId"));
        }
        UpdateWorkItemOutcome::InvalidPlacement(_) => {
            return Err(AppError::Internal("the web update changed position"));
        }
        UpdateWorkItemOutcome::Noop(_) | UpdateWorkItemOutcome::Applied { .. } => {}
    }
    let row = fetch_work_item_in_transaction(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::Internal("updated Work Item is unavailable"))?;
    let item = work_item_view(row, capabilities)?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn archive_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    set_work_item_archived(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        request,
        true,
    )
    .await
}

async fn restore_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    set_work_item_archived(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        request,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn set_work_item_archived(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    request: WorkItemVersionRequest,
    archived: bool,
) -> Result<Response, AppError> {
    require_authenticated_write(state, headers, principal)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let idempotency_key = IdempotencyKey::from_headers(headers)?;
    let action = if archived { "archive" } else { "restore" };
    let scope = resource_bound_scope("work_items", work_item_id, action)?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &idempotency_key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let capabilities = capabilities(&membership)?;
    let (summary, audit_action) = if archived {
        ("Work Item archived", "work_item.archived")
    } else {
        ("Work Item restored", "work_item.restored")
    };
    let outcome = set_archived(
        &mut transaction,
        &browser_actor(headers, principal, project_id)?,
        work_item_id,
        ArchiveCommand {
            expected_version: request.expected_version,
            archived,
            summary,
            audit_action,
        },
    )
    .await?;
    match outcome {
        ArchiveOutcome::NotFound => return Err(AppError::NotFound("work_item")),
        // Restoring the original of a moved task would make a live copy
        // whose old key opens the moved one.
        ArchiveOutcome::MovedAway { .. } => return Err(AppError::Conflict("work_item", "moved")),
        ArchiveOutcome::VersionConflict(_) => {
            let current = fetch_work_item_in_transaction(
                &mut transaction,
                project_id,
                work_item_id,
                principal.actor_id,
            )
            .await?
            .ok_or(AppError::NotFound("work_item"))?;
            return work_item_version_conflict_response(work_item_view(current, capabilities)?);
        }
        ArchiveOutcome::Unchanged(_) | ArchiveOutcome::Applied { .. } => {}
    }
    let row = fetch_work_item_in_transaction(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::Internal("archived Work Item is unavailable"))?;
    let item = work_item_view(row, capabilities)?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn link_work_item_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, conversation_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<EmptyLinkRequest>,
) -> Result<Response, AppError> {
    change_work_item_thread_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        conversation_id,
        request,
        true,
    )
    .await
}

async fn work_with_worker(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<WorkWithWorkerRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    request.prompt = normalize_worker_prompt(&request.prompt)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let idempotency_key = IdempotencyKey::from_headers(&headers)?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkerInvoke,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }

    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "{}:work_item_worker_invocation:{}:{}",
            principal.actor_id,
            work_item_id,
            idempotency_key.as_str()
        ))
        .execute(&mut *transaction)
        .await?;
    let existing = sqlx::query_as::<_, WorkerInvocationRow>(
        r#"
        SELECT conversation_id, repository_id, request_hash, response_body
        FROM work_item_worker_invocations
        WHERE requesting_actor_id = $1 AND work_item_id = $2 AND idempotency_key = $3
        FOR UPDATE
        "#,
    )
    .bind(principal.actor_id)
    .bind(work_item_id)
    .bind(idempotency_key.as_str())
    .fetch_optional(&mut *transaction)
    .await?;

    let (thread_id, repository_id, work_item_key) = if let Some(existing) = existing {
        if existing.request_hash.as_slice() != request_hash.as_bytes() {
            return Err(AppError::Conflict("idempotency_key", "payload mismatch"));
        }
        if let Some(response_body) = existing.response_body {
            let response: WorkWithWorkerResponse = serde_json::from_value(response_body)
                .map_err(|_| AppError::Internal("decode Work with Worker response"))?;
            transaction.commit().await?;
            return Ok((StatusCode::ACCEPTED, Json(response)).into_response());
        }
        let key = sqlx::query_scalar::<_, String>(
            "SELECT project.project_key || '-' || item.number::text FROM work_items item JOIN projects project ON project.id = item.project_id WHERE item.project_id = $1 AND item.id = $2",
        )
        .bind(project_id)
        .bind(work_item_id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
        transaction.commit().await?;
        (existing.conversation_id, existing.repository_id, key)
    } else {
        let current = fetch_work_item_for_update(
            &mut transaction,
            project_id,
            work_item_id,
            principal.actor_id,
        )
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
        if current.version != request.expected_version {
            let current = work_item_view(current, capabilities(&membership)?)?;
            return work_item_version_conflict_response(current);
        }
        if current.archived_at.is_some() {
            return Err(AppError::Conflict("work_item", "archived"));
        }
        let channel_exists = sqlx::query_scalar::<_, bool>(
            r#"
            SELECT EXISTS (
                SELECT 1 FROM conversations
                WHERE project_id = $1 AND id = $2 AND kind = 'channel' AND archived_at IS NULL
                  AND conversation_visible_to(id, $3)
            )
            "#,
        )
        .bind(project_id)
        .bind(request.channel_id)
        .bind(principal.actor_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !channel_exists {
            return Err(AppError::NotFound("channel"));
        }

        let thread_id = Uuid::now_v7();
        let work_item_key = format!("{}-{}", current.project_key, current.number);
        let thread_name = worker_thread_name(&work_item_key, &current.title, thread_id);
        sqlx::query(
            r#"
            INSERT INTO conversations (id, project_id, kind, parent_channel_id, name)
            VALUES ($1, $2, 'thread', $3, $4)
            "#,
        )
        .bind(thread_id)
        .bind(project_id)
        .bind(request.channel_id)
        .bind(&thread_name)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO conversation_read_states (
                project_id, conversation_id, actor_id, highest_seen_sequence
            ) VALUES ($1, $2, $3, 0)
            "#,
        )
        .bind(project_id)
        .bind(thread_id)
        .bind(principal.actor_id)
        .execute(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT INTO work_item_conversation_links (
                project_id, work_item_id, conversation_id, created_by_actor_id
            ) VALUES ($1, $2, $3, $4)
            "#,
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(thread_id)
        .bind(principal.actor_id)
        .execute(&mut *transaction)
        .await?;

        let setup = crate::api::select_run_setup(
            &mut transaction,
            project_id,
            thread_id,
            principal.actor_id,
            request.repository_id,
        )
        .await?;
        let version =
            advance_work_item_version(&mut transaction, project_id, work_item_id, current.version)
                .await?;
        enqueue_typed(
            &mut transaction,
            project_id,
            OutboxEventPayload::ConversationCreated(ConversationCreatedPayload {
                conversation_id: thread_id,
                kind: OutboxConversationKind::Thread,
                version: 1,
            }),
        )
        .await?;
        record_work_item_change(
            &mut transaction,
            &headers,
            &principal,
            project_id,
            work_item_id,
            "worker_invoked",
            "Work with Worker started",
            "work_item.worker_invoked",
            serde_json::json!({
                "conversationId": thread_id,
                "repositoryId": setup.repository_id,
            }),
            version,
        )
        .await?;
        sqlx::query(
            r#"
            INSERT INTO work_item_worker_invocations (
                id, project_id, work_item_id, conversation_id, requesting_actor_id,
                idempotency_key, request_hash, repository_id
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(Uuid::now_v7())
        .bind(project_id)
        .bind(work_item_id)
        .bind(thread_id)
        .bind(principal.actor_id)
        .bind(idempotency_key.as_str())
        .bind(request_hash.as_bytes().as_slice())
        .bind(setup.repository_id)
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        (thread_id, setup.repository_id, work_item_key)
    };

    let provider_prompt = format!(
        "Work Item {work_item_key} ({work_item_id}).\n\nUser instructions:\n{}",
        request.prompt
    );
    let (_, Json(run)) = crate::api::start_agent_run(
        State(state.clone()),
        headers,
        principal.clone(),
        ApiPath(thread_id),
        ApiJson(crate::api::StartRunRequest {
            prompt: provider_prompt,
            repository_id: Some(repository_id),
            config: None,
            provider_account_id: None,
            agent_profile_id: None,
        }),
    )
    .await?;
    let response = WorkWithWorkerResponse { thread_id, run };
    let response_body = serde_json::to_value(&response)
        .map_err(|_| AppError::Internal("serialize Work with Worker response"))?;
    let mut transaction = state.pool.begin().await?;
    let updated = sqlx::query(
        r#"
        UPDATE work_item_worker_invocations
        SET agent_run_id = $4, response_body = $5, updated_at = now()
        WHERE requesting_actor_id = $1 AND work_item_id = $2 AND idempotency_key = $3
          AND request_hash = $6
          AND (agent_run_id IS NULL OR agent_run_id = $4)
        "#,
    )
    .bind(principal.actor_id)
    .bind(work_item_id)
    .bind(idempotency_key.as_str())
    .bind(response.run.run_id)
    .bind(response_body)
    .bind(request_hash.as_bytes().as_slice())
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Conflict(
            "work_item_worker_invocation",
            "completion raced with a different run",
        ));
    }
    transaction.commit().await?;
    Ok((StatusCode::ACCEPTED, Json(response)).into_response())
}

fn normalize_worker_prompt(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > 18_000
        || value
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
    {
        return Err(AppError::Validation("worker prompt"));
    }
    Ok(value.to_owned())
}

pub(crate) fn worker_thread_name(key: &str, title: &str, thread_id: Uuid) -> String {
    let suffix = &thread_id.simple().to_string()[24..];
    let prefix = format!("{key}: ");
    let suffix = format!(" [{suffix}]");
    let available = 120usize.saturating_sub(prefix.chars().count() + suffix.chars().count());
    let title = title.chars().take(available).collect::<String>();
    format!("{prefix}{title}{suffix}")
}

async fn unlink_work_item_thread(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, conversation_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<EmptyLinkRequest>,
) -> Result<Response, AppError> {
    change_work_item_thread_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        conversation_id,
        request,
        false,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn change_work_item_thread_link(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    conversation_id: Uuid,
    request: EmptyLinkRequest,
    linked: bool,
) -> Result<Response, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let key = IdempotencyKey::from_headers(headers)?;
    let scope = resource_bound_scope(
        "work_items",
        work_item_id,
        if linked {
            "thread_link"
        } else {
            "thread_unlink"
        },
    )?;
    let request_hash = RequestHash::for_value(&(conversation_id, request))?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = lock_work_item(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    // An archived task takes no new threads, but the thread page offers
    // Unlink on every task linked to it, archived ones included.
    if current.archived && linked {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    match change_thread_link(
        &mut transaction,
        &browser_actor(headers, principal, project_id)?,
        &current,
        None,
        conversation_id,
        linked,
        LinkWording::WEB,
    )
    .await?
    {
        ThreadLinkOutcome::ThreadNotFound => return Err(AppError::NotFound("conversation")),
        ThreadLinkOutcome::VersionConflict => {
            return Err(AppError::Internal("thread link checked no version"));
        }
        ThreadLinkOutcome::Unchanged | ThreadLinkOutcome::Changed { .. } => {}
    }
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
        capabilities(&membership)?,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn link_work_item_repository(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, repository_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<LinkRepositoryRequest>,
) -> Result<Response, AppError> {
    if !matches!(request.role.as_str(), "primary" | "related") {
        return Err(AppError::Validation("repository role"));
    }
    change_work_item_repository_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        repository_id,
        request.expected_version,
        Some(request.role),
        "repository_link",
    )
    .await
}

async fn unlink_work_item_repository(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, repository_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    change_work_item_repository_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        repository_id,
        request.expected_version,
        None,
        "repository_unlink",
    )
    .await
}

async fn set_primary_work_item_repository(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, repository_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<WorkItemVersionRequest>,
) -> Result<Response, AppError> {
    change_work_item_repository_link(
        &state,
        &headers,
        &principal,
        project_id,
        work_item_id,
        repository_id,
        request.expected_version,
        Some("primary".to_owned()),
        "repository_primary",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn change_work_item_repository_link(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    repository_id: Uuid,
    expected_version: i64,
    desired_role: Option<String>,
    action: &'static str,
) -> Result<Response, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let key = IdempotencyKey::from_headers(headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, action)?;
    let request_hash = RequestHash::for_value(&(repository_id, expected_version, &desired_role))?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = lock_work_item(&mut transaction, project_id, work_item_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let caps = capabilities(&membership)?;
    if current.version != expected_version {
        let row = fetch_work_item_in_transaction(
            &mut transaction,
            project_id,
            work_item_id,
            principal.actor_id,
        )
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
        return work_item_version_conflict_response(work_item_view(row, caps)?);
    }
    if current.archived {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    match change_repository_link(
        &mut transaction,
        &browser_actor(headers, principal, project_id)?,
        &current,
        None,
        repository_id,
        desired_role.as_deref(),
        LinkWording::WEB,
    )
    .await?
    {
        RepositoryLinkOutcome::RepositoryNotFound => {
            return Err(AppError::NotFound("repository"));
        }
        RepositoryLinkOutcome::NotEligibleForPrimary { .. } => {
            return Err(AppError::Conflict(
                "repository",
                "the primary repository must be enabled, trusted, ready, and active",
            ));
        }
        RepositoryLinkOutcome::RepositoryArchived => {
            return Err(AppError::Conflict(
                "repository",
                "archived repositories can't be linked",
            ));
        }
        RepositoryLinkOutcome::VersionConflict => {
            return Err(AppError::Internal("repository link checked its version"));
        }
        RepositoryLinkOutcome::Unchanged | RepositoryLinkOutcome::Changed { .. } => {}
    }
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
        caps,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn search_assignable_actors(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<AssigneeSearchQuery>,
) -> Result<Json<AssigneeSearchResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::NotFound("project"));
    }
    let search = normalize_search(query.q.as_deref())?
        .unwrap_or_default()
        .to_lowercase();
    let limit = i64::from(query.limit.unwrap_or(20).clamp(1, 50));
    let rows = sqlx::query_as::<_, AssigneeSearchRow>(
        r#"
        SELECT membership.actor_id, membership.display_name, membership.handle, actor.kind
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.enabled
          AND actor.status = 'active'
          AND actor.kind IN ('human', 'external_agent')
          AND (
              $2 = ''
              OR strpos(lower(membership.display_name), $2) > 0
              OR strpos(lower(membership.handle), $2) > 0
          )
        ORDER BY lower(membership.display_name), lower(membership.handle), actor.id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(search)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(AssigneeSearchResponse {
        actors: rows
            .into_iter()
            .map(|row| ActorView {
                id: row.actor_id,
                display_name: row.display_name,
                handle: row.handle,
                kind: row.kind,
                avatar_url: None,
            })
            .collect(),
    }))
}

async fn search_work_item_references(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<WorkItemReferenceSearchQuery>,
) -> Result<Json<WorkItemReferenceSearchResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let search = normalize_search(query.q.as_deref())?.unwrap_or_default();
    let limit = i64::from(query.limit.unwrap_or(20).clamp(1, 50));
    let rows = sqlx::query_as::<_, WorkItemReferenceRow>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key,
               item.title, status.semantic_category AS status_category,
               item.archived_at::text AS archived_at
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1
          AND (
              $2 = '' OR item.title ILIKE '%' || $2 || '%'
              OR project.project_key || '-' || item.number::text ILIKE '%' || $2 || '%'
          )
        -- An exact key comes first, so AW-1 isn't crowded out by AW-10 to AW-199.
        ORDER BY upper(project.project_key || '-' || item.number::text) = upper($2) DESC,
                 item.archived_at NULLS FIRST, item.number DESC, item.id
        LIMIT $3
        "#,
    )
    .bind(project_id)
    .bind(search)
    .bind(limit)
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(WorkItemReferenceSearchResponse {
        items: rows
            .into_iter()
            .map(|row| work_item_reference_view(project_id, row))
            .collect(),
    }))
}

async fn move_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<MoveWorkItemRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let placement = match (request.target_position, request.before_id, request.after_id) {
        (Some(position), None, None) if (1..=10_000).contains(&position) => BoardPlacement::Index(
            usize::try_from(position - 1).map_err(|_| AppError::Validation("targetPosition"))?,
        ),
        (None, Some(anchor), None) => BoardPlacement::Before(anchor),
        (None, None, Some(anchor)) => BoardPlacement::After(anchor),
        _ => return Err(AppError::Validation("targetPosition")),
    };
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "move")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    let caps = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, caps)?);
    }
    if current.archived_at.is_some() {
        return Err(AppError::NotFound("work_item"));
    }
    let target_status = sqlx::query_as::<_, WorkflowStatusRow>(
        "SELECT id, name, semantic_category, position, is_default, enabled FROM work_item_statuses WHERE project_id = $1 AND id = $2 AND enabled",
    )
    .bind(project_id)
    .bind(request.target_status_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::Validation("targetStatusId"))?;
    if current.status_id != request.target_status_id {
        let allowed = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM work_item_status_transitions WHERE project_id = $1 AND from_status_id = $2 AND to_status_id = $3)",
        )
        .bind(project_id)
        .bind(current.status_id)
        .bind(request.target_status_id)
        .fetch_one(&mut *transaction)
        .await?;
        if !allowed {
            return Err(AppError::Conflict("work_item_status", "transition"));
        }
    }
    let current_rank = sqlx::query_scalar::<_, i64>(
        "SELECT board_rank FROM work_items WHERE project_id = $1 AND id = $2",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_one(&mut *transaction)
    .await?;
    let board_rank = match place_in_column(
        &mut transaction,
        project_id,
        request.target_status_id,
        work_item_id,
        Some((current.status_id, current_rank)),
        placement,
    )
    .await?
    {
        Ok(placed) => placed.board_rank,
        Err(PlacementProblem::OutOfRange { .. }) => {
            return Err(AppError::Validation("targetPosition"));
        }
        Err(PlacementProblem::AnchorIsSelf | PlacementProblem::AnchorNotInColumn { .. }) => {
            return Err(AppError::Validation(if request.before_id.is_some() {
                "beforeId"
            } else {
                "afterId"
            }));
        }
    };
    let updated = sqlx::query(
        r#"
        UPDATE work_items
        SET status_id = $3, board_rank = $4, version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $5
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(request.target_status_id)
    .bind(board_rank)
    .bind(request.expected_version)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(AppError::Internal("locked Work Item move lost its version"));
    }
    let version = request
        .expected_version
        .checked_add(1)
        .ok_or(AppError::Validation("expectedVersion"))?;
    record_work_item_change(
        &mut transaction,
        &headers,
        &principal,
        project_id,
        work_item_id,
        "work_item_updated",
        "Work Item moved",
        "work_item.moved",
        serde_json::json!({
            "previousStatusId": current.status_id,
            "statusId": target_status.id,
            "targetPosition": request.target_position,
            "beforeId": request.before_id,
            "afterId": request.after_id,
        }),
        version,
    )
    .await?;
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
        caps,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn create_work_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(mut request): ApiJson<CreateWorkItemRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    request.title = normalize_title(&request.title)?;
    request.description = normalize_description(&request.description)?;
    validate_priority(&request.priority)?;
    request.acceptance_criteria = request
        .acceptance_criteria
        .iter()
        .map(|text| normalize_criterion_text(text))
        .collect::<Result<_, _>>()?;
    if request.acceptance_criteria.len() > MAX_NEW_CRITERIA
        || request.label_ids.len() > MAX_LABEL_CHANGES
    {
        return Err(AppError::Validation("Work Item create"));
    }
    request.label_ids.sort();
    request.label_ids.dedup();
    let fields = NewTaskFields {
        milestone_id: request.milestone_id,
        start_date: parse_task_date(request.start_date.as_deref(), "startDate")?,
        due_date: parse_task_date(request.due_date.as_deref(), "dueDate")?,
        estimate: request.estimate,
    };
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("projects", project_id, "work_items_create")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemCreate, None)?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            work_item_response(replay.status(), item)
        }
        IdempotencyOutcome::Reserved(reservation) => {
            ensure_default_workflow(&mut transaction, project_id).await?;
            if !labels_are_active(&mut transaction, project_id, &request.label_ids).await? {
                return Err(AppError::Validation("labelIds"));
            }
            let outcome = execute_create_work_item(
                &mut transaction,
                &browser_actor(&headers, &principal, project_id)?,
                CreateWorkItemCommand {
                    title: &request.title,
                    description: &request.description,
                    type_id: request.type_id,
                    status_id: request.status_id,
                    priority: &request.priority,
                    assignee_actor_id: request.assignee_actor_id,
                    parent_work_item_id: request.parent_work_item_id,
                    label_ids: &request.label_ids,
                    acceptance_criteria: &request.acceptance_criteria,
                    board_placement: None,
                    fields,
                    activity_summary: "Work Item created",
                    activity_metadata: serde_json::json!({}),
                    activity_number_metadata: AllocatedNumberMetadata::Omit,
                    audit_action: "work_item.created",
                    audit_metadata: serde_json::json!({}),
                },
            )
            .await?;
            let work_item_id = match outcome {
                CreateWorkItemOutcome::Created { work_item_id, .. } => work_item_id,
                CreateWorkItemOutcome::InvalidAssignee => {
                    return Err(AppError::Validation("assigneeActorId"));
                }
                CreateWorkItemOutcome::InvalidMilestone => {
                    return Err(AppError::Validation("milestoneId"));
                }
                CreateWorkItemOutcome::InvalidDates => {
                    return Err(AppError::Validation("dueDate"));
                }
                CreateWorkItemOutcome::ParentNotFound
                | CreateWorkItemOutcome::ParentArchived(_)
                | CreateWorkItemOutcome::ParentTooDeep => {
                    return Err(AppError::Validation("parentWorkItemId"));
                }
                CreateWorkItemOutcome::InvalidPlacement(_) => {
                    return Err(AppError::Internal("new Work Item had a board position"));
                }
            };
            if let Some(message_id) = request.source_message_id {
                let linked = sqlx::query(
                    r#"
                    INSERT INTO work_item_links (
                        work_item_id, project_id, target_kind, target_id, created_by_actor_id
                    )
                    SELECT $1, $2, 'message', message.id, $3
                    FROM messages message
                    WHERE message.id = $4 AND message.project_id = $2
                      AND message.removed_at IS NULL
                      AND conversation_visible_to(message.conversation_id, $3)
                    ON CONFLICT DO NOTHING
                    "#,
                )
                .bind(work_item_id)
                .bind(project_id)
                .bind(principal.actor_id)
                .bind(message_id)
                .execute(&mut *transaction)
                .await?
                .rows_affected();
                if linked == 0 {
                    return Err(AppError::Validation("sourceMessageId"));
                }
            }
            let row = fetch_work_item_in_transaction(
                &mut transaction,
                project_id,
                work_item_id,
                principal.actor_id,
            )
            .await?
            .ok_or(AppError::Internal("created Work Item is unavailable"))?;
            let item = work_item_view(row, capabilities(&membership)?)?;
            complete(&mut transaction, &reservation, StatusCode::CREATED, &item).await?;
            transaction.commit().await?;
            work_item_response(StatusCode::CREATED, item)
        }
    }
}

async fn list_acceptance_criteria(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<Vec<AcceptanceCriterionView>>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemView,
        Some(work_item_id),
    )?;
    let row = fetch_work_item(&state.pool, project_id, work_item_id, principal.actor_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    Ok(Json(
        work_item_view(row, capabilities(&membership)?)?.acceptance_criteria,
    ))
}

async fn create_acceptance_criterion(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<CreateCriterionRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    request.text = normalize_criterion_text(&request.text)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "criterion_create")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    let item_capabilities = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, item_capabilities)?);
    }
    if current.archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let operations = [CriteriaOperation::Add {
        text: request.text.clone(),
        position: None,
    }];
    let outcome = change_criteria(
        &mut transaction,
        &browser_actor(&headers, &principal, project_id)?,
        work_item_id,
        CriteriaCommand {
            expected_version: request.expected_version,
            operations: &operations,
            audit_action: "work_item.criterion_created",
        },
    )
    .await?;
    if !matches!(outcome, CriteriaOutcome::Applied { .. }) {
        return criteria_refusal(
            &mut transaction,
            project_id,
            work_item_id,
            principal.actor_id,
            request.expected_version,
            outcome,
            item_capabilities,
        )
        .await;
    }
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
        item_capabilities,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::CREATED, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::CREATED, item)
}

async fn update_acceptance_criterion(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, criterion_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(mut request): ApiJson<UpdateCriterionRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1
        || (request.text.is_none() && request.position.is_none() && request.completed.is_none())
    {
        return Err(AppError::Validation("acceptance criterion update"));
    }
    request.text = request
        .text
        .as_deref()
        .map(normalize_criterion_text)
        .transpose()?;
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("criteria", criterion_id, "update")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    let item_capabilities = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, item_capabilities)?);
    }
    if current.archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let operations = [CriteriaOperation::Update {
        target: CriterionTarget::Id(criterion_id),
        text: request.text.clone(),
        position: request.position,
        completed: request.completed,
    }];
    let outcome = change_criteria(
        &mut transaction,
        &browser_actor(&headers, &principal, project_id)?,
        work_item_id,
        CriteriaCommand {
            expected_version: request.expected_version,
            operations: &operations,
            audit_action: "work_item.criterion_updated",
        },
    )
    .await?;
    if !matches!(
        outcome,
        CriteriaOutcome::Applied { .. } | CriteriaOutcome::Unchanged { .. }
    ) {
        return criteria_refusal(
            &mut transaction,
            project_id,
            work_item_id,
            principal.actor_id,
            request.expected_version,
            outcome,
            item_capabilities,
        )
        .await;
    }
    // Unchanged answers 200 with the item as it is, without a version bump.
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
        item_capabilities,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

async fn remove_acceptance_criterion(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id, criterion_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<RemoveCriterionRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    if request.expected_version < 1 {
        return Err(AppError::Validation("expectedVersion"));
    }
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("criteria", criterion_id, "remove")?;
    let request_hash = RequestHash::for_value(&request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let item: WorkItemView = replay.decode()?;
            transaction.commit().await?;
            return work_item_response(replay.status(), item);
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    let item_capabilities = capabilities(&membership)?;
    if current.version != request.expected_version {
        return work_item_version_conflict_response(work_item_view(current, item_capabilities)?);
    }
    if current.archived_at.is_some() || membership.project_archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let operations = [CriteriaOperation::Remove {
        target: CriterionTarget::Id(criterion_id),
    }];
    let outcome = change_criteria(
        &mut transaction,
        &browser_actor(&headers, &principal, project_id)?,
        work_item_id,
        CriteriaCommand {
            expected_version: request.expected_version,
            operations: &operations,
            audit_action: "work_item.criterion_removed",
        },
    )
    .await?;
    if !matches!(outcome, CriteriaOutcome::Applied { .. }) {
        return criteria_refusal(
            &mut transaction,
            project_id,
            work_item_id,
            principal.actor_id,
            request.expected_version,
            outcome,
            item_capabilities,
        )
        .await;
    }
    let item = current_work_item_view(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
        item_capabilities,
    )
    .await?;
    complete(&mut transaction, &reservation, StatusCode::OK, &item).await?;
    transaction.commit().await?;
    work_item_response(StatusCode::OK, item)
}

/// The response for a criteria change the shared command didn't apply, in
/// the shapes the web app has always had. The handlers check the version and
/// archive state under the same lock first, so a conflict here is rare.
async fn criteria_refusal(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    viewer: Uuid,
    expected_version: i64,
    outcome: CriteriaOutcome,
    item_capabilities: WorkItemCapabilities,
) -> Result<Response, AppError> {
    match outcome {
        CriteriaOutcome::Conflict(current)
            if conflict_kind(&current, expected_version) == Some(ConflictKind::Version) =>
        {
            work_item_version_conflict_response(
                current_work_item_view(
                    transaction,
                    project_id,
                    work_item_id,
                    viewer,
                    item_capabilities,
                )
                .await?,
            )
        }
        CriteriaOutcome::Conflict(_) => Err(AppError::Conflict("work_item", "archived")),
        CriteriaOutcome::NotFound => Err(AppError::NotFound("work_item")),
        CriteriaOutcome::Plan {
            error: CriteriaPlanError::TooMany { .. },
            ..
        } => Err(AppError::Validation("acceptanceCriteria")),
        CriteriaOutcome::Plan {
            error: CriteriaPlanError::Position { .. },
            ..
        } => Err(AppError::Validation("position")),
        CriteriaOutcome::Plan {
            error:
                CriteriaPlanError::UnknownCriterion { .. } | CriteriaPlanError::RemovedEarlier { .. },
            ..
        } => Err(AppError::NotFound("acceptance_criterion")),
        CriteriaOutcome::Applied { .. } | CriteriaOutcome::Unchanged { .. } => Err(
            AppError::Internal("an applied criteria change has no refusal"),
        ),
    }
}

async fn create_work_item_comment(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CreateCommentRequest>,
) -> Result<Response, AppError> {
    require_authenticated_write(&state, &headers, &principal)?;
    let content = MessageContent::from_json(request.content).map_err(message_content_error)?;
    let key = IdempotencyKey::from_headers(&headers)?;
    let scope = resource_bound_scope("work_items", work_item_id, "comment_create")?;
    let request_hash = RequestHash::for_value(&content)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_project_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemEdit,
        Some(work_item_id),
    )?;
    if membership.project_archived_at.is_some() {
        return Err(AppError::ProjectArchived);
    }
    let reservation = match reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let body: CommentAppendResponse = replay.decode()?;
            transaction.commit().await?;
            return Ok((replay.status(), Json(body)).into_response());
        }
        IdempotencyOutcome::Reserved(reservation) => reservation,
    };
    let current = fetch_work_item_for_update(
        &mut transaction,
        project_id,
        work_item_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::NotFound("work_item"))?;
    if current.archived_at.is_some() {
        return Err(AppError::Conflict("work_item", "archived"));
    }
    let mention_ids = content.mentioned_actor_ids();
    let mention_rows = fetch_comment_actor_rows(&mut transaction, project_id, &mention_ids).await?;
    let handles = mention_rows
        .iter()
        .map(|actor| (actor.id, actor.handle.clone()))
        .collect::<HashMap<_, _>>();
    let references = content.work_item_references();
    let reference_ids = references
        .iter()
        .map(|(_, work_item_id)| *work_item_id)
        .collect::<Vec<_>>();
    let reference_rows =
        fetch_work_item_reference_rows(&mut transaction, project_id, &reference_ids).await?;
    let labels = reference_rows
        .iter()
        .map(|item| (item.id, item.key.clone()))
        .collect::<HashMap<_, _>>();
    let mention_nodes = content
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| match node {
            MessageNode::Mention { actor_id } => Some((index, *actor_id)),
            MessageNode::Text { .. } | MessageNode::WorkItemRef { .. } => None,
        })
        .collect::<Vec<_>>();
    let (content, plain_text, _) = content
        .validate_with_resources(
            |actor_id| handles.get(&actor_id).cloned(),
            |referenced_id| labels.get(&referenced_id).cloned(),
        )
        .map_err(message_content_error)?
        .into_parts();
    let content_json = serde_json::to_value(content)
        .map_err(|_| AppError::Internal("serialize Work Item comment"))?;
    let comment_id = Uuid::now_v7();
    let created_at = sqlx::query_scalar::<_, String>(
        r#"
        INSERT INTO work_item_comments (
            id, project_id, work_item_id, author_actor_id,
            content_version, content, plain_text
        ) VALUES ($1, $2, $3, $4, 1, $5, $6)
        RETURNING created_at::text
        "#,
    )
    .bind(comment_id)
    .bind(project_id)
    .bind(work_item_id)
    .bind(principal.actor_id)
    .bind(&content_json)
    .bind(&plain_text)
    .fetch_one(&mut *transaction)
    .await?;
    for (node_index, actor_id) in mention_nodes {
        sqlx::query(
            "INSERT INTO work_item_comment_mentions (project_id, work_item_id, comment_id, actor_id, node_index) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(comment_id)
        .bind(actor_id)
        .bind(i32::try_from(node_index).map_err(|_| AppError::Validation("comment content"))?)
        .execute(&mut *transaction)
        .await?;
    }
    for (node_index, referenced_id) in &references {
        sqlx::query(
            "INSERT INTO work_item_comment_references (project_id, source_work_item_id, comment_id, referenced_work_item_id, node_index) VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(project_id)
        .bind(work_item_id)
        .bind(comment_id)
        .bind(referenced_id)
        .bind(i32::try_from(*node_index).map_err(|_| AppError::Validation("comment content"))?)
        .execute(&mut *transaction)
        .await?;
    }
    let (sequence, _) = finalize_comment(
        &mut transaction,
        &browser_actor(&headers, &principal, project_id)?,
        work_item_id,
        comment_id,
        current.version,
        "work_item.comment_created",
    )
    .await?;
    let author = fetch_comment_actor_rows(&mut transaction, project_id, &[principal.actor_id])
        .await?
        .into_iter()
        .next()
        .ok_or(AppError::Internal("comment author unavailable"))?;
    let body = CommentAppendResponse {
        work_item_id,
        sequence,
        comment: WorkItemCommentView {
            id: comment_id,
            author,
            content: content_json,
            plain_text,
            mentioned_actors: mention_rows,
            referenced_work_items: reference_rows
                .into_iter()
                .map(|row| work_item_reference_view(project_id, row))
                .collect(),
            created_at,
            via: None,
        },
    };
    complete(&mut transaction, &reservation, StatusCode::CREATED, &body).await?;
    transaction.commit().await?;
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

async fn get_work_item_timeline(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath((project_id, work_item_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<WorkItemTimelineQuery>,
) -> Result<Json<WorkItemTimelineResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    authorize_work_item(
        &principal,
        &membership,
        Action::WorkItemView,
        Some(work_item_id),
    )?;
    fetch_work_item(&state.pool, project_id, work_item_id, principal.actor_id)
        .await?
        .ok_or(AppError::NotFound("work_item"))?;
    let before_sequence = query
        .cursor
        .as_deref()
        .map(|cursor| decode_work_item_timeline_cursor(cursor, project_id, work_item_id))
        .transpose()?;
    let limit = i64::from(query.limit.unwrap_or(50).clamp(1, 100));
    let TimelinePage { rows, has_more } = fetch_timeline_rows(
        &state.pool,
        project_id,
        work_item_id,
        before_sequence,
        limit,
    )
    .await?;
    let next_cursor = if has_more {
        rows.last()
            .map(|row| encode_work_item_timeline_cursor(project_id, work_item_id, row.sequence))
            .transpose()?
    } else {
        None
    };
    let mut entries = Vec::with_capacity(rows.len());
    for row in rows {
        entries.push(timeline_entry_view(&state.pool, project_id, work_item_id, row).await?);
    }
    // Field changes name statuses, people and milestones, not their IDs.
    let mut payloads = entries
        .iter_mut()
        .filter_map(|entry| {
            entry
                .activity
                .as_mut()
                .map(|activity| &mut activity.payload)
        })
        .collect::<Vec<_>>();
    crate::work_item_history::label_changes(&state.pool, project_id, &mut payloads).await?;
    Ok(Json(WorkItemTimelineResponse {
        entries,
        next_cursor,
    }))
}

/// A page of a task's timeline, newest first: up to `limit` entries before
/// `before_sequence` (or the latest ones), for the web timeline and MCP
/// history alike. The caller has checked the reader may see the task.
pub(crate) async fn fetch_timeline_rows(
    pool: &PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
    before_sequence: Option<i64>,
    limit: i64,
) -> Result<TimelinePage, AppError> {
    let limit = limit.clamp(1, 100);
    let mut rows = sqlx::query_as::<_, WorkItemTimelineRow>(
        r#"
        SELECT entry.sequence, entry.kind, entry.created_at::text AS created_at,
               activity.id AS activity_id, activity.kind AS activity_kind,
               activity.summary AS activity_summary,
               activity.payload AS activity_payload,
               comment.id AS comment_id, comment.content AS comment_content,
               comment.plain_text AS comment_plain_text,
               actor.id AS actor_id, actor.kind AS actor_kind,
               membership.display_name AS actor_display_name,
               membership.handle AS actor_handle,
               via_client.client_name AS via
        FROM work_item_timeline_entries entry
        LEFT JOIN work_item_activity activity ON activity.id = entry.activity_id
        LEFT JOIN work_item_comments comment ON comment.id = entry.comment_id
        JOIN actors actor ON actor.id = COALESCE(activity.actor_id, comment.author_actor_id)
        JOIN project_memberships membership
          ON membership.project_id = entry.project_id AND membership.actor_id = actor.id
        LEFT JOIN mcp_oauth_grants via_grant
          ON via_grant.id = COALESCE(activity.via_oauth_grant_id, comment.via_oauth_grant_id)
         AND via_grant.show_via
        LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
        WHERE entry.project_id = $1 AND entry.work_item_id = $2
          AND ($3::bigint IS NULL OR entry.sequence < $3)
        ORDER BY entry.sequence DESC
        LIMIT $4
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(before_sequence)
    .bind(limit + 1)
    .fetch_all(pool)
    .await?;
    let page = usize::try_from(limit).unwrap_or(100);
    let has_more = rows.len() > page;
    rows.truncate(page);
    Ok(TimelinePage { rows, has_more })
}

async fn fetch_comment_actor_rows(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    actor_ids: &[Uuid],
) -> Result<Vec<ActorView>, AppError> {
    if actor_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, (Uuid, String, String, String)>(
        r#"
        SELECT actor.id, actor.kind, membership.display_name, membership.handle
        FROM project_memberships membership
        JOIN actors actor ON actor.id = membership.actor_id
        WHERE membership.project_id = $1
          AND membership.actor_id = ANY($2)
          AND membership.enabled AND actor.status = 'active'
        ORDER BY lower(membership.display_name), actor.id
        FOR SHARE OF membership, actor
        "#,
    )
    .bind(project_id)
    .bind(actor_ids)
    .fetch_all(&mut **transaction)
    .await?
    .into_iter()
    .map(|(id, kind, display_name, handle)| ActorView {
        id,
        kind,
        display_name,
        handle,
        avatar_url: None,
    })
    .collect())
}

async fn fetch_work_item_reference_rows(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_ids: &[Uuid],
) -> Result<Vec<WorkItemReferenceRow>, AppError> {
    if work_item_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as::<_, WorkItemReferenceRow>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key,
               item.title, status.semantic_category AS status_category,
               item.archived_at::text AS archived_at
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.id = ANY($2)
        ORDER BY item.number, item.id
        FOR SHARE OF item
        "#,
    )
    .bind(project_id)
    .bind(work_item_ids)
    .fetch_all(&mut **transaction)
    .await?)
}

fn work_item_reference_view(project_id: Uuid, row: WorkItemReferenceRow) -> WorkItemReferenceView {
    WorkItemReferenceView {
        id: row.id,
        key: row.key,
        title: row.title,
        status_category: row.status_category,
        archived_at: row.archived_at,
        href: format!("/projects/{project_id}/work-items/{}", row.id),
    }
}

async fn timeline_entry_view(
    pool: &PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
    row: WorkItemTimelineRow,
) -> Result<WorkItemTimelineEntryView, AppError> {
    let actor = ActorView {
        id: row.actor_id,
        kind: row.actor_kind,
        display_name: row.actor_display_name,
        handle: row.actor_handle,
        avatar_url: None,
    };
    match row.kind.as_str() {
        "activity" => Ok(WorkItemTimelineEntryView {
            sequence: row.sequence,
            kind: row.kind,
            created_at: row.created_at,
            activity: Some(WorkItemActivityView {
                id: row
                    .activity_id
                    .ok_or(AppError::Internal("activity timeline entry"))?,
                actor,
                kind: row
                    .activity_kind
                    .ok_or(AppError::Internal("activity timeline kind"))?,
                summary: row
                    .activity_summary
                    .ok_or(AppError::Internal("activity timeline summary"))?,
                payload: row
                    .activity_payload
                    .ok_or(AppError::Internal("activity timeline payload"))?,
                via: row.via,
            }),
            comment: None,
        }),
        "comment" => {
            let comment_id = row
                .comment_id
                .ok_or(AppError::Internal("comment timeline entry"))?;
            let mentioned_actors = sqlx::query_as::<_, (Uuid, String, String, String)>(
                r#"
                SELECT actor.id, actor.kind, membership.display_name, membership.handle
                FROM work_item_comment_mentions mention
                JOIN actors actor ON actor.id = mention.actor_id
                JOIN project_memberships membership
                  ON membership.project_id = mention.project_id
                 AND membership.actor_id = mention.actor_id
                WHERE mention.project_id = $1 AND mention.work_item_id = $2
                  AND mention.comment_id = $3
                GROUP BY actor.id, actor.kind, membership.display_name, membership.handle
                ORDER BY min(mention.node_index), actor.id
                "#,
            )
            .bind(project_id)
            .bind(work_item_id)
            .bind(comment_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(id, kind, display_name, handle)| ActorView {
                id,
                kind,
                display_name,
                handle,
                avatar_url: None,
            })
            .collect();
            let referenced_work_items = sqlx::query_as::<_, WorkItemReferenceRow>(
                r#"
                SELECT item.id,
                       project.project_key || '-' || item.number::text AS key,
                       item.title, status.semantic_category AS status_category,
                       item.archived_at::text AS archived_at
                FROM work_item_comment_references reference
                JOIN work_items item
                  ON item.project_id = reference.project_id
                 AND item.id = reference.referenced_work_item_id
                JOIN projects project ON project.id = item.project_id
                JOIN work_item_statuses status
                  ON status.project_id = item.project_id AND status.id = item.status_id
                WHERE reference.project_id = $1
                  AND reference.source_work_item_id = $2
                  AND reference.comment_id = $3
                GROUP BY item.id, project.project_key, item.number, item.title,
                         status.semantic_category, item.archived_at
                ORDER BY min(reference.node_index), item.id
                "#,
            )
            .bind(project_id)
            .bind(work_item_id)
            .bind(comment_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|item| work_item_reference_view(project_id, item))
            .collect();
            Ok(WorkItemTimelineEntryView {
                sequence: row.sequence,
                kind: row.kind,
                created_at: row.created_at.clone(),
                activity: None,
                comment: Some(WorkItemCommentView {
                    id: comment_id,
                    author: actor,
                    content: row
                        .comment_content
                        .ok_or(AppError::Internal("comment timeline content"))?,
                    plain_text: row
                        .comment_plain_text
                        .ok_or(AppError::Internal("comment timeline text"))?,
                    mentioned_actors,
                    referenced_work_items,
                    created_at: row.created_at,
                    via: row.via,
                }),
            })
        }
        _ => Err(AppError::Internal("Work Item timeline kind")),
    }
}

async fn project_membership(
    pool: &PgPool,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectMembershipRow>, AppError> {
    Ok(sqlx::query_as::<_, ProjectMembershipRow>(
        r#"
        SELECT project.id AS project_id,
               project.archived_at::text AS project_archived_at,
               membership.role, membership.enabled AS membership_enabled
        FROM projects project
        JOIN project_memberships membership ON membership.project_id = project.id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = membership.actor_id
         AND organization_membership.enabled
        WHERE project.id = $1 AND membership.actor_id = $2
        "#,
    )
    .bind(project_id)
    .bind(actor_id)
    .fetch_optional(pool)
    .await?)
}

async fn lock_project_membership(
    transaction: &mut Transaction<'_, Postgres>,
    actor_id: Uuid,
    project_id: Uuid,
) -> Result<Option<ProjectMembershipRow>, AppError> {
    Ok(lock_membership(transaction, actor_id, project_id)
        .await?
        .map(|membership| ProjectMembershipRow {
            project_id: membership.project_id,
            project_archived_at: membership.project_archived_at,
            role: membership.role,
            membership_enabled: membership.enabled,
        }))
}

fn authorize_work_item(
    principal: &AuthenticatedActor,
    membership: &ProjectMembershipRow,
    action: Action,
    work_item_id: Option<Uuid>,
) -> Result<(), AppError> {
    let command_actor = CommandActor {
        actor_id: principal.actor_id,
        actor_kind: actor_kind(&principal.actor_kind)?,
        project_id: membership.project_id,
        audit: AuditAttribution::Browser {
            session_id: principal.session_id,
            request_id: None,
        },
    };
    let command_membership = ProjectMembership {
        project_id: membership.project_id,
        project_archived_at: membership.project_archived_at.clone(),
        role: membership.role.clone(),
        enabled: membership.membership_enabled,
    };
    match authorize_mutation(&command_actor, &command_membership, action, work_item_id)? {
        Decision::Allow => Ok(()),
        Decision::Deny(_) if action == Action::WorkItemView => Err(AppError::NotFound("work_item")),
        Decision::Deny(_) => Err(AppError::Forbidden),
    }
}

fn capabilities(membership: &ProjectMembershipRow) -> Result<WorkItemCapabilities, AppError> {
    let role = membership_role(&membership.role)?;
    Ok(WorkItemCapabilities {
        view: membership.membership_enabled,
        edit: membership.membership_enabled
            && matches!(
                role,
                MembershipRole::Owner | MembershipRole::Admin | MembershipRole::Member
            ),
        archive: membership.membership_enabled
            && matches!(
                role,
                MembershipRole::Owner | MembershipRole::Admin | MembershipRole::Member
            ),
    })
}

async fn fetch_work_item(
    pool: &PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
    viewer: Uuid,
) -> Result<Option<WorkItemRow>, AppError> {
    Ok(sqlx::query_as::<_, WorkItemRow>(WORK_ITEM_SELECT)
        .bind(project_id)
        .bind(work_item_id)
        .bind(viewer)
        .fetch_optional(pool)
        .await?)
}

async fn fetch_work_item_in_transaction(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    viewer: Uuid,
) -> Result<Option<WorkItemRow>, AppError> {
    Ok(sqlx::query_as::<_, WorkItemRow>(WORK_ITEM_SELECT)
        .bind(project_id)
        .bind(work_item_id)
        .bind(viewer)
        .fetch_optional(&mut **transaction)
        .await?)
}

async fn fetch_work_item_for_update(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    viewer: Uuid,
) -> Result<Option<WorkItemRow>, AppError> {
    let query = format!("{WORK_ITEM_SELECT} FOR UPDATE OF item");
    Ok(sqlx::query_as::<_, WorkItemRow>(&query)
        .bind(project_id)
        .bind(work_item_id)
        .bind(viewer)
        .fetch_optional(&mut **transaction)
        .await?)
}

async fn current_work_item_view(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    viewer: Uuid,
    capabilities: WorkItemCapabilities,
) -> Result<WorkItemView, AppError> {
    let row = fetch_work_item_in_transaction(transaction, project_id, work_item_id, viewer)
        .await?
        .ok_or(AppError::Internal("updated Work Item is unavailable"))?;
    work_item_view(row, capabilities)
}

async fn advance_work_item_version(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    expected_version: i64,
) -> Result<i64, AppError> {
    sqlx::query_scalar(
        r#"
        UPDATE work_items
        SET version = version + 1, updated_at = now()
        WHERE project_id = $1 AND id = $2 AND version = $3
        RETURNING version
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .bind(expected_version)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::Internal("locked Work Item lost its version"))
}

/// A task's active labels as `[{id, name, color}]`, read through the view so
/// archived labels never show. Cards show labels too, so the list and board
/// queries select the real thing, not `'[]'`.
macro_rules! work_item_labels_column {
    () => {
        r#"
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object('id', active.label_id, 'name', active.name, 'color', active.color)
                ORDER BY lower(active.name), active.label_id
            )
            FROM work_item_active_labels active
            WHERE active.project_id = item.project_id AND active.work_item_id = item.id
        ), '[]'::jsonb) AS labels
        "#
    };
}
const WORK_ITEM_LABELS_COLUMN: &str = work_item_labels_column!();

/// A task as `TaskRef` JSON; `$t` is the task, `$s` its status and `$p`
/// its project.
macro_rules! task_ref_json {
    ($t:literal, $s:literal, $p:literal) => {
        concat!(
            "jsonb_build_object('id', ",
            $t,
            ".id, 'projectId', ",
            $t,
            ".project_id, ",
            "'key', ",
            $p,
            ".project_key || '-' || ",
            $t,
            ".number, 'title', ",
            $t,
            ".title, ",
            "'statusName', ",
            $s,
            ".name, 'statusCategory', ",
            $s,
            ".semantic_category, ",
            "'finished', ",
            $s,
            ".semantic_category IN ('completed', 'canceled'), ",
            "'archived', ",
            $t,
            ".archived_at IS NOT NULL, ",
            "'href', '/projects/' || ",
            $t,
            ".project_id || '/work-items/' || ",
            $t,
            ".id)"
        )
    };
}

/// An actor as `ActorView` JSON, named by their membership `$m`.
macro_rules! actor_json {
    ($a:literal, $m:literal) => {
        concat!(
            "jsonb_build_object('id', ",
            $a,
            ".id, 'kind', ",
            $a,
            ".kind, ",
            "'displayName', COALESCE(",
            $m,
            ".display_name, 'Unknown'), ",
            "'handle', COALESCE(",
            $m,
            ".handle, ''), 'avatarUrl', NULL)"
        )
    };
}

/// One task as `$3` (the viewing Actor) sees it: linked threads and pull
/// requests from private conversations they are not in are left out.
const WORK_ITEM_SELECT: &str = concat!(
    r#"
    SELECT
        item.id, item.project_id, project.project_key,
        item.number, item.title, item.description,
        item.type_id, item_type.name AS type_name,
        item.status_id, status.name AS status_name,
        status.semantic_category, item.priority,
        item.assignee_actor_id,
        assignee.display_name AS assignee_display_name,
        assignee.handle AS assignee_handle,
        assignee_actor.kind AS assignee_kind,
        CASE WHEN assignee.actor_id IS NULL THEN NULL
             ELSE assignee.enabled AND assignee_actor.status = 'active'
        END AS assignee_active,
        item.version, item.archived_at::text AS archived_at,
        item.created_at::text AS created_at,
        item.updated_at::text AS updated_at,
"#,
    work_item_labels_column!(),
    r#",
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', criterion.id,
                    'text', criterion.text,
                    'position', criterion.position,
                    'completed', criterion.completed,
                    'completedBy', CASE
                        WHEN criterion.completed_by_actor_id IS NULL THEN NULL
                        ELSE jsonb_build_object(
                            'id', completed_actor.id,
                            'kind', completed_actor.kind,
                            'displayName', completed_membership.display_name,
                            'handle', completed_membership.handle,
                            'avatarUrl', NULL
                        )
                    END,
                    'completedAt', criterion.completed_at::text,
                    'createdAt', criterion.created_at::text,
                    'updatedAt', criterion.updated_at::text
                ) ORDER BY criterion.position, criterion.id
            )
            FROM work_item_acceptance_criteria criterion
            LEFT JOIN project_memberships completed_membership
              ON completed_membership.project_id = criterion.project_id
             AND completed_membership.actor_id = criterion.completed_by_actor_id
            LEFT JOIN actors completed_actor
              ON completed_actor.id = criterion.completed_by_actor_id
            WHERE criterion.project_id = item.project_id
              AND criterion.work_item_id = item.id
        ), '[]'::jsonb) AS acceptance_criteria,
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', thread.id,
                    'name', thread.name,
                    'parentChannelId', channel.id,
                    'parentChannelName', channel.name,
                    'archivedAt', thread.archived_at::text,
                    'newestRunState', newest_run.state,
                    'href', '/projects/' || item.project_id || '/threads/' || thread.id
                ) ORDER BY link.created_at, thread.id
            )
            FROM work_item_conversation_links link
            JOIN conversations thread
              ON thread.project_id = link.project_id AND thread.id = link.conversation_id
            JOIN conversations channel
              ON channel.project_id = thread.project_id AND channel.id = thread.parent_channel_id
            LEFT JOIN LATERAL (
                SELECT run.state
                FROM agent_runs run
                WHERE run.project_id = thread.project_id
                  AND run.conversation_id = thread.id
                ORDER BY run.created_at DESC, run.id DESC
                LIMIT 1
            ) newest_run ON true
            WHERE link.project_id = item.project_id AND link.work_item_id = item.id
              AND conversation_visible_to(thread.id, $3)
        ), '[]'::jsonb) AS linked_threads,
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', repository.id,
                    'owner', repository.owner,
                    'name', repository.name,
                    'cloneUrl', repository.clone_url,
                    'role', link.role,
                    'preparationState', repository.preparation_state,
                    'trusted', repository.trusted,
                    'available', repository.enabled AND repository.trusted
                        AND repository.archived_at IS NULL
                        AND repository.preparation_state = 'ready',
                    'archivedAt', repository.archived_at::text,
                    'href', '/projects/' || item.project_id || '/settings'
                ) ORDER BY CASE link.role WHEN 'primary' THEN 0 ELSE 1 END,
                         repository.owner, repository.name, repository.id
            )
            FROM work_item_repository_links link
            JOIN repositories repository
              ON repository.project_id = link.project_id AND repository.id = link.repository_id
            WHERE link.project_id = item.project_id AND link.work_item_id = item.id
        ), '[]'::jsonb) AS linked_repositories,
        COALESCE((
            SELECT jsonb_agg(
                jsonb_build_object(
                    'id', pull_request.id,
                    'number', pull_request.pull_request_number,
                    'title', pull_request.title,
                    'url', pull_request.html_url,
                    'state', pull_request.state,
                    'draft', pull_request.draft,
                    'checksState', pull_request.checks_state,
                    'reviewState', pull_request.review_state,
                    'mergeabilityState', pull_request.mergeability_state,
                    'syncState', pull_request.sync_state,
                    'syncFailureCategory', pull_request.sync_failure_category,
                    'automationState', pull_request.automation_state,
                    'automationReason', pull_request.automation_reason,
                    'source', link.source,
                    'confirmed', link.confirmed
                ) ORDER BY pull_request.pull_request_number, pull_request.id
            )
            FROM github_pull_request_work_item_links link
            JOIN github_pull_requests pull_request
              ON pull_request.project_id = link.project_id
             AND pull_request.id = link.github_pull_request_id
            WHERE link.project_id = item.project_id AND link.work_item_id = item.id
              AND pull_request_visible_to(pull_request.id, $3)
        ), '[]'::jsonb) AS linked_pull_requests,
"#,
    r#"
        (SELECT "#,
    task_ref_json!("parent", "parent_status", "project"),
    r#"
           FROM work_items parent
           JOIN work_item_statuses parent_status
             ON parent_status.project_id = parent.project_id AND parent_status.id = parent.status_id
          WHERE parent.project_id = item.project_id AND parent.id = item.parent_work_item_id
        ) AS parent,
        (SELECT jsonb_build_object('id', milestone.id, 'name', milestone.name,
                                   'targetDate', milestone.target_date::text)
           FROM project_milestones milestone
          WHERE milestone.project_id = item.project_id AND milestone.id = item.milestone_id
        ) AS milestone,
        item.start_date::text AS start_date, item.due_date::text AS due_date, item.estimate,
        CASE WHEN item.blocked_reason IS NULL THEN NULL ELSE jsonb_build_object(
            'reason', item.blocked_reason, 'at', item.blocked_at::text,
            'by', (SELECT "#,
    actor_json!("blocker_actor", "blocker_membership"),
    r#"
                     FROM actors blocker_actor
                     LEFT JOIN project_memberships blocker_membership
                       ON blocker_membership.project_id = item.project_id
                      AND blocker_membership.actor_id = blocker_actor.id
                    WHERE blocker_actor.id = item.blocked_by_actor_id)
        ) END AS blocked,
        COALESCE((
            SELECT jsonb_agg("#,
    task_ref_json!("linked", "linked_status", "linked_project"),
    r#" ORDER BY linked_status.semantic_category IN ('completed', 'canceled'),
                         linked_project.project_key, linked.number)
            FROM work_item_dependencies dependency
            JOIN work_items linked
              ON linked.project_id = dependency.blocking_project_id
             AND linked.id = dependency.blocking_work_item_id
            JOIN projects linked_project ON linked_project.id = linked.project_id
            JOIN work_item_statuses linked_status
              ON linked_status.project_id = linked.project_id AND linked_status.id = linked.status_id
            WHERE dependency.project_id = item.project_id
              AND dependency.blocked_work_item_id = item.id
              AND linked.archived_at IS NULL
              AND (linked.project_id = item.project_id
                   OR actor_can_access_project($3, NULL, linked.project_id))
        ), '[]'::jsonb) AS blocked_by,
        COALESCE((
            SELECT jsonb_agg("#,
    task_ref_json!("linked", "linked_status", "linked_project"),
    r#" ORDER BY linked_status.semantic_category IN ('completed', 'canceled'),
                         linked_project.project_key, linked.number)
            FROM work_item_dependencies dependency
            JOIN work_items linked
              ON linked.project_id = dependency.project_id
             AND linked.id = dependency.blocked_work_item_id
            JOIN projects linked_project ON linked_project.id = linked.project_id
            JOIN work_item_statuses linked_status
              ON linked_status.project_id = linked.project_id AND linked_status.id = linked.status_id
            WHERE dependency.blocking_project_id = item.project_id
              AND dependency.blocking_work_item_id = item.id
              AND linked.archived_at IS NULL
              AND (linked.project_id = item.project_id
                   OR actor_can_access_project($3, NULL, linked.project_id))
        ), '[]'::jsonb) AS blocks,
        COALESCE((
            SELECT jsonb_agg(jsonb_build_object(
                'kind', CASE
                    WHEN relation.kind = 'duplicates' AND relation.work_item_id = item.id THEN 'duplicates'
                    WHEN relation.kind = 'duplicates' THEN 'duplicated_by'
                    ELSE 'relates_to' END,
                'task', "#,
    task_ref_json!("linked", "linked_status", "linked_project"),
    r#") ORDER BY relation.created_at, linked.id)
            FROM work_item_relations relation
            JOIN work_items linked
              ON linked.id = CASE WHEN relation.work_item_id = item.id
                                  THEN relation.other_work_item_id ELSE relation.work_item_id END
            JOIN projects linked_project ON linked_project.id = linked.project_id
            JOIN work_item_statuses linked_status
              ON linked_status.project_id = linked.project_id AND linked_status.id = linked.status_id
            WHERE (relation.work_item_id = item.id OR relation.other_work_item_id = item.id)
              AND (linked.project_id = item.project_id
                   OR actor_can_access_project($3, NULL, linked.project_id))
        ), '[]'::jsonb) AS relations,
        COALESCE((
            SELECT jsonb_agg("#,
    task_ref_json!("child", "child_status", "project"),
    r#" || jsonb_build_object('assignee', CASE WHEN child_assignee.actor_id IS NULL THEN NULL ELSE "#,
    actor_json!("child_assignee_actor", "child_assignee"),
    r#" END) ORDER BY child_status.position, child.board_rank, child.id)
            FROM work_items child
            JOIN work_item_statuses child_status
              ON child_status.project_id = child.project_id AND child_status.id = child.status_id
            LEFT JOIN project_memberships child_assignee
              ON child_assignee.project_id = child.project_id
             AND child_assignee.actor_id = child.assignee_actor_id
            LEFT JOIN actors child_assignee_actor ON child_assignee_actor.id = child.assignee_actor_id
            WHERE child.project_id = item.project_id AND child.parent_work_item_id = item.id
              AND child.archived_at IS NULL
        ), '[]'::jsonb) AS subtasks,
        (SELECT jsonb_build_object('actor', "#,
    actor_json!("claimer", "claimer_membership"),
    r#", 'note', claim.note,
                                   'claimedAt', claim.claimed_at::text,
                                   'expiresAt', claim.expires_at::text)
           FROM work_item_claims claim
           JOIN actors claimer ON claimer.id = claim.actor_id
           LEFT JOIN project_memberships claimer_membership
             ON claimer_membership.project_id = item.project_id
            AND claimer_membership.actor_id = claim.actor_id
          WHERE claim.work_item_id = item.id AND claim.expires_at > now()
        ) AS claim,
        COALESCE((
            SELECT jsonb_agg("#,
    actor_json!("watcher_actor", "watcher"),
    r#" ORDER BY lower(watcher.display_name), watcher.actor_id)
            FROM work_item_watchers watching
            JOIN project_memberships watcher
              ON watcher.project_id = watching.project_id AND watcher.actor_id = watching.actor_id
            JOIN actors watcher_actor ON watcher_actor.id = watching.actor_id
            WHERE watching.work_item_id = item.id AND watcher.enabled
        ), '[]'::jsonb) AS watchers,
        EXISTS (SELECT 1 FROM work_item_watchers watching
                 WHERE watching.work_item_id = item.id AND watching.actor_id = $3) AS watching,
        COALESCE((SELECT settings.estimate_unit FROM project_task_settings settings
                   WHERE settings.project_id = item.project_id), 'points') AS estimate_unit,
        item.blocked_reason IS NULL AND item.archived_at IS NULL
          AND status.semantic_category NOT IN ('completed', 'canceled')
          AND NOT EXISTS (
              SELECT 1
                FROM work_item_dependencies dependency
                JOIN work_items blocker
                  ON blocker.project_id = dependency.blocking_project_id
                 AND blocker.id = dependency.blocking_work_item_id
                JOIN work_item_statuses blocker_status
                  ON blocker_status.project_id = blocker.project_id
                 AND blocker_status.id = blocker.status_id
               WHERE dependency.project_id = item.project_id
                 AND dependency.blocked_work_item_id = item.id
                 AND blocker.archived_at IS NULL
                 AND blocker_status.semantic_category NOT IN ('completed', 'canceled')
          ) AS ready
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_types item_type
      ON item_type.project_id = item.project_id AND item_type.id = item.type_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = item.project_id
     AND assignee.actor_id = item.assignee_actor_id
    LEFT JOIN actors assignee_actor ON assignee_actor.id = assignee.actor_id
    WHERE item.project_id = $1 AND item.id = $2
"#
);

fn work_item_view(
    row: WorkItemRow,
    capabilities: WorkItemCapabilities,
) -> Result<WorkItemView, AppError> {
    validate_priority(&row.priority)?;
    validate_semantic_category(Some(&row.semantic_category))?;
    let assignee = match (
        row.assignee_actor_id,
        row.assignee_display_name,
        row.assignee_handle,
        row.assignee_kind,
        row.assignee_active,
    ) {
        (Some(actor_id), Some(display_name), Some(handle), Some(kind), Some(active)) => {
            Some(AssigneeView {
                actor_id,
                display_name,
                handle,
                kind,
                active,
            })
        }
        (None, None, None, None, None) => None,
        _ => return Err(AppError::Internal("invalid Work Item assignee projection")),
    };
    let labels = serde_json::from_value(row.labels)
        .map_err(|_| AppError::Internal("invalid Work Item label projection"))?;
    let acceptance_criteria = serde_json::from_value(row.acceptance_criteria)
        .map_err(|_| AppError::Internal("invalid Work Item criteria projection"))?;
    let linked_threads = serde_json::from_value(row.linked_threads)
        .map_err(|_| AppError::Internal("invalid linked Thread projection"))?;
    let linked_repositories = serde_json::from_value(row.linked_repositories)
        .map_err(|_| AppError::Internal("invalid linked Repository projection"))?;
    let linked_pull_requests = serde_json::from_value(row.linked_pull_requests)
        .map_err(|_| AppError::Internal("invalid linked pull-request projection"))?;
    Ok(WorkItemView {
        id: row.id,
        project_id: row.project_id,
        key: format!("{}-{}", row.project_key, row.number),
        number: row.number,
        title: row.title,
        description: row.description,
        item_type: WorkItemTypeView {
            id: row.type_id,
            name: row.type_name,
        },
        status: WorkItemStatusView {
            id: row.status_id,
            name: row.status_name,
            semantic_category: row.semantic_category,
        },
        priority: row.priority,
        assignee,
        version: row.version,
        archived_at: row.archived_at,
        created_at: row.created_at,
        updated_at: row.updated_at,
        labels,
        acceptance_criteria,
        linked_threads,
        linked_repositories,
        linked_pull_requests,
        capabilities,
        planning: TaskPlanningView {
            parent: row.parent,
            milestone: row.milestone,
            start_date: row.start_date,
            due_date: row.due_date,
            estimate: row.estimate,
            blocked: row.blocked,
            blocked_by: json_list(row.blocked_by)?,
            blocks: json_list(row.blocks)?,
            ready: row.ready,
            relations: json_list(row.relations)?,
            subtasks: json_list(row.subtasks)?,
            claim: row.claim,
            watchers: json_list(row.watchers)?,
            watching: row.watching,
            estimate_unit: row.estimate_unit,
        },
    })
}

fn json_list(value: serde_json::Value) -> Result<Vec<serde_json::Value>, AppError> {
    match value {
        serde_json::Value::Array(items) => Ok(items),
        _ => Err(AppError::Internal("invalid Work Item planning projection")),
    }
}

/// Most labels one change may add and remove together.
const MAX_LABEL_CHANGES: usize = 50;
/// Most acceptance criteria a new task may come with.
const MAX_NEW_CRITERIA: usize = 50;

fn parse_task_date(
    value: Option<&str>,
    field: &'static str,
) -> Result<Option<NaiveDate>, AppError> {
    value
        .map(|value| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| AppError::Validation(field))
        })
        .transpose()
}

fn normalize_blocked_reason(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_BLOCKED_REASON_CHARACTERS
        || value
            .chars()
            .any(|character| character.is_control() && character != '\n')
    {
        return Err(AppError::Validation("blockedReason"));
    }
    Ok(value.to_owned())
}

fn planning_field_changes(request: &UpdateWorkItemRequest) -> Result<TaskFieldChanges, AppError> {
    let date = |value: &Option<Option<String>>, field| {
        value
            .as_ref()
            .map(|value| parse_task_date(value.as_deref(), field))
            .transpose()
    };
    if request
        .estimate
        .flatten()
        .is_some_and(|estimate| !valid_estimate(estimate))
    {
        return Err(AppError::Validation("estimate"));
    }
    Ok(TaskFieldChanges {
        milestone_id: request.milestone_id,
        start_date: date(&request.start_date, "startDate")?,
        due_date: date(&request.due_date, "dueDate")?,
        estimate: request.estimate,
        blocked_reason: request
            .blocked_reason
            .as_ref()
            .map(|value| value.as_deref().map(normalize_blocked_reason).transpose())
            .transpose()?,
    })
}

/// Whether every ID is an active label of the project.
async fn labels_are_active(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    label_ids: &[Uuid],
) -> Result<bool, AppError> {
    if label_ids.is_empty() {
        return Ok(true);
    }
    let found = sqlx::query_scalar::<_, i64>(
        "SELECT count(DISTINCT id) FROM project_labels WHERE project_id = $1 AND id = ANY($2) AND archived_at IS NULL",
    )
    .bind(project_id)
    .bind(label_ids)
    .fetch_one(&mut **transaction)
    .await?;
    let mut distinct = label_ids.to_vec();
    distinct.sort();
    distinct.dedup();
    Ok(found == distinct.len() as i64)
}

fn normalize_title(value: &str) -> Result<String, AppError> {
    crate::work_item_commands::normalize_title(value)
}

fn normalize_description(value: &str) -> Result<String, AppError> {
    crate::work_item_commands::normalize_description(value)
}

fn normalize_criterion_text(value: &str) -> Result<String, AppError> {
    crate::work_item_commands::normalize_criterion_text(value)
}

fn normalize_search(value: Option<&str>) -> Result<Option<String>, AppError> {
    let value = value.map(str::trim).filter(|value| !value.is_empty());
    if value.is_some_and(|value| {
        value.chars().count() > MAX_SEARCH_CHARACTERS || value.chars().any(char::is_control)
    }) {
        return Err(AppError::Validation("search"));
    }
    Ok(value.map(str::to_owned))
}

fn validate_readable_key(value: &str) -> Result<(), AppError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'-')
        || !value.rsplit_once('-').is_some_and(|(project_key, number)| {
            !project_key.is_empty()
                && !number.is_empty()
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && number.parse::<i64>().is_ok_and(|number| number > 0)
        })
    {
        return Err(AppError::Invalid("Work Item key"));
    }
    Ok(())
}

fn deserialize_optional_field<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

fn default_priority() -> String {
    "no_priority".to_owned()
}

fn validate_priority(value: &str) -> Result<(), AppError> {
    crate::work_item_commands::validate_priority(value)
}

fn validate_priority_filter(value: Option<&str>) -> Result<(), AppError> {
    value.map_or(Ok(()), validate_priority)
}

fn validate_semantic_category(value: Option<&str>) -> Result<(), AppError> {
    if value.is_none_or(|value| {
        matches!(
            value,
            "backlog"
                | "unstarted"
                | "started"
                | "review"
                | "verification"
                | "completed"
                | "canceled"
        )
    }) {
        Ok(())
    } else {
        Err(AppError::Validation("semanticCategory"))
    }
}

pub(crate) fn encode_work_item_timeline_cursor(
    project_id: Uuid,
    work_item_id: Uuid,
    before_sequence: i64,
) -> Result<String, AppError> {
    if before_sequence < 1 {
        return Err(AppError::Internal("encode Work Item timeline cursor"));
    }
    let mut bytes = Vec::with_capacity(41);
    bytes.push(1);
    bytes.extend_from_slice(project_id.as_bytes());
    bytes.extend_from_slice(work_item_id.as_bytes());
    bytes.extend_from_slice(&before_sequence.to_be_bytes());
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn decode_work_item_timeline_cursor(
    value: &str,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<i64, AppError> {
    if value.len() > 80 {
        return Err(AppError::Invalid("Work Item timeline cursor"));
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| AppError::Invalid("Work Item timeline cursor"))?;
    if bytes.len() != 41
        || bytes[0] != 1
        || bytes[1..17] != *project_id.as_bytes()
        || bytes[17..33] != *work_item_id.as_bytes()
    {
        return Err(AppError::Invalid("Work Item timeline cursor"));
    }
    let sequence = i64::from_be_bytes(
        bytes[33..41]
            .try_into()
            .map_err(|_| AppError::Invalid("Work Item timeline cursor"))?,
    );
    if sequence < 1 {
        return Err(AppError::Invalid("Work Item timeline cursor"));
    }
    Ok(sequence)
}

fn message_content_error(_error: MessageContentError) -> AppError {
    AppError::Validation("comment content")
}

fn work_item_response(status: StatusCode, item: WorkItemView) -> Result<Response, AppError> {
    let location = HeaderValue::from_str(&format!(
        "/projects/{}/work-items/{}",
        item.project_id, item.id
    ))
    .map_err(|_| AppError::Internal("construct Work Item Location header"))?;
    Ok((status, [(LOCATION, location)], Json(item)).into_response())
}

fn work_item_version_conflict_response(item: WorkItemView) -> Result<Response, AppError> {
    Ok((
        StatusCode::CONFLICT,
        Json(WorkItemConflictEnvelope {
            error: WorkItemConflictBody {
                code: "version_conflict",
                message: "Someone changed this after you opened it. Refresh and try again.",
                details: WorkItemConflictDetails {
                    resource: "work_item",
                    current_version: item.version,
                    current: item,
                },
            },
        }),
    )
        .into_response())
}

#[allow(clippy::too_many_arguments)]
async fn record_work_item_change(
    transaction: &mut Transaction<'_, Postgres>,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    work_item_id: Uuid,
    kind: &'static str,
    summary: &'static str,
    audit_action: &'static str,
    payload: serde_json::Value,
    version: i64,
) -> Result<(), AppError> {
    finalize_change(
        transaction,
        &browser_actor(headers, principal, project_id)?,
        work_item_id,
        ChangeFinalization {
            activity_kind: kind,
            summary,
            audit_action,
            activity_payload: payload.clone(),
            audit_metadata: payload,
            version,
        },
    )
    .await?;
    Ok(())
}

pub(crate) async fn insert_work_item_activity(
    transaction: &mut Transaction<'_, Postgres>,
    project_id: Uuid,
    work_item_id: Uuid,
    actor_id: Uuid,
    kind: &str,
    summary: &str,
    payload: &serde_json::Value,
) -> Result<Uuid, AppError> {
    insert_activity(
        transaction,
        project_id,
        work_item_id,
        actor_id,
        kind,
        summary,
        payload,
    )
    .await
}

fn browser_actor(
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<CommandActor, AppError> {
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128)
        .map(str::to_owned);
    Ok(CommandActor {
        actor_id: principal.actor_id,
        actor_kind: actor_kind(&principal.actor_kind)?,
        project_id,
        audit: AuditAttribution::Browser {
            session_id: principal.session_id,
            request_id,
        },
    })
}

fn membership_role(value: &str) -> Result<MembershipRole, AppError> {
    match value {
        "owner" => Ok(MembershipRole::Owner),
        "admin" => Ok(MembershipRole::Admin),
        "member" => Ok(MembershipRole::Member),
        "viewer" => Ok(MembershipRole::Viewer),
        "worker" => Ok(MembershipRole::Worker),
        _ => Err(AppError::Internal("invalid membership role")),
    }
}

fn actor_kind(value: &str) -> Result<ActorKind, AppError> {
    match value {
        "human" => Ok(ActorKind::Human),
        "external_agent" => Ok(ActorKind::ExternalAgent),
        "execution_worker" => Ok(ActorKind::ExecutionWorker),
        "integration" => Ok(ActorKind::Integration),
        "system" => Ok(ActorKind::System),
        _ => Err(AppError::Internal("invalid actor kind")),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_DESCRIPTION_CHARACTERS, UpdateWorkItemRequest, normalize_description, normalize_title,
        normalize_worker_prompt, validate_priority, validate_readable_key, worker_thread_name,
    };
    use uuid::Uuid;

    #[test]
    fn work_item_text_and_priority_are_bounded() {
        assert_eq!(
            normalize_title("  Build it  ").expect("normalize valid Work Item title"),
            "Build it"
        );
        assert!(normalize_title("bad\u{1b}").is_err());
        assert!(normalize_description("line one\nline two").is_ok());
        assert!(normalize_description("bad\u{7}").is_err());
        assert!(normalize_description(&"a".repeat(MAX_DESCRIPTION_CHARACTERS + 1)).is_err());
        assert!(validate_priority("urgent").is_ok());
        assert!(validate_priority("critical").is_err());
    }

    #[test]
    fn readable_keys_and_nullable_assignee_patch_are_unambiguous() {
        assert!(validate_readable_key("AW-42").is_ok());
        assert!(validate_readable_key("aw-42").is_err());
        assert!(validate_readable_key("AW-nope").is_err());

        let omitted: UpdateWorkItemRequest = serde_json::from_value(serde_json::json!({
            "expectedVersion": 1,
            "title": "Keep assignment"
        }))
        .expect("decode omitted assignee patch");
        assert_eq!(omitted.assignee_actor_id, None);
        let cleared: UpdateWorkItemRequest = serde_json::from_value(serde_json::json!({
            "expectedVersion": 1,
            "assigneeActorId": null
        }))
        .expect("decode cleared assignee patch");
        assert_eq!(cleared.assignee_actor_id, Some(None));
    }

    #[test]
    fn worker_prompt_and_thread_name_are_bounded() {
        assert_eq!(
            normalize_worker_prompt("  make the change  ").expect("valid Worker prompt"),
            "make the change"
        );
        assert!(normalize_worker_prompt("\u{1b}").is_err());
        assert!(normalize_worker_prompt(&"a".repeat(18_001)).is_err());

        let name = worker_thread_name("AW-42", &"界".repeat(200), Uuid::now_v7());
        assert!(name.chars().count() <= 120);
        assert!(name.starts_with("AW-42: "));
    }
}
