use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

use super::McpPrincipal;
use crate::policy::{
    Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, Subject,
    authorize,
};

const MAX_INSTRUCTIONS_CHARACTERS: usize = 20_000;
const MAX_DESCRIPTION_CHARACTERS: usize = 20_000;
const MAX_SEARCH_CHARACTERS: usize = 120;
const DEFAULT_SEARCH_LIMIT: u16 = 25;
const MAX_SEARCH_LIMIT: u16 = 50;

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectInstructionsArguments {}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkItemGetArguments {
    pub identifier: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkItemSearchArguments {
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RepositoryGetArguments {
    pub repository_id: String,
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PullRequestGetArguments {
    pub identifier: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ReadResult<T> {
    Found { value: T },
    NotFound { retry_category: String },
    PermissionDenied { retry_category: String },
    InvalidRequest { retry_category: String },
    TemporarilyUnavailable { retry_category: String },
}

impl<T> ReadResult<T> {
    fn not_found() -> Self {
        Self::NotFound {
            retry_category: "do_not_retry".to_owned(),
        }
    }

    fn denied() -> Self {
        Self::PermissionDenied {
            retry_category: "reauthenticate_or_request_scope".to_owned(),
        }
    }

    fn invalid() -> Self {
        Self::InvalidRequest {
            retry_category: "correct_input".to_owned(),
        }
    }

    fn unavailable() -> Self {
        Self::TemporarilyUnavailable {
            retry_category: "retry_with_backoff".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectInstructionsProjection {
    pub project_id: String,
    pub content: String,
    pub version: i64,
    pub truncated: bool,
    pub canonical_url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemProjection {
    pub id: String,
    pub project_id: String,
    pub key: String,
    pub title: String,
    pub description: String,
    pub description_truncated: bool,
    pub item_type: NamedProjection,
    pub status: StatusProjection,
    pub priority: String,
    pub assignee: Option<AssigneeProjection>,
    pub version: i64,
    pub archived: bool,
    pub parent_work_item_id: Option<String>,
    pub blocked: Option<BlockedProjection>,
    pub acceptance_criteria: Vec<AcceptanceCriterionProjection>,
    pub linked_repositories: Vec<LinkedRepositoryProjection>,
    pub linked_pull_requests: Vec<PullRequestProjection>,
    pub canonical_url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemSummaryProjection {
    pub id: String,
    pub key: String,
    pub title: String,
    pub status: StatusProjection,
    pub priority: String,
    pub assignee: Option<AssigneeProjection>,
    pub version: i64,
    pub archived: bool,
    pub canonical_url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemSearchProjection {
    pub items: Vec<WorkItemSummaryProjection>,
    pub next_cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkItemContextProjection {
    pub project_instructions: ProjectInstructionsProjection,
    pub work_item: WorkItemProjection,
    pub source_versions: SourceVersions,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SourceVersions {
    pub work_item_version: i64,
    pub project_instructions_version: i64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct NamedProjection {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct StatusProjection {
    pub id: String,
    pub name: String,
    pub semantic_category: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssigneeProjection {
    pub actor_id: String,
    pub display_name: String,
    pub handle: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AcceptanceCriterionProjection {
    pub id: String,
    pub text: String,
    pub position: i32,
    pub completed: bool,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct BlockedProjection {
    pub reason: String,
    pub blocked_at: String,
    pub blocked_by_actor_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryProjection {
    pub id: String,
    pub project_id: String,
    pub owner: String,
    pub name: String,
    pub clone_url: String,
    pub default_branch: String,
    pub trusted: bool,
    pub enabled: bool,
    pub preparation_state: String,
    pub version: i64,
    pub archived: bool,
    pub canonical_url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkedRepositoryProjection {
    pub repository: RepositoryProjection,
    pub role: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PullRequestProjection {
    pub id: String,
    pub project_id: String,
    pub repository_id: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub head_ref: String,
    pub head_sha: String,
    pub base_ref: String,
    pub draft: bool,
    pub state: String,
    pub checks_state: String,
    pub review_state: String,
    pub mergeability_state: String,
    pub sync_state: String,
    pub version: i64,
    pub canonical_url: String,
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
    version: i64,
    archived: bool,
    parent_work_item_id: Option<Uuid>,
    blocked_reason: Option<String>,
    blocked_at: Option<String>,
    blocked_by_actor_id: Option<Uuid>,
}

#[derive(Debug, FromRow)]
struct InstructionsRow {
    content: String,
    version: i64,
}

#[derive(Debug, FromRow)]
struct AcceptanceCriterionRow {
    id: Uuid,
    text: String,
    position: i32,
    completed: bool,
}

#[derive(Debug, FromRow)]
struct RepositoryRow {
    id: Uuid,
    project_id: Uuid,
    owner: String,
    name: String,
    clone_url: String,
    default_branch: String,
    trusted: bool,
    enabled: bool,
    preparation_state: String,
    version: i64,
    archived: bool,
}

#[derive(Debug, FromRow)]
struct LinkedRepositoryRow {
    role: String,
    id: Uuid,
    project_id: Uuid,
    owner: String,
    name: String,
    clone_url: String,
    default_branch: String,
    trusted: bool,
    enabled: bool,
    preparation_state: String,
    version: i64,
    archived: bool,
}

#[derive(Debug, FromRow)]
struct PullRequestRow {
    id: Uuid,
    project_id: Uuid,
    repository_id: Uuid,
    pull_request_number: i64,
    title: String,
    html_url: String,
    head_ref: String,
    head_sha: String,
    base_ref: String,
    draft: bool,
    state: String,
    checks_state: String,
    review_state: String,
    mergeability_state: String,
    sync_state: String,
    version: i64,
}

pub async fn project_instructions_get(
    state: &crate::AppState,
    principal: &McpPrincipal,
) -> ReadResult<ProjectInstructionsProjection> {
    if !authorize_read(&state.pool, principal).await {
        return ReadResult::denied();
    }
    match load_instructions(&state.pool, principal.project_id).await {
        Ok(instructions) => ReadResult::Found {
            value: instructions_projection(state, principal.project_id, instructions),
        },
        Err(error) => {
            tracing::error!(error = %error, "MCP project_instructions_get failed");
            ReadResult::unavailable()
        }
    }
}

pub async fn work_item_get(
    state: &crate::AppState,
    principal: &McpPrincipal,
    identifier: &str,
) -> ReadResult<WorkItemProjection> {
    if !authorize_read(&state.pool, principal).await {
        return ReadResult::denied();
    }
    let Some(work_item_id) =
        resolve_work_item_id(&state.pool, principal.project_id, identifier).await
    else {
        return ReadResult::not_found();
    };
    match load_work_item(state, principal.project_id, work_item_id).await {
        Ok(Some(item)) => ReadResult::Found { value: item },
        Ok(None) => ReadResult::not_found(),
        Err(error) => {
            tracing::error!(error = %error, "MCP work_item_get failed");
            ReadResult::unavailable()
        }
    }
}

pub async fn work_item_search(
    state: &crate::AppState,
    principal: &McpPrincipal,
    arguments: WorkItemSearchArguments,
) -> ReadResult<WorkItemSearchProjection> {
    if !authorize_read(&state.pool, principal).await {
        return ReadResult::denied();
    }
    let query = arguments.query.unwrap_or_default();
    if query.chars().count() > MAX_SEARCH_CHARACTERS {
        return ReadResult::invalid();
    }
    let limit = arguments.limit.unwrap_or(DEFAULT_SEARCH_LIMIT);
    if !(1..=MAX_SEARCH_LIMIT).contains(&limit) {
        return ReadResult::invalid();
    }
    let cursor = match arguments.cursor.as_deref().map(decode_cursor).transpose() {
        Ok(cursor) => cursor.unwrap_or(0),
        Err(()) => return ReadResult::invalid(),
    };
    let rows = sqlx::query_as::<_, WorkItemRow>(WORK_ITEM_SELECT)
        .bind(principal.project_id)
        .bind(cursor)
        .bind(query.trim())
        .bind(i64::from(limit) + 1)
        .fetch_all(&state.pool)
        .await;
    let mut rows = match rows {
        Ok(rows) => rows,
        Err(error) => {
            tracing::error!(error = %error, "MCP work_item_search failed");
            return ReadResult::unavailable();
        }
    };
    let truncated = rows.len() > usize::from(limit);
    if truncated {
        rows.pop();
    }
    let next_cursor = truncated
        .then(|| rows.last().map(|row| encode_cursor(row.number)))
        .flatten();
    let items = rows
        .into_iter()
        .map(|row| summary_projection(state, row))
        .collect();
    ReadResult::Found {
        value: WorkItemSearchProjection {
            items,
            next_cursor,
            truncated,
        },
    }
}

pub async fn work_item_context_get(
    state: &crate::AppState,
    principal: &McpPrincipal,
    identifier: &str,
) -> ReadResult<WorkItemContextProjection> {
    if !authorize_read(&state.pool, principal).await {
        return ReadResult::denied();
    }
    let Some(work_item_id) =
        resolve_work_item_id(&state.pool, principal.project_id, identifier).await
    else {
        return ReadResult::not_found();
    };
    let (item, instructions) = tokio::join!(
        load_work_item(state, principal.project_id, work_item_id),
        load_instructions(&state.pool, principal.project_id)
    );
    match (item, instructions) {
        (Ok(Some(item)), Ok(instructions)) => {
            let instructions = instructions_projection(state, principal.project_id, instructions);
            let versions = SourceVersions {
                work_item_version: item.version,
                project_instructions_version: instructions.version,
            };
            ReadResult::Found {
                value: WorkItemContextProjection {
                    project_instructions: instructions,
                    work_item: item,
                    source_versions: versions,
                },
            }
        }
        (Ok(None), _) => ReadResult::not_found(),
        (Err(error), _) | (_, Err(error)) => {
            tracing::error!(error = %error, "MCP work_item_context_get failed");
            ReadResult::unavailable()
        }
    }
}

pub async fn repository_get(
    state: &crate::AppState,
    principal: &McpPrincipal,
    repository_id: &str,
) -> ReadResult<RepositoryProjection> {
    if !authorize_read(&state.pool, principal).await {
        return ReadResult::denied();
    }
    let Ok(repository_id) = repository_id.parse::<Uuid>() else {
        return ReadResult::invalid();
    };
    match load_repository(&state.pool, principal.project_id, repository_id).await {
        Ok(Some(row)) => ReadResult::Found {
            value: repository_projection(state, row),
        },
        Ok(None) => ReadResult::not_found(),
        Err(error) => {
            tracing::error!(error = %error, "MCP repository_get failed");
            ReadResult::unavailable()
        }
    }
}

pub async fn pull_request_get(
    state: &crate::AppState,
    principal: &McpPrincipal,
    identifier: &str,
) -> ReadResult<PullRequestProjection> {
    if !authorize_read(&state.pool, principal).await {
        return ReadResult::denied();
    }
    if identifier.is_empty() || identifier.len() > 2048 {
        return ReadResult::invalid();
    }
    let id = identifier.parse::<Uuid>().ok();
    let row = sqlx::query_as::<_, PullRequestRow>(
        r#"
        SELECT
            id, project_id, repository_id, pull_request_number, title, html_url,
            head_ref, head_sha, base_ref, draft, state, checks_state, review_state,
            mergeability_state, sync_state, version
        FROM github_pull_requests
        WHERE project_id = $1
          AND (($2::uuid IS NOT NULL AND id = $2) OR html_url = $3)
        "#,
    )
    .bind(principal.project_id)
    .bind(id)
    .bind(identifier)
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some(row)) => ReadResult::Found {
            value: pull_request_projection(state, row),
        },
        Ok(None) => ReadResult::not_found(),
        Err(error) => {
            tracing::error!(error = %error, "MCP pull_request_get failed");
            ReadResult::unavailable()
        }
    }
}

async fn authorize_read(pool: &sqlx::PgPool, principal: &McpPrincipal) -> bool {
    if !principal.can_read() {
        return false;
    }
    let role = sqlx::query_scalar::<_, String>(
        r#"
        SELECT pm.role
        FROM project_memberships pm
        JOIN projects p ON p.id = pm.project_id
        JOIN organization_memberships om
          ON om.organization_id = p.organization_id
         AND om.actor_id = pm.actor_id
         AND om.enabled
        JOIN actors a ON a.id = pm.actor_id
        JOIN users u ON u.actor_id = pm.actor_id AND u.status = 'active'
        WHERE pm.project_id = $1
          AND pm.actor_id = $2
          AND pm.enabled
          AND p.archived_at IS NULL
          AND a.kind = 'human'
          AND a.status = 'active'
        "#,
    )
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some(role) = role.and_then(|role| parse_membership_role(&role)) else {
        return false;
    };
    matches!(
        authorize(
            &Subject::new(
                principal.actor_id,
                ActorKind::Human,
                true,
                Some(Membership::project(principal.project_id, role, true)),
                None,
            ),
            Action::McpRead,
            Resource::Project(ProjectScope::project(principal.project_id)),
        ),
        Decision::Allow
    )
}

fn parse_membership_role(role: &str) -> Option<MembershipRole> {
    match role {
        "owner" => Some(MembershipRole::Owner),
        "admin" => Some(MembershipRole::Admin),
        "member" => Some(MembershipRole::Member),
        "viewer" => Some(MembershipRole::Viewer),
        "worker" => Some(MembershipRole::Worker),
        _ => None,
    }
}

async fn load_instructions(
    pool: &sqlx::PgPool,
    project_id: Uuid,
) -> Result<InstructionsRow, sqlx::Error> {
    sqlx::query_as::<_, InstructionsRow>(
        "SELECT content, version FROM project_instructions WHERE project_id = $1",
    )
    .bind(project_id)
    .fetch_optional(pool)
    .await
    .map(|row| {
        row.unwrap_or(InstructionsRow {
            content: String::new(),
            version: 1,
        })
    })
}

fn instructions_projection(
    state: &crate::AppState,
    project_id: Uuid,
    row: InstructionsRow,
) -> ProjectInstructionsProjection {
    let (content, truncated) = truncate(&row.content, MAX_INSTRUCTIONS_CHARACTERS);
    ProjectInstructionsProjection {
        project_id: project_id.to_string(),
        content,
        version: row.version,
        truncated,
        canonical_url: canonical_url(state, &format!("/projects/{project_id}/settings")),
    }
}

async fn resolve_work_item_id(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    identifier: &str,
) -> Option<Uuid> {
    if identifier.is_empty() || identifier.len() > 2048 || identifier.chars().any(char::is_control)
    {
        return None;
    }
    if let Ok(id) = identifier.parse::<Uuid>() {
        return Some(id);
    }
    if identifier.starts_with('/')
        || identifier.starts_with("http://")
        || identifier.starts_with("https://")
    {
        return identifier
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .and_then(|segment| segment.parse::<Uuid>().ok());
    }
    sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT wi.id
        FROM work_items wi
        JOIN projects p ON p.id = wi.project_id
        WHERE wi.project_id = $1
          AND upper(p.project_key || '-' || wi.number::text) = upper($2)
        "#,
    )
    .bind(project_id)
    .bind(identifier)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

async fn load_work_item(
    state: &crate::AppState,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<WorkItemProjection>, sqlx::Error> {
    let row = sqlx::query_as::<_, WorkItemRow>(WORK_ITEM_BY_ID_SELECT)
        .bind(project_id)
        .bind(work_item_id)
        .fetch_optional(&state.pool)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let (criteria, repositories, pull_requests) = tokio::try_join!(
        load_acceptance_criteria(&state.pool, project_id, work_item_id),
        load_linked_repositories(&state.pool, project_id, work_item_id),
        load_linked_pull_requests(&state.pool, project_id, work_item_id),
    )?;
    Ok(Some(work_item_projection(
        state,
        row,
        criteria,
        repositories,
        pull_requests,
    )))
}

async fn load_acceptance_criteria(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<AcceptanceCriterionProjection>, sqlx::Error> {
    let rows = sqlx::query_as::<_, AcceptanceCriterionRow>(
        r#"
        SELECT id, text, position, completed
        FROM work_item_acceptance_criteria
        WHERE project_id = $1 AND work_item_id = $2
        ORDER BY position, id
        LIMIT 100
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| AcceptanceCriterionProjection {
            id: row.id.to_string(),
            text: row.text,
            position: row.position,
            completed: row.completed,
        })
        .collect())
}

async fn load_repository(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    repository_id: Uuid,
) -> Result<Option<RepositoryRow>, sqlx::Error> {
    sqlx::query_as::<_, RepositoryRow>(REPOSITORY_SELECT)
        .bind(project_id)
        .bind(repository_id)
        .fetch_optional(pool)
        .await
}

async fn load_linked_repositories(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<LinkedRepositoryRow>, sqlx::Error> {
    sqlx::query_as::<_, LinkedRepositoryRow>(
        r#"
        SELECT
            link.role,
            repository.id,
            repository.project_id,
            repository.owner,
            repository.name,
            repository.clone_url,
            repository.default_branch,
            repository.trusted,
            repository.enabled,
            repository.preparation_state,
            repository.version,
            repository.archived_at IS NOT NULL AS archived
        FROM work_item_repository_links link
        JOIN repositories repository
          ON repository.project_id = link.project_id AND repository.id = link.repository_id
        WHERE link.project_id = $1 AND link.work_item_id = $2
        ORDER BY (link.role = 'primary') DESC, repository.owner, repository.name, repository.id
        LIMIT 20
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(pool)
    .await
}

async fn load_linked_pull_requests(
    pool: &sqlx::PgPool,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Vec<PullRequestRow>, sqlx::Error> {
    sqlx::query_as::<_, PullRequestRow>(
        r#"
        SELECT
            pull_request.id,
            pull_request.project_id,
            pull_request.repository_id,
            pull_request.pull_request_number,
            pull_request.title,
            pull_request.html_url,
            pull_request.head_ref,
            pull_request.head_sha,
            pull_request.base_ref,
            pull_request.draft,
            pull_request.state,
            pull_request.checks_state,
            pull_request.review_state,
            pull_request.mergeability_state,
            pull_request.sync_state,
            pull_request.version
        FROM github_pull_request_work_item_links link
        JOIN github_pull_requests pull_request
          ON pull_request.project_id = link.project_id
         AND pull_request.id = link.github_pull_request_id
        WHERE link.project_id = $1 AND link.work_item_id = $2 AND link.confirmed
        ORDER BY pull_request.external_updated_at DESC, pull_request.id DESC
        LIMIT 20
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(pool)
    .await
}

fn work_item_projection(
    state: &crate::AppState,
    row: WorkItemRow,
    acceptance_criteria: Vec<AcceptanceCriterionProjection>,
    linked_repositories: Vec<LinkedRepositoryRow>,
    linked_pull_requests: Vec<PullRequestRow>,
) -> WorkItemProjection {
    let (description, description_truncated) =
        truncate(&row.description, MAX_DESCRIPTION_CHARACTERS);
    let assignee = assignee_projection(&row);
    WorkItemProjection {
        id: row.id.to_string(),
        project_id: row.project_id.to_string(),
        key: format!("{}-{}", row.project_key, row.number),
        title: row.title,
        description,
        description_truncated,
        item_type: NamedProjection {
            id: row.type_id.to_string(),
            name: row.type_name,
        },
        status: StatusProjection {
            id: row.status_id.to_string(),
            name: row.status_name,
            semantic_category: row.semantic_category,
        },
        priority: row.priority,
        assignee,
        version: row.version,
        archived: row.archived,
        parent_work_item_id: row.parent_work_item_id.map(|id| id.to_string()),
        blocked: match (row.blocked_reason, row.blocked_at, row.blocked_by_actor_id) {
            (Some(reason), Some(blocked_at), Some(blocked_by_actor_id)) => {
                Some(BlockedProjection {
                    reason,
                    blocked_at,
                    blocked_by_actor_id: blocked_by_actor_id.to_string(),
                })
            }
            _ => None,
        },
        acceptance_criteria,
        linked_repositories: linked_repositories
            .into_iter()
            .map(|row| LinkedRepositoryProjection {
                role: row.role,
                repository: repository_projection(
                    state,
                    RepositoryRow {
                        id: row.id,
                        project_id: row.project_id,
                        owner: row.owner,
                        name: row.name,
                        clone_url: row.clone_url,
                        default_branch: row.default_branch,
                        trusted: row.trusted,
                        enabled: row.enabled,
                        preparation_state: row.preparation_state,
                        version: row.version,
                        archived: row.archived,
                    },
                ),
            })
            .collect(),
        linked_pull_requests: linked_pull_requests
            .into_iter()
            .map(|row| pull_request_projection(state, row))
            .collect(),
        canonical_url: canonical_url(
            state,
            &format!("/projects/{}/work-items/{}", row.project_id, row.id),
        ),
    }
}

fn summary_projection(state: &crate::AppState, row: WorkItemRow) -> WorkItemSummaryProjection {
    let assignee = assignee_projection(&row);
    WorkItemSummaryProjection {
        id: row.id.to_string(),
        key: format!("{}-{}", row.project_key, row.number),
        title: row.title,
        status: StatusProjection {
            id: row.status_id.to_string(),
            name: row.status_name,
            semantic_category: row.semantic_category,
        },
        priority: row.priority,
        assignee,
        version: row.version,
        archived: row.archived,
        canonical_url: canonical_url(
            state,
            &format!("/projects/{}/work-items/{}", row.project_id, row.id),
        ),
    }
}

fn assignee_projection(row: &WorkItemRow) -> Option<AssigneeProjection> {
    Some(AssigneeProjection {
        actor_id: row.assignee_actor_id?.to_string(),
        display_name: row.assignee_display_name.clone()?,
        handle: row.assignee_handle.clone()?,
    })
}

fn repository_projection(state: &crate::AppState, row: RepositoryRow) -> RepositoryProjection {
    RepositoryProjection {
        id: row.id.to_string(),
        project_id: row.project_id.to_string(),
        owner: row.owner,
        name: row.name,
        clone_url: row.clone_url,
        default_branch: row.default_branch,
        trusted: row.trusted,
        enabled: row.enabled,
        preparation_state: row.preparation_state,
        version: row.version,
        archived: row.archived,
        canonical_url: canonical_url(
            state,
            &format!(
                "/projects/{}/settings/repositories/{}",
                row.project_id, row.id
            ),
        ),
    }
}

fn pull_request_projection(state: &crate::AppState, row: PullRequestRow) -> PullRequestProjection {
    PullRequestProjection {
        id: row.id.to_string(),
        project_id: row.project_id.to_string(),
        repository_id: row.repository_id.to_string(),
        number: row.pull_request_number,
        title: row.title,
        url: row.html_url,
        head_ref: row.head_ref,
        head_sha: row.head_sha,
        base_ref: row.base_ref,
        draft: row.draft,
        state: row.state,
        checks_state: row.checks_state,
        review_state: row.review_state,
        mergeability_state: row.mergeability_state,
        sync_state: row.sync_state,
        version: row.version,
        canonical_url: canonical_url(
            state,
            &format!("/projects/{}/pull-requests/{}", row.project_id, row.id),
        ),
    }
}

fn canonical_url(state: &crate::AppState, path: &str) -> String {
    format!("http://{}{path}", state.config.server_addr)
}

fn truncate(value: &str, max_characters: usize) -> (String, bool) {
    let mut characters = value.chars();
    let truncated = characters.clone().count() > max_characters;
    let value = characters.by_ref().take(max_characters).collect();
    (value, truncated)
}

fn encode_cursor(number: i64) -> String {
    URL_SAFE_NO_PAD.encode(number.to_be_bytes())
}

fn decode_cursor(cursor: &str) -> Result<i64, ()> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| ())?;
    let bytes: [u8; 8] = bytes.try_into().map_err(|_| ())?;
    let number = i64::from_be_bytes(bytes);
    (number >= 0).then_some(number).ok_or(())
}

const WORK_ITEM_BY_ID_SELECT: &str = r#"
    SELECT
        wi.id,
        wi.project_id,
        p.project_key,
        wi.number,
        wi.title,
        wi.description,
        wi.type_id,
        item_type.name AS type_name,
        wi.status_id,
        status.name AS status_name,
        status.semantic_category,
        wi.priority,
        wi.assignee_actor_id,
        assignee.display_name AS assignee_display_name,
        assignee.handle AS assignee_handle,
        wi.version,
        wi.archived_at IS NOT NULL AS archived,
        wi.parent_work_item_id,
        wi.blocked_reason,
        wi.blocked_at::text AS blocked_at,
        wi.blocked_by_actor_id
    FROM work_items wi
    JOIN projects p ON p.id = wi.project_id
    JOIN work_item_types item_type
      ON item_type.project_id = wi.project_id AND item_type.id = wi.type_id
    JOIN work_item_statuses status
      ON status.project_id = wi.project_id AND status.id = wi.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = wi.project_id AND assignee.actor_id = wi.assignee_actor_id
    WHERE wi.project_id = $1 AND wi.id = $2
"#;

const WORK_ITEM_SELECT: &str = r#"
    SELECT
        wi.id,
        wi.project_id,
        p.project_key,
        wi.number,
        wi.title,
        wi.description,
        wi.type_id,
        item_type.name AS type_name,
        wi.status_id,
        status.name AS status_name,
        status.semantic_category,
        wi.priority,
        wi.assignee_actor_id,
        assignee.display_name AS assignee_display_name,
        assignee.handle AS assignee_handle,
        wi.version,
        wi.archived_at IS NOT NULL AS archived,
        wi.parent_work_item_id,
        wi.blocked_reason,
        wi.blocked_at::text AS blocked_at,
        wi.blocked_by_actor_id
    FROM work_items wi
    JOIN projects p ON p.id = wi.project_id
    JOIN work_item_types item_type
      ON item_type.project_id = wi.project_id AND item_type.id = wi.type_id
    JOIN work_item_statuses status
      ON status.project_id = wi.project_id AND status.id = wi.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = wi.project_id AND assignee.actor_id = wi.assignee_actor_id
    WHERE wi.project_id = $1
      AND wi.number > $2
      AND (
          $3 = ''
          OR wi.title ILIKE '%' || $3 || '%'
          OR upper(p.project_key || '-' || wi.number::text) = upper($3)
      )
    ORDER BY wi.number, wi.id
    LIMIT $4
"#;

const REPOSITORY_SELECT: &str = r#"
    SELECT
        id,
        project_id,
        owner,
        name,
        clone_url,
        default_branch,
        trusted,
        enabled,
        preparation_state,
        version,
        archived_at IS NOT NULL AS archived
    FROM repositories
    WHERE project_id = $1 AND id = $2
"#;

#[cfg(test)]
mod tests {
    use super::{decode_cursor, encode_cursor, truncate};

    #[test]
    fn cursors_are_exact_and_bounded() {
        let cursor = encode_cursor(42);
        assert_eq!(decode_cursor(&cursor), Ok(42));
        assert_eq!(decode_cursor("hostile"), Err(()));
    }

    #[test]
    fn truncation_preserves_utf8_character_boundaries() {
        assert_eq!(truncate("a🦀b", 2), ("a🦀".to_owned(), true));
        assert_eq!(truncate("a🦀", 2), ("a🦀".to_owned(), false));
    }
}
