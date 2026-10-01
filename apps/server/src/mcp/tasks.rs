//! Task tools: list and open tasks, put them in order, claim them before
//! working on them, and link them to the conversations, pages and decisions
//! they came from. Creating and changing tasks reuses the canonical commands
//! in `writes.rs`.

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ToolResult, accessible_project_ids, mark_via, parse_id, project_is_chosen, public_url,
        scope_projects, snippet,
    },
    reads::{self, ReadResult, WorkItemProjection},
    talk::can_write_in,
    writes::{self, WriteResult},
};
use crate::{
    AppState,
    error::AppError,
    policy::Action,
    work_item_commands::{
        BOARD_RANK_STEP, CommandActor, enqueue_update, insert_activity, record_audit,
        validate_assignee,
    },
};

const DEFAULT_CLAIM_MINUTES: i32 = 120;

// ---------------------------------------------------------------------------
// task_list

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskListArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// Only this project. Leave it out for tasks across all your projects.
    pub project_id: Option<String>,
    /// A status name (such as "In Progress") or group: backlog, unstarted,
    /// started, review, verification, completed, canceled.
    pub status: Option<String>,
    /// "me", "none", an actor ID, or an @handle.
    pub assignee: Option<String>,
    /// Words in the title or description, or a key such as AW-12.
    pub query: Option<String>,
    /// Only subtasks of this task.
    pub parent_id: Option<String>,
    /// board (the order people set, the default), priority, updated, or number.
    pub sort: Option<String>,
    /// Include completed and canceled tasks.
    pub include_done: Option<bool>,
    /// How many (default 50, at most 200).
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskSummary {
    pub id: String,
    pub key: String,
    pub title: String,
    pub status: String,
    pub status_group: String,
    pub priority: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// 1 is the top of its status column on the board.
    pub board_position: i64,
    pub blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    pub project: String,
    pub version: i64,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskListValue {
    pub tasks: Vec<TaskSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

pub(super) async fn task_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskListArguments,
) -> ToolResult<TaskListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let limit = i64::from(arguments.limit.unwrap_or(50));
    if !(1..=200).contains(&limit) {
        return ToolResult::invalid("limit", "Use a limit from 1 to 200.");
    }
    let offset = match arguments
        .cursor
        .as_deref()
        .map(str::parse::<i64>)
        .transpose()
    {
        Ok(offset) => offset.unwrap_or(0).max(0),
        Err(_) => return ToolResult::invalid("cursor", "Pass the nextCursor you were given."),
    };
    let order = match arguments.sort.as_deref().unwrap_or("board") {
        "board" => "project.name, status.position, item.board_rank, item.id",
        "priority" => {
            "CASE item.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END, status.position, item.board_rank, item.id"
        }
        "updated" => "item.updated_at DESC, item.id",
        "number" => "project.name, item.number",
        _ => {
            return ToolResult::invalid("sort", "Use board, priority, updated, or number.");
        }
    };
    let parent_id = match arguments.parent_id.as_deref() {
        Some(value) => match parse_id(value) {
            Some(id) => Some(id),
            None => return ToolResult::invalid("parentId", "Pass the parent task's ID."),
        },
        None => None,
    };
    let result = async {
        let projects = scope_projects(state, principal).await?;
        let (assignee_mode, assignee_id) = match arguments.assignee.as_deref().map(str::trim) {
            None | Some("") => ("any", None),
            Some("me") => ("one", Some(principal.actor_id)),
            Some("none") => ("none", None),
            Some(person) => match resolve_actor(state, &projects, person).await? {
                Some(id) => ("one", Some(id)),
                None => {
                    return Ok(ToolResult::invalid(
                        "assignee",
                        format!("No one called {person} was found."),
                    ));
                }
            },
        };
        let status = arguments
            .status
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let query = arguments.query.clone().unwrap_or_default();
        // Board positions count every task in a column, so they're numbered
        // before the filters and the page apply.
        let mut tasks = sqlx::query_as::<_, TaskSummary>(&format!(
            r#"
            WITH numbered AS (
                SELECT item.id,
                       row_number() OVER (PARTITION BY item.project_id, item.status_id
                                          ORDER BY item.board_rank, item.id) AS board_position
                FROM work_items item
                WHERE item.project_id = ANY($1) AND item.archived_at IS NULL
            )
            SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
                   item.title, status.name AS status, status.semantic_category AS status_group,
                   item.priority, assignee.display_name AS assignee,
                   numbered.board_position,
                   item.blocked_reason IS NOT NULL AS blocked,
                   claimer.display_name AS claimed_by, project.name AS project,
                   item.version, item.updated_at::text AS updated_at, item.project_id
            FROM numbered
            JOIN work_items item ON item.id = numbered.id
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            LEFT JOIN project_memberships assignee
              ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
            LEFT JOIN work_item_claims claim
              ON claim.work_item_id = item.id AND claim.expires_at > now()
            LEFT JOIN project_memberships claimer
              ON claimer.project_id = item.project_id AND claimer.actor_id = claim.actor_id
            WHERE ($2 OR status.semantic_category NOT IN ('completed', 'canceled'))
              AND ($3::text IS NULL OR lower(status.name) = lower($3)
                   OR status.semantic_category = lower($3))
              AND ($4 = 'any' OR ($4 = 'none' AND item.assignee_actor_id IS NULL)
                   OR ($4 = 'one' AND item.assignee_actor_id = $5))
              AND ($6 = '' OR item.search_vector @@ websearch_to_tsquery('english', $6)
                   OR item.title ILIKE '%' || $6 || '%'
                   OR upper(project.project_key || '-' || item.number::text) = upper($6))
              AND ($7::uuid IS NULL OR item.parent_work_item_id = $7)
            ORDER BY {order}
            LIMIT $8 OFFSET $9
            "#
        ))
        .bind(&projects)
        .bind(arguments.include_done.unwrap_or(false))
        .bind(status)
        .bind(assignee_mode)
        .bind(assignee_id)
        .bind(query.trim())
        .bind(parent_id)
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&state.pool)
        .await?;
        let has_more = i64::try_from(tasks.len()).unwrap_or(i64::MAX) > limit;
        tasks.truncate(usize::try_from(limit).unwrap_or(200));
        Ok::<_, AppError>(ToolResult::ok(TaskListValue {
            tasks,
            next_cursor: has_more.then(|| (offset + limit).to_string()),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_list"))
}

async fn resolve_actor(
    state: &AppState,
    projects: &[Uuid],
    person: &str,
) -> Result<Option<Uuid>, AppError> {
    if let Ok(id) = person.parse::<Uuid>() {
        return Ok(Some(id));
    }
    Ok(sqlx::query_scalar::<_, Uuid>(
        "SELECT actor_id FROM project_memberships WHERE project_id = ANY($1) AND lower(handle) = lower($2) AND enabled LIMIT 1",
    )
    .bind(projects)
    .bind(person.trim_start_matches('@'))
    .fetch_optional(&state.pool)
    .await?)
}

/// Find a task by ID, link, or key (such as AW-12) among the caller's
/// projects. Returns the task and its project. A moved task's old key leads
/// to where it went when the caller can reach that project; anyone else
/// gets the archived original they could always see.
pub(super) async fn resolve_task(
    state: &AppState,
    principal: &McpPrincipal,
    identifier: &str,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    let projects = accessible_project_ids(state, principal).await?;
    resolve_among(state, principal, &projects, identifier).await
}

/// `resolve_task` for several references, in order, reading the caller's
/// projects once.
#[allow(dead_code)] // The batch tools resolve their task lists with it next.
pub(super) async fn resolve_tasks(
    state: &AppState,
    principal: &McpPrincipal,
    references: &[String],
) -> Result<Vec<Option<(Uuid, Uuid)>>, AppError> {
    let projects = accessible_project_ids(state, principal).await?;
    let mut found = Vec::with_capacity(references.len());
    for reference in references {
        found.push(resolve_among(state, principal, &projects, reference).await?);
    }
    Ok(found)
}

async fn resolve_among(
    state: &AppState,
    principal: &McpPrincipal,
    projects: &[Uuid],
    identifier: &str,
) -> Result<Option<(Uuid, Uuid)>, AppError> {
    let reachable = |id: Uuid| {
        sqlx::query_as::<_, (Uuid, Uuid)>(
            "SELECT id, project_id FROM work_items WHERE id = $1 AND project_id = ANY($2)",
        )
        .bind(id)
        .bind(projects)
        .fetch_optional(&state.pool)
    };
    if let Some(id) = parse_id(identifier) {
        return Ok(reachable(id).await?);
    }
    let Some((project_key, number)) = super::split_task_key(identifier) else {
        return Ok(None);
    };
    // task_by_key also matches projects the caller belongs to beyond this
    // connection's reach, so its answer is checked against `projects`.
    if let Some(id) = super::task_by_key(state, principal, project_key, number).await?
        && let Some(found) = reachable(id).await?
    {
        return Ok(Some(found));
    }
    Ok(sqlx::query_as::<_, (Uuid, Uuid)>(
        r#"
        SELECT item.id, item.project_id
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        WHERE item.project_id = ANY($3)
          AND upper(project.project_key) = upper($1) AND item.number = $2
        LIMIT 1
        "#,
    )
    .bind(project_key)
    .bind(number)
    .bind(projects)
    .fetch_optional(&state.pool)
    .await?)
}

/// Why an assignee can't be used.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum AssigneeProblem {
    /// Nobody by that handle or ID is in the project.
    NotInProject(String),
    /// They're in the project but can't hold tasks (inactive, or not a
    /// person or agent).
    CantBeAssigned(String),
}

impl AssigneeProblem {
    pub(super) fn message(&self) -> &str {
        match self {
            Self::NotInProject(message) | Self::CantBeAssigned(message) => message,
        }
    }
}

/// An assignee as agents write one: "me", "none" (nobody), an @handle or an
/// actor ID of someone in the project who can hold tasks. `Ok(None)` means
/// unassigned.
pub(super) async fn resolve_assignee(
    connection: &mut PgConnection,
    principal: &McpPrincipal,
    project_id: Uuid,
    value: &str,
) -> Result<Result<Option<Uuid>, AssigneeProblem>, AppError> {
    let value = value.trim();
    match value.to_lowercase().as_str() {
        "me" => return Ok(Ok(Some(principal.actor_id))),
        "none" => return Ok(Ok(None)),
        _ => {}
    }
    let found = sqlx::query_as::<_, (Uuid, String, String)>(
        r#"
        SELECT membership.actor_id, membership.display_name, project.project_key
        FROM project_memberships membership
        JOIN projects project ON project.id = membership.project_id
        WHERE membership.project_id = $1 AND membership.enabled
          AND (membership.actor_id = $2 OR lower(membership.handle) = lower($3))
        LIMIT 1
        "#,
    )
    .bind(project_id)
    .bind(value.parse::<Uuid>().ok())
    .bind(value.trim_start_matches('@'))
    .fetch_optional(&mut *connection)
    .await?;
    let Some((actor_id, name, project_key)) = found else {
        let project_key =
            sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
                .bind(project_id)
                .fetch_optional(&mut *connection)
                .await?
                .unwrap_or_else(|| "that project".to_owned());
        return Ok(Err(AssigneeProblem::NotInProject(format!(
            "No one called {value} is in {project_key}. Find people with people_search."
        ))));
    };
    if !validate_assignee(connection, project_id, Some(actor_id)).await? {
        return Ok(Err(AssigneeProblem::CantBeAssigned(format!(
            "{name} can't be assigned tasks in {project_key}: only active people and agents in the project can."
        ))));
    }
    Ok(Ok(Some(actor_id)))
}

/// The two shapes task_create and task_update have always given for an
/// assignee they can't use: an unknown handle names the person in
/// retryCategory, and an ID that can't be assigned is a correct_input.
fn assignee_error(person: &str, problem: AssigneeProblem) -> WriteResult {
    match problem {
        AssigneeProblem::NotInProject(message) if person.parse::<Uuid>().is_err() => {
            WriteResult::InvalidRequest {
                field: "assignee".to_owned(),
                retry_category: format!("No one called {person} is in that project."),
                message: Some(message),
            }
        }
        problem => WriteResult::invalid("assigneeActorId", problem.message()),
    }
}

pub(super) fn scoped(principal: &McpPrincipal, project_id: Uuid) -> McpPrincipal {
    let mut scoped = principal.clone();
    scoped.project_id = project_id;
    scoped.project_explicit = true;
    scoped
}

// ---------------------------------------------------------------------------
// task_open

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskOpenArguments {
    /// The task's ID, link, or key such as AW-12.
    #[serde(alias = "identifier")]
    pub task_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskDetail {
    pub task: WorkItemProjection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<ClaimValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<TaskSummary>,
    /// Threads and conversations about this task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conversations: Vec<LinkedItem>,
    /// Messages, pages and decisions linked to this task.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<LinkedItem>,
    /// The latest comments and updates, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub comments: Vec<CommentItem>,
    /// Where to read how this project works: project_open or page_get.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guide_page_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct LinkedItem {
    pub kind: String,
    pub id: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub text: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema, FromRow)]
#[serde(rename_all = "camelCase")]
pub(super) struct CommentItem {
    pub author: String,
    pub text: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

pub(super) async fn task_open(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskOpenArguments,
) -> ToolResult<TaskDetail> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    let result = async {
        let Some((task_id, project_id)) = resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        let scoped = scoped(principal, project_id);
        let task = match reads::work_item_get(state, &scoped, &task_id.to_string()).await {
            ReadResult::Found { value } => value,
            ReadResult::NotFound { .. } => return Ok(ToolResult::not_found("That task")),
            ReadResult::PermissionDenied { .. } => {
                return Ok(ToolResult::denied("You can't read tasks in that project."));
            }
            _ => return Ok(ToolResult::unavailable()),
        };
        let claimed_by = current_claim(state, task_id).await?;
        let subtasks = sqlx::query_as::<_, TaskSummary>(
            r#"
            SELECT item.id::text AS id, project.project_key || '-' || item.number::text AS key,
                   item.title, status.name AS status, status.semantic_category AS status_group,
                   item.priority, assignee.display_name AS assignee, 0::bigint AS board_position,
                   item.blocked_reason IS NOT NULL AS blocked, NULL::text AS claimed_by,
                   project.name AS project, item.version, item.updated_at::text AS updated_at,
                   item.project_id
            FROM work_items item
            JOIN projects project ON project.id = item.project_id
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            LEFT JOIN project_memberships assignee
              ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
            WHERE item.parent_work_item_id = $1 AND item.archived_at IS NULL
            ORDER BY item.number
            LIMIT 50
            "#,
        )
        .bind(task_id)
        .fetch_all(&state.pool)
        .await?;
        let conversations = sqlx::query_as::<_, LinkedItem>(
            r#"
            SELECT 'conversation' AS kind, conversation.id::text AS id,
                   CASE WHEN conversation.kind = 'dm' THEN 'DM' ELSE conversation.name END AS title,
                   CASE WHEN conversation.kind = 'thread' THEN conversation.thread_status END AS status,
                   COALESCE(conversation.thread_outcome, '') AS text
            FROM (
                SELECT conversation_id AS id FROM work_item_conversation_links WHERE work_item_id = $1
                UNION
                SELECT target_id FROM work_item_links
                WHERE work_item_id = $1 AND target_kind = 'conversation'
            ) linked
            JOIN conversations conversation ON conversation.id = linked.id
            WHERE conversation.archived_at IS NULL
              AND conversation.project_id = $3
              AND conversation_visible_to(conversation.id, $2)
            ORDER BY conversation.created_at
            "#,
        )
        .bind(task_id)
        .bind(principal.actor_id)
        .bind(project_id)
        .fetch_all(&state.pool)
        .await?;
        let links = sqlx::query_as::<_, LinkedItem>(
            r#"
            SELECT link.target_kind AS kind, link.target_id::text AS id,
                   COALESCE(page.title, decision.title,
                            'Message from ' || COALESCE(author.display_name, 'someone')) AS title,
                   decision.status AS status,
                   left(COALESCE(page.body, decision.detail, message.plain_text, ''), 400) AS text
            FROM work_item_links link
            LEFT JOIN project_pages page
              ON link.target_kind = 'page' AND page.id = link.target_id AND page.archived_at IS NULL
             AND page.project_id = link.project_id
            LEFT JOIN project_decisions decision
              ON link.target_kind = 'decision' AND decision.id = link.target_id
             AND decision.project_id = link.project_id
             AND (decision.source_conversation_id IS NULL
                  OR conversation_visible_to(decision.source_conversation_id, $2))
            LEFT JOIN messages message
              ON link.target_kind = 'message' AND message.id = link.target_id
             AND message.project_id = link.project_id
             AND conversation_visible_to(message.conversation_id, $2)
            LEFT JOIN project_memberships author
              ON author.project_id = message.project_id AND author.actor_id = message.author_actor_id
            WHERE link.work_item_id = $1 AND link.target_kind <> 'conversation'
              AND (page.id IS NOT NULL OR decision.id IS NOT NULL OR message.id IS NOT NULL)
            ORDER BY link.created_at
            "#,
        )
        .bind(task_id)
        .bind(principal.actor_id)
        .fetch_all(&state.pool)
        .await?;
        let mut comments = sqlx::query_as::<_, CommentItem>(
            r#"
            SELECT COALESCE(author.display_name, 'Former member') AS author,
                   comment.plain_text AS text, comment.created_at::text AS at,
                   via_client.client_name AS via
            FROM work_item_comments comment
            LEFT JOIN project_memberships author
              ON author.project_id = comment.project_id AND author.actor_id = comment.author_actor_id
            LEFT JOIN mcp_oauth_grants via_grant
              ON via_grant.id = comment.via_oauth_grant_id AND via_grant.show_via
            LEFT JOIN mcp_oauth_clients via_client ON via_client.id = via_grant.client_id
            WHERE comment.work_item_id = $1
            ORDER BY comment.created_at DESC
            LIMIT 10
            "#,
        )
        .bind(task_id)
        .fetch_all(&state.pool)
        .await?;
        comments.reverse();
        let guide_page_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM project_pages WHERE project_id = $1 AND is_guide AND archived_at IS NULL",
        )
        .bind(project_id)
        .fetch_optional(&state.pool)
        .await?
        .map(|id| id.to_string());
        Ok::<_, AppError>(ToolResult::ok(TaskDetail {
            task,
            claimed_by,
            subtasks,
            conversations,
            links,
            comments,
            guide_page_id,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_open"))
}

// ---------------------------------------------------------------------------
// task_create and task_update, over the canonical commands

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskCreateArguments {
    // Read by the MCP middleware, which scopes the call to this project.
    #[allow(dead_code)]
    /// The project to create it in. Not needed for a subtask.
    pub project_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    /// no_priority, low, medium, high, or urgent.
    pub priority: Option<String>,
    /// "me", an actor ID, or an @handle.
    #[serde(alias = "assigneeActorId")]
    pub assignee: Option<String>,
    /// Make it a subtask of this task.
    #[serde(alias = "parentWorkItemId")]
    pub parent_id: Option<String>,
    /// A stable ID so retrying never creates it twice.
    pub client_request_id: Option<String>,
}

pub(super) async fn task_create(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskCreateArguments,
) -> WriteResult {
    let client_request_id = arguments
        .client_request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    if let Some(parent) = arguments.parent_id.as_deref() {
        return writes::create_subtask(
            state,
            principal,
            writes::CreateSubtaskArguments {
                parent_work_item_id: parent.to_owned(),
                title: arguments.title,
                description: arguments.description,
                client_request_id,
            },
        )
        .await;
    }
    // Tasks never go in the hidden Messages space, where nobody would see them.
    let in_messages_space =
        sqlx::query_scalar::<_, bool>("SELECT kind = 'direct' FROM projects WHERE id = $1")
            .bind(principal.project_id)
            .fetch_optional(&state.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(true);
    if !project_is_chosen(principal) || in_messages_space {
        const PROJECT_MESSAGE: &str = "Say which project with projectId (see projects_list).";
        return writes::WriteResult::InvalidRequest {
            field: "projectId".to_owned(),
            retry_category: PROJECT_MESSAGE.to_owned(),
            message: Some(PROJECT_MESSAGE.to_owned()),
        };
    }
    let assignee_actor_id =
        match assignee_for(state, principal, arguments.assignee.as_deref()).await {
            Ok(assignee) => assignee.flatten().map(|id| id.to_string()),
            Err(result) => return result,
        };
    writes::create_work_item(
        state,
        principal,
        writes::CreateWorkItemArguments {
            title: arguments.title,
            description: arguments.description,
            priority: arguments.priority,
            assignee_actor_id,
            client_request_id,
        },
    )
    .await
}

/// The assignee a task_create or task_update call named, in the project it
/// acts in: None when it named none, `Some(None)` for "none".
async fn assignee_for(
    state: &AppState,
    principal: &McpPrincipal,
    assignee: Option<&str>,
) -> Result<Option<Option<Uuid>>, WriteResult> {
    let Some(person) = assignee.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    let resolved = async {
        let mut connection = state.pool.acquire().await?;
        resolve_assignee(&mut connection, principal, principal.project_id, person).await
    }
    .await;
    match resolved {
        Ok(Ok(assignee)) => Ok(Some(assignee)),
        Ok(Err(problem)) => Err(assignee_error(person, problem)),
        Err(error) => {
            tracing::error!(
                category = error.log_category(),
                "MCP assignee lookup failed"
            );
            Err(WriteResult::TemporarilyUnavailable {
                retry_category: "retry_with_backoff".to_owned(),
            })
        }
    }
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskUpdateArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
    /// The version you last read (from task_open or task_list).
    pub expected_version: i64,
    pub title: Option<String>,
    pub description: Option<String>,
    /// no_priority, low, medium, high, or urgent.
    pub priority: Option<String>,
    /// "me", an actor ID, or an @handle.
    #[serde(alias = "assigneeActorId")]
    pub assignee: Option<String>,
    pub clear_assignee: Option<bool>,
    /// Move to this status, by name (such as "In Progress").
    pub status: Option<String>,
    /// A stable ID so retrying is safe.
    pub client_request_id: Option<String>,
}

pub(super) async fn task_update(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskUpdateArguments,
) -> WriteResult {
    let client_request_id = arguments
        .client_request_id
        .clone()
        .unwrap_or_else(|| Uuid::now_v7().to_string());
    let mut clear_assignee = arguments.clear_assignee;
    let assignee_actor_id =
        match assignee_for(state, principal, arguments.assignee.as_deref()).await {
            Ok(Some(Some(id))) => Some(id.to_string()),
            // "none" leaves it unassigned.
            Ok(Some(None)) => {
                clear_assignee = Some(true);
                None
            }
            Ok(None) => None,
            Err(result) => return result,
        };
    let arguments = TaskUpdateArguments {
        clear_assignee,
        ..arguments
    };
    let changes_fields = arguments.title.is_some()
        || arguments.description.is_some()
        || arguments.priority.is_some()
        || assignee_actor_id.is_some()
        || arguments.clear_assignee == Some(true);
    let Some(status) = arguments.status.clone() else {
        return writes::update_work_item(
            state,
            principal,
            writes::UpdateWorkItemArguments {
                work_item_id: arguments.task_id,
                expected_version: arguments.expected_version,
                title: arguments.title,
                description: arguments.description,
                priority: arguments.priority,
                assignee_actor_id,
                clear_assignee: arguments.clear_assignee,
                client_request_id,
            },
        )
        .await;
    };
    if !changes_fields {
        return writes::set_status(
            state,
            principal,
            writes::SetStatusArguments {
                work_item_id: arguments.task_id,
                expected_version: arguments.expected_version,
                status,
                client_request_id,
            },
        )
        .await;
    }
    writes::update_work_item_and_status(
        state,
        principal,
        writes::UpdateWorkItemArguments {
            work_item_id: arguments.task_id,
            expected_version: arguments.expected_version,
            title: arguments.title,
            description: arguments.description,
            priority: arguments.priority,
            assignee_actor_id,
            clear_assignee: arguments.clear_assignee,
            client_request_id,
        },
        status,
    )
    .await
}

// ---------------------------------------------------------------------------
// task_reorder

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskReorderArguments {
    /// Tasks in the order you want them, all in the same status column.
    pub task_ids: Vec<String>,
    /// Put them at the top (default) or bottom of their column.
    pub position: Option<String>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReorderValue {
    pub status: String,
    /// The whole column after the change, top first.
    pub column: Vec<String>,
}

pub(super) async fn task_reorder(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskReorderArguments,
) -> ToolResult<ReorderValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if arguments.task_ids.is_empty() || arguments.task_ids.len() > 100 {
        return ToolResult::invalid("taskIds", "Pass between 1 and 100 tasks.");
    }
    let at_top = match arguments.position.as_deref().unwrap_or("top") {
        "top" => true,
        "bottom" => false,
        _ => return ToolResult::invalid("position", "Use top or bottom."),
    };
    let result = async {
        let mut ordered = Vec::new();
        for identifier in &arguments.task_ids {
            match resolve_task(state, principal, identifier).await? {
                Some(found) if !ordered.contains(&found) => ordered.push(found),
                Some(_) => {
                    return Ok(ToolResult::invalid("taskIds", "Each task can appear once."));
                }
                None => {
                    return Ok(ToolResult::not_found(&format!("Task {identifier}")));
                }
            }
        }
        let project_id = ordered[0].1;
        if ordered.iter().any(|(_, project)| *project != project_id) {
            return Ok(ToolResult::invalid(
                "taskIds",
                "All the tasks must be in one project.",
            ));
        }
        if !can_write_in(state, principal, project_id, Action::WorkItemEdit).await? {
            return Ok(ToolResult::denied("You can't change tasks in that project."));
        }
        let ids = ordered.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        let statuses = sqlx::query_as::<_, (Uuid, String)>(
            r#"
            SELECT DISTINCT item.status_id, status.name
            FROM work_items item
            JOIN work_item_statuses status
              ON status.project_id = item.project_id AND status.id = item.status_id
            WHERE item.id = ANY($1) AND item.archived_at IS NULL
            "#,
        )
        .bind(&ids)
        .fetch_all(&mut *transaction)
        .await?;
        let [(status_id, status_name)] = statuses.as_slice() else {
            return Ok(ToolResult::invalid(
                "taskIds",
                "All the tasks must be open and in the same status column. Move them with task_update first.",
            ));
        };
        let column = sqlx::query_as::<_, (Uuid, i64, i64, String)>(
            r#"
            SELECT item.id, item.board_rank, item.version,
                   project.project_key || '-' || item.number::text
            FROM work_items item
            JOIN projects project ON project.id = item.project_id
            WHERE item.project_id = $1 AND item.status_id = $2 AND item.archived_at IS NULL
            ORDER BY item.board_rank, item.id
            FOR UPDATE OF item
            "#,
        )
        .bind(project_id)
        .bind(status_id)
        .fetch_all(&mut *transaction)
        .await?;
        let rest = column
            .iter()
            .filter(|row| !ids.contains(&row.0))
            .cloned()
            .collect::<Vec<_>>();
        let moved = ids
            .iter()
            .filter_map(|id| column.iter().find(|row| row.0 == *id).cloned())
            .collect::<Vec<_>>();
        let new_order = if at_top {
            moved.iter().chain(rest.iter()).cloned().collect::<Vec<_>>()
        } else {
            rest.iter().chain(moved.iter()).cloned().collect::<Vec<_>>()
        };
        let actor = command_actor(principal, project_id);
        for (index, (id, rank, version, _)) in new_order.iter().enumerate() {
            let new_rank = i64::try_from(index + 1)
                .ok()
                .and_then(|value| value.checked_mul(BOARD_RANK_STEP))
                .ok_or(AppError::Internal("task board rank"))?;
            if new_rank == *rank {
                continue;
            }
            let is_moved = ids.contains(id);
            sqlx::query(
                r#"
                UPDATE work_items
                SET board_rank = $2,
                    version = CASE WHEN $3 THEN version + 1 ELSE version END,
                    updated_at = CASE WHEN $3 THEN now() ELSE updated_at END
                WHERE id = $1
                "#,
            )
            .bind(id)
            .bind(new_rank)
            .bind(is_moved)
            .execute(&mut *transaction)
            .await?;
            if is_moved {
                insert_activity(
                    &mut transaction,
                    project_id,
                    *id,
                    principal.actor_id,
                    "work_item_updated",
                    "Task reordered on the board",
                    &serde_json::json!({ "position": index + 1 }),
                )
                .await?;
                enqueue_update(&mut transaction, project_id, *id, version + 1).await?;
                record_audit(
                    &mut transaction,
                    &actor,
                    "work_item.mcp_reordered",
                    *id,
                    serde_json::json!({ "position": index + 1 }),
                )
                .await?;
            }
        }
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(ReorderValue {
            status: status_name.clone(),
            column: new_order.into_iter().map(|row| row.3).collect(),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_reorder"))
}

/// The command actor for a write in `project_id` that takes no
/// clientRequestId; the audit row gets a fresh request ID.
pub(super) fn command_actor(principal: &McpPrincipal, project_id: Uuid) -> CommandActor {
    writes::mcp_actor(&scoped(principal, project_id), Uuid::now_v7().to_string())
}

// ---------------------------------------------------------------------------
// Claims

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskClaimArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
    /// What you're doing, shown to others.
    pub note: Option<String>,
    /// How long the claim lasts, in minutes (default 120, at most 1440).
    /// Claiming again renews it.
    pub minutes: Option<u16>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClaimValue {
    pub task_id: String,
    pub claimed_by: String,
    pub note: Option<String>,
    pub claimed_at: String,
    pub expires_at: String,
}

#[derive(Debug, FromRow)]
struct ClaimRow {
    work_item_id: Uuid,
    claimed_by: String,
    note: Option<String>,
    claimed_at: String,
    expires_at: String,
}

async fn current_claim(state: &AppState, task_id: Uuid) -> Result<Option<ClaimValue>, AppError> {
    Ok(sqlx::query_as::<_, ClaimRow>(
        r#"
        SELECT claim.work_item_id, COALESCE(member.display_name, 'Someone') AS claimed_by,
               claim.note, claim.claimed_at::text AS claimed_at,
               claim.expires_at::text AS expires_at
        FROM work_item_claims claim
        LEFT JOIN project_memberships member
          ON member.project_id = claim.project_id AND member.actor_id = claim.actor_id
        WHERE claim.work_item_id = $1 AND claim.expires_at > now()
        "#,
    )
    .bind(task_id)
    .fetch_optional(&state.pool)
    .await?
    .map(|row| ClaimValue {
        task_id: row.work_item_id.to_string(),
        claimed_by: row.claimed_by,
        note: row.note,
        claimed_at: row.claimed_at,
        expires_at: row.expires_at,
    }))
}

pub(super) async fn task_claim(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskClaimArguments,
) -> ToolResult<ClaimValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let minutes = arguments.minutes.map_or(DEFAULT_CLAIM_MINUTES, i32::from);
    if !(5..=1440).contains(&minutes) {
        return ToolResult::invalid("minutes", "Claim for 5 to 1440 minutes.");
    }
    let note = arguments
        .note
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| snippet(value, 500));
    let result = async {
        let Some((task_id, project_id)) =
            resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        if !can_write_in(state, principal, project_id, Action::WorkItemEdit).await? {
            return Ok(ToolResult::denied(
                "You can't work on tasks in that project.",
            ));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        // Claimers take turns on the task row. With a shared lock here, two
        // agents claiming at once deadlocked: the winner's activity write
        // waited on the loser's lock while the loser waited on the claim.
        let archived = sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NOT NULL FROM work_items WHERE id = $1 FOR NO KEY UPDATE",
        )
        .bind(task_id)
        .fetch_one(&mut *transaction)
        .await?;
        if archived {
            return Ok(ToolResult::conflict("That task is archived."));
        }
        // The second claimer runs after the first commits and finds a live
        // claim it doesn't own, so the update below changes nothing.
        let claimed = sqlx::query_as::<_, (Uuid, bool)>(
            r#"
            INSERT INTO work_item_claims (work_item_id, project_id, actor_id, note, expires_at)
            VALUES ($1, $2, $3, $4, now() + make_interval(mins => $5))
            ON CONFLICT (work_item_id) DO UPDATE
            SET actor_id = EXCLUDED.actor_id,
                note = COALESCE(EXCLUDED.note, work_item_claims.note),
                claimed_at = CASE WHEN work_item_claims.actor_id = EXCLUDED.actor_id
                                  THEN work_item_claims.claimed_at ELSE now() END,
                expires_at = EXCLUDED.expires_at
            WHERE work_item_claims.actor_id = EXCLUDED.actor_id
               OR work_item_claims.expires_at <= now()
            RETURNING actor_id, (xmax = 0) AS inserted
            "#,
        )
        .bind(task_id)
        .bind(project_id)
        .bind(principal.actor_id)
        .bind(&note)
        .bind(minutes)
        .fetch_optional(&mut *transaction)
        .await?;
        let Some((_, fresh)) = claimed else {
            transaction.rollback().await?;
            let holder = current_claim(state, task_id).await?;
            return Ok(ToolResult::conflict(match holder {
                Some(holder) => format!(
                    "{} is working on this until {}{}. Pick another task, or ask them.",
                    holder.claimed_by,
                    holder.expires_at,
                    holder
                        .note
                        .map(|note| format!(" ({note})"))
                        .unwrap_or_default()
                ),
                None => "Someone else just claimed this task. Try again.".to_owned(),
            }));
        };
        if fresh {
            insert_activity(
                &mut transaction,
                project_id,
                task_id,
                principal.actor_id,
                "work_item_updated",
                "Started working on this",
                &serde_json::json!({ "claim": true }),
            )
            .await?;
        }
        transaction.commit().await?;
        let claim = current_claim(state, task_id)
            .await?
            .ok_or(AppError::Internal("claim just written"))?;
        Ok::<_, AppError>(ToolResult::ok(claim))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_claim"))
}

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskReleaseArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct ReleaseValue {
    pub task_id: String,
    pub released: bool,
}

pub(super) async fn task_release(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskReleaseArguments,
) -> ToolResult<ReleaseValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let result = async {
        let Some((task_id, _)) = resolve_task(state, principal, &arguments.task_id).await? else {
            return Ok(ToolResult::not_found("That task"));
        };
        let released =
            sqlx::query("DELETE FROM work_item_claims WHERE work_item_id = $1 AND actor_id = $2")
                .bind(task_id)
                .bind(principal.actor_id)
                .execute(&state.pool)
                .await?
                .rows_affected()
                == 1;
        Ok::<_, AppError>(ToolResult::ok(ReleaseValue {
            task_id: task_id.to_string(),
            released,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_release"))
}

// ---------------------------------------------------------------------------
// task_link

#[derive(Clone, Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TaskLinkArguments {
    #[serde(rename = "taskId", alias = "workItemId")]
    pub task_id: String,
    /// A thread or conversation about this task.
    pub conversation_id: Option<String>,
    /// A message this task came from.
    pub message_id: Option<String>,
    pub page_id: Option<String>,
    pub decision_id: Option<String>,
    /// Remove the link instead.
    pub remove: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(super) struct LinkValue {
    pub task_id: String,
    pub kind: String,
    pub target_id: String,
    pub linked: bool,
    pub task_url: String,
}

pub(super) async fn task_link(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: TaskLinkArguments,
) -> ToolResult<LinkValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    let targets = [
        ("conversation", arguments.conversation_id.as_deref()),
        ("message", arguments.message_id.as_deref()),
        ("page", arguments.page_id.as_deref()),
        ("decision", arguments.decision_id.as_deref()),
    ]
    .into_iter()
    .filter_map(|(kind, value)| value.map(|value| (kind, value)))
    .collect::<Vec<_>>();
    let [(kind, target)] = targets.as_slice() else {
        return ToolResult::invalid(
            "conversationId",
            "Pass exactly one of conversationId, messageId, pageId, or decisionId.",
        );
    };
    let Some(target_id) = parse_id(target) else {
        return ToolResult::invalid(kind, "Pass an ID or link.");
    };
    let remove = arguments.remove.unwrap_or(false);
    let result = async {
        let Some((task_id, project_id)) = resolve_task(state, principal, &arguments.task_id).await?
        else {
            return Ok(ToolResult::not_found("That task"));
        };
        if !can_write_in(state, principal, project_id, Action::WorkItemEdit).await? {
            return Ok(ToolResult::denied("You can't change tasks in that project."));
        }
        // Every target must be in the task's own project, so a link can't show
        // another project's conversation, page, or decision to the people who
        // can read this task.
        let visible = match *kind {
            "conversation" => sqlx::query_scalar::<_, bool>(
                r#"SELECT EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND project_id = $3
                   AND conversation_visible_to(id, $2) AND actor_can_access_project($2, $4, $3))"#,
            ),
            "message" => sqlx::query_scalar::<_, bool>(
                r#"SELECT EXISTS (SELECT 1 FROM messages WHERE id = $1 AND project_id = $3
                   AND conversation_visible_to(conversation_id, $2) AND actor_can_access_project($2, $4, $3))"#,
            ),
            "page" => sqlx::query_scalar::<_, bool>(
                r#"SELECT EXISTS (SELECT 1 FROM project_pages WHERE id = $1 AND project_id = $3
                   AND archived_at IS NULL AND actor_can_access_project($2, $4, $3))"#,
            ),
            _ => sqlx::query_scalar::<_, bool>(
                r#"SELECT EXISTS (SELECT 1 FROM project_decisions WHERE id = $1 AND project_id = $3
                   AND (source_conversation_id IS NULL OR conversation_visible_to(source_conversation_id, $2))
                   AND actor_can_access_project($2, $4, $3))"#,
            ),
        }
        .bind(target_id)
        .bind(principal.actor_id)
        .bind(project_id)
        .bind(principal.external_agent_connection_id)
        .fetch_one(&state.pool)
        .await?;
        if !visible {
            return Ok(ToolResult::not_found(&format!("That {kind}")));
        }
        let mut transaction = state.pool.begin().await?;
        mark_via(&mut transaction, principal).await?;
        // Threads in the task's own project use the link the web app shows.
        let same_project_thread = *kind == "conversation"
            && sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS (SELECT 1 FROM conversations WHERE id = $1 AND project_id = $2 AND kind = 'thread')",
            )
            .bind(target_id)
            .bind(project_id)
            .fetch_one(&mut *transaction)
            .await?;
        if remove {
            if same_project_thread {
                sqlx::query(
                    "DELETE FROM work_item_conversation_links WHERE work_item_id = $1 AND conversation_id = $2",
                )
                .bind(task_id)
                .bind(target_id)
                .execute(&mut *transaction)
                .await?;
            }
            sqlx::query(
                "DELETE FROM work_item_links WHERE work_item_id = $1 AND target_kind = $2 AND target_id = $3",
            )
            .bind(task_id)
            .bind(kind)
            .bind(target_id)
            .execute(&mut *transaction)
            .await?;
        } else if same_project_thread {
            sqlx::query(
                r#"
                INSERT INTO work_item_conversation_links (project_id, work_item_id, conversation_id, created_by_actor_id)
                VALUES ($1, $2, $3, $4)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(project_id)
            .bind(task_id)
            .bind(target_id)
            .bind(principal.actor_id)
            .execute(&mut *transaction)
            .await?;
            insert_activity(
                &mut transaction,
                project_id,
                task_id,
                principal.actor_id,
                "thread_linked",
                "Thread linked",
                &serde_json::json!({ "conversationId": target_id }),
            )
            .await?;
        } else {
            sqlx::query(
                r#"
                INSERT INTO work_item_links (work_item_id, project_id, target_kind, target_id, created_by_actor_id)
                VALUES ($1, $2, $3, $4, $5)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(task_id)
            .bind(project_id)
            .bind(kind)
            .bind(target_id)
            .bind(principal.actor_id)
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(LinkValue {
            task_id: task_id.to_string(),
            kind: (*kind).to_owned(),
            target_id: target_id.to_string(),
            linked: !remove,
            task_url: public_url(state, &format!("/projects/{project_id}/work-items/{task_id}")),
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "task_link"))
}
