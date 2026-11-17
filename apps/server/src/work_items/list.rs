//! Task lists, group counts and board columns: the views engine
//! (`crate::views`) over the project's Tasks collection, with the list's
//! address read as a view (`collections::task_query::from_params`). A page
//! is found as task IDs in order, then the cards for just those IDs.
//!
//! Most filters take several values, comma separated: `statusId=a,b` lists
//! tasks in either status and `notStatusId=a,b` leaves both out. "none"
//! stands for no value where a field can be empty (`assigneeActorId=none,x`
//! is unassigned or assigned to x). Labels have three: `labelIds` (all of
//! them, as it always was), `anyLabelIds` and `notLabelIds`.
//!
//! `viewId` reads a saved view of the Tasks collection that the page can't
//! show as parameters (`task_query::to_params` has none for it), with any
//! parameters narrowing it.

use std::collections::{BTreeMap, HashMap};

use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::json;
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

use super::{
    AssigneeView, FIELD_VALUES_COLUMNS, WORK_ITEM_LABELS_COLUMN, WorkItemCapabilities,
    WorkItemLabelView, WorkItemStatusView, WorkItemTypeView, authorize_work_item, capabilities,
    project_membership, validate_priority,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    collections::{
        task_query::{self, TaskQuery},
        tasks,
    },
    error::{ApiPath, ApiQuery, AppError},
    fields::{FieldDefinition, FieldType},
    policy::Action,
    views::{
        definition::{self as views, SortRule, ViewConfig},
        filter::Filter,
        query::{self, GroupCount, GroupRef, Spec},
    },
    work_item_planning::OPEN_BLOCKER_COUNT,
};

const DEFAULT_LIMIT: i64 = 50;
/// The most values one filter names, such as 20 labels or 20 people.
pub(crate) const MAX_FILTER_VALUES: usize = 20;

/// A request the person can fix, with what to change.
fn invalid(field: &str, message: impl Into<String>) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(json!({ "error": {
            "code": "validation_failed",
            "message": message.into(),
            "details": { "field": field },
        } })),
    )
        .into_response()
}

/// A list request read: the view it asks for, as the compiler runs it.
struct Listing {
    connection: sqlx::Transaction<'static, sqlx::Postgres>,
    project_id: Uuid,
    capabilities: WorkItemCapabilities,
    /// The project's Tasks collection; None until its first task.
    collection_id: Option<Uuid>,
    definitions: Vec<FieldDefinition>,
    /// What the request asks for, a saved view's settings included: what
    /// "Save view" keeps.
    config: ViewConfig,
    query: TaskQuery,
    words: Vec<String>,
    spec: Spec,
    /// The parameters as asked, without the page's place.
    asked: BTreeMap<String, String>,
    cursor: Option<String>,
    limit: i64,
}

impl Listing {
    /// Every task in the project, whatever collection it was made in (a
    /// task-like collection's too); the Tasks collection's fields read them.
    fn source(&self) -> Option<tasks::TasksAcross> {
        self.collection_id.map(|_| tasks::TasksAcross {
            projects: vec![self.project_id],
            collection: None,
        })
    }

    /// The group as the task screens key it.
    fn shown(&self, group: GroupRef) -> GroupRef {
        match &self.spec.group {
            Some(field) => GroupRef {
                key: task_query::shown_key(field.id, &group.key),
                label: group.label,
            },
            None => group,
        }
    }
}

/// Reads a list request: whether the caller reads tasks here, then its
/// parameters as a view of the Tasks collection.
async fn listing(
    state: &AppState,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    mut params: BTreeMap<String, String>,
) -> Result<Result<Listing, Response>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)
        .await?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(principal, &membership, Action::WorkItemView, None)?;
    let capabilities = capabilities(&membership)?;
    let cursor = params.remove("cursor").filter(|cursor| !cursor.is_empty());
    let limit = match params.remove("limit") {
        None => DEFAULT_LIMIT,
        Some(value) => match value.trim().parse::<i64>() {
            Ok(limit) if (1..=query::MAX_LIMIT).contains(&limit) => limit,
            _ => {
                return Ok(Err(invalid(
                    "limit",
                    format!("Use a limit from 1 to {}.", query::MAX_LIMIT),
                )));
            }
        },
    };
    let asked = params.clone();
    let view_id = match params.remove("viewId") {
        None => None,
        Some(value) => match value.trim().parse::<Uuid>() {
            Ok(id) => Some(id),
            Err(_) => return Ok(Err(invalid("viewId", "Pass a view's ID."))),
        },
    };
    let in_group = params.remove("inGroup");
    if let Some((key, _)) = params
        .iter()
        .find(|(_, value)| value.split(',').count() > MAX_FILTER_VALUES)
    {
        return Ok(Err(invalid(
            key,
            format!("A filter names at most {MAX_FILTER_VALUES} values."),
        )));
    }
    let query = match task_query::from_params(&params) {
        Ok(query) => query,
        Err((field, message)) => return Ok(Err(invalid(field, message))),
    };
    let Some(words) = query::search_words(query.search.as_deref().unwrap_or("")) else {
        return Ok(Err(invalid(
            "q",
            format!(
                "Search for at most {} words in {} characters.",
                query::MAX_SEARCH_WORDS,
                query::MAX_SEARCH_CHARACTERS
            ),
        )));
    };
    // One snapshot for every query, so a page, its count and its cards agree.
    let mut connection = crate::db::scoped_snapshot(&state.pool).await?;
    let collection_id = task_query::tasks_collection(&mut connection, project_id).await?;
    let definitions = match collection_id {
        Some(id) => crate::collections::view_fields(&mut connection, project_id, id, false).await?,
        None => tasks::system_fields(&mut connection, project_id).await?,
    };
    let mut config = query.config.clone();
    if let Some(view_id) = view_id {
        let view =
            task_query::usable_view(&mut connection, project_id, view_id, principal.actor_id)
                .await?
                .ok_or(AppError::NotFound("view"))?;
        let saved = view.config.active(&definitions);
        config.filters = if config.filters.is_empty() {
            saved.filters
        } else if saved.filters.is_empty() {
            config.filters
        } else {
            Filter::And(vec![saved.filters, config.filters])
        };
        if config.sort.is_empty() {
            config.sort = saved.sort;
        }
        if config.group_by.is_none() {
            config.group_by = saved.group_by;
            config.group_bucket = saved.group_bucket;
        }
        config.search = saved.search;
    }
    if let Err(message) = views::check(&config, &definitions) {
        return Ok(Err(invalid("filters", message)));
    }
    if let Err(message) =
        views::check_references(&mut connection, project_id, &config, &definitions).await?
    {
        return Ok(Err(invalid("filters", message)));
    }
    let mut spec = Spec::from_config(&config, &definitions, principal.actor_id)
        .searching(words.clone(), &definitions);
    spec.archived = query.archived;
    if let Some(key) = in_group {
        let Some(field) = &spec.group else {
            return Ok(Err(invalid(
                "inGroup",
                "A group is read within a grouping.",
            )));
        };
        if key.is_empty() || key.len() > query::MAX_GROUP_KEY_LENGTH {
            return Ok(Err(invalid(
                "inGroup",
                "Name a group by a key the groups list gives.",
            )));
        }
        spec.in_group = Some(task_query::stored_key(field.id, &key));
    }
    Ok(Ok(Listing {
        connection,
        project_id,
        capabilities,
        collection_id,
        definitions,
        config,
        query,
        words,
        spec,
        asked,
        cursor,
        limit,
    }))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskListResponse {
    items: Vec<TaskCard>,
    next_cursor: Option<String>,
    total: i64,
    /// The Tasks collection, where views of tasks are saved; null until the
    /// project's first task.
    collection_id: Option<Uuid>,
    /// What the list shows as a view's settings: what saving it keeps.
    config: ViewConfig,
}

pub(super) async fn list_work_items(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, AppError> {
    let mut listing = match listing(&state, &principal, project_id, params).await? {
        Ok(listing) => listing,
        Err(refusal) => return Ok(refusal),
    };
    if listing.query.columns_by.is_some() {
        return Ok(invalid(
            "columnsBy",
            "Board columns are read from the board.",
        ));
    }
    let Some(source) = listing.source() else {
        return Ok(Json(TaskListResponse {
            items: Vec::new(),
            next_cursor: None,
            total: 0,
            collection_id: None,
            config: listing.config,
        })
        .into_response());
    };
    let Ok(page) = query::page(
        &mut listing.connection,
        &source,
        &listing.spec,
        listing.cursor.as_deref(),
        listing.limit,
    )
    .await?
    else {
        return Ok(invalid("cursor", query::CURSOR_MESSAGE));
    };
    let total = query::total(&mut listing.connection, &source, &listing.spec).await?;
    let rows = page
        .rows
        .into_iter()
        .map(|(id, group)| (id, group.map(|group| listing.shown(group))))
        .collect::<Vec<_>>();
    let items = cards(
        &mut *listing.connection,
        project_id,
        &rows,
        listing.capabilities,
    )
    .await?;
    listing.connection.commit().await?;
    Ok(Json(TaskListResponse {
        items,
        next_cursor: page.next_cursor,
        total,
        collection_id: listing.collection_id,
        config: listing.config,
    })
    .into_response())
}

#[derive(Debug, Serialize)]
pub(super) struct GroupsResponse {
    groups: Vec<GroupCount>,
}

pub(super) async fn list_work_item_groups(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, AppError> {
    let mut listing = match listing(&state, &principal, project_id, params).await? {
        Ok(listing) => listing,
        Err(refusal) => return Ok(refusal),
    };
    if listing.cursor.is_some() || listing.query.columns_by.is_some() {
        return Ok(invalid(
            "groupBy",
            "Group counts take the list's filters and a grouping, without a cursor or columns.",
        ));
    }
    let Some(field) = listing.spec.group.clone() else {
        return Ok(invalid("groupBy", "Name a grouping."));
    };
    let Some(source) = listing.source() else {
        return Ok(Json(GroupsResponse { groups: Vec::new() }).into_response());
    };
    let groups = query::group_counts(&mut listing.connection, &source, &listing.spec)
        .await?
        .into_iter()
        .map(|group| GroupCount {
            key: task_query::shown_key(field.id, &group.key),
            ..group
        })
        .collect();
    listing.connection.commit().await?;
    Ok(Json(GroupsResponse { groups }).into_response())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BoardResponse {
    columns_by: String,
    columns: Vec<BoardColumn>,
    collection_id: Option<Uuid>,
    /// The board as a view's settings: what saving it keeps.
    config: ViewConfig,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BoardColumn {
    key: String,
    label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    color: Option<String>,
    status: Option<WorkItemStatusView>,
    items: Vec<TaskCard>,
    has_more: bool,
    total: i64,
    next_cursor: Option<String>,
    /// The list's parameters for the rest of the column.
    filter: BTreeMap<String, String>,
    /// Whether a card can be moved here. Columns for an archived milestone
    /// or label, a disabled type or a removed member stay for the tasks that
    /// still carry them, but nothing new can be given that value.
    droppable: bool,
}

/// A value a column can stand for: its key, label, color, the status's
/// kind (for a status) and whether a card can still be given it.
struct ColumnValue {
    key: String,
    label: String,
    color: Option<String>,
    category: Option<String>,
    active: bool,
}

/// The values a board's columns can be: a choice field's options, or the
/// project's people for a person field.
async fn column_values(
    connection: &mut PgConnection,
    project_id: Uuid,
    field: &FieldDefinition,
) -> Result<Vec<ColumnValue>, AppError> {
    if field.field_type == FieldType::Person {
        let people = sqlx::query_as::<_, (Uuid, String)>(
            "SELECT membership.actor_id, membership.display_name \
             FROM project_memberships membership JOIN actors actor ON actor.id = membership.actor_id \
             WHERE membership.project_id = $1 AND membership.enabled AND actor.status = 'active' \
               AND actor.kind IN ('human', 'external_agent') \
             ORDER BY lower(membership.display_name), membership.actor_id::text",
        )
        .bind(project_id)
        .fetch_all(&mut *connection)
        .await?;
        return Ok(people
            .into_iter()
            .map(|(id, name)| ColumnValue {
                key: id.to_string(),
                label: name,
                color: None,
                category: None,
                active: true,
            })
            .collect());
    }
    Ok(field
        .config
        .options
        .iter()
        .map(|option| ColumnValue {
            key: option.id.to_string(),
            label: option.name.clone(),
            color: Some(option.color.clone()).filter(|color| !color.is_empty()),
            category: option.category.clone(),
            active: !option.archived,
        })
        .collect())
}

/// What a board's empty column is called.
fn none_label(field: Uuid) -> &'static str {
    match field {
        tasks::ASSIGNEE => "No assignee",
        tasks::LABELS => "No label",
        tasks::MILESTONE => "No milestone",
        tasks::PRIORITY => "No priority",
        _ => "No value",
    }
}

pub(super) async fn get_work_item_board(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(params): ApiQuery<BTreeMap<String, String>>,
) -> Result<Response, AppError> {
    let mut listing = match listing(&state, &principal, project_id, params).await? {
        Ok(listing) => listing,
        Err(refusal) => return Ok(refusal),
    };
    if listing.cursor.is_some() {
        return Ok(invalid(
            "cursor",
            "A board reads each column's first cards; read the rest through its filter.",
        ));
    }
    if listing.query.config.group_by.is_some() || !listing.query.config.sort.is_empty() {
        return Ok(invalid(
            "columnsBy",
            "A board's columns come from columnsBy, and its cards keep their board order.",
        ));
    }
    let field_id = listing.query.columns_by.unwrap_or(tasks::STATUS);
    let Some(field) = listing
        .definitions
        .iter()
        .find(|field| field.id == field_id && !field.archived && field.can_group())
        .filter(|field| field.field_type != FieldType::Date && field.id != tasks::PARENT)
        .cloned()
    else {
        return Ok(invalid(
            "columnsBy",
            "Board columns are by status, assignee, priority, label, milestone, type or a \
             choice or person field.",
        ));
    };
    let config = ViewConfig {
        group_by: Some(field.id),
        sort: vec![SortRule {
            field: tasks::RANK.to_owned(),
            direction: views::Direction::Asc,
        }],
        ..listing.config.clone()
    };
    let mut spec = Spec::from_config(&config, &listing.definitions, principal.actor_id)
        .searching(listing.words.clone(), &listing.definitions);
    spec.archived = listing.query.archived;
    let source = listing.source();
    let counts = match &source {
        Some(source) => query::group_counts(&mut listing.connection, source, &spec).await?,
        None => Vec::new(),
    };
    let values = column_values(&mut listing.connection, project_id, &field).await?;
    let mut count_of = counts
        .into_iter()
        .map(|group| (group.key.clone(), group))
        .collect::<HashMap<_, _>>();
    let category_of = values
        .iter()
        .filter_map(|value| Some((value.key.clone(), value.category.clone()?)))
        .collect::<HashMap<_, _>>();
    let droppable = values
        .iter()
        .filter(|value| value.active)
        .map(|value| value.key.clone())
        .collect::<std::collections::HashSet<_>>();
    // Every value a card can be given gets a column, used or not; values only
    // tasks still carry (an archived label, a removed member) follow them,
    // then "none".
    let mut columns = values
        .iter()
        .filter(|value| value.active)
        .map(|value| {
            count_of.remove(&value.key).unwrap_or(GroupCount {
                key: value.key.clone(),
                label: value.label.clone(),
                count: 0,
                color: value.color.clone(),
                value: None,
            })
        })
        .collect::<Vec<_>>();
    let mut rest = count_of.into_values().collect::<Vec<_>>();
    rest.sort_by(|a, b| {
        (a.key == "none", a.label.to_lowercase(), &a.key).cmp(&(
            b.key == "none",
            b.label.to_lowercase(),
            &b.key,
        ))
    });
    columns.extend(rest);
    let can_be_empty = field.id != tasks::STATUS && field.id != tasks::TYPE;
    if can_be_empty && !columns.iter().any(|group| group.key == "none") {
        columns.push(GroupCount {
            key: "none".into(),
            label: none_label(field.id).into(),
            count: 0,
            color: None,
            value: None,
        });
    }
    let many = matches!(
        field.field_type,
        FieldType::MultipleChoice | FieldType::People | FieldType::Labels
    );
    columns.retain(|group| {
        task_query::allows_group(
            &spec.filter,
            field.id,
            &group.key,
            category_of.get(&group.key).map(String::as_str),
            many,
        )
    });

    let mut pages = Vec::with_capacity(columns.len());
    let mut all_rows = Vec::new();
    for group in &columns {
        let page = match &source {
            Some(source) => {
                let mut column = spec.clone();
                column.in_group = Some(group.key.clone());
                query::page(
                    &mut listing.connection,
                    source,
                    &column,
                    None,
                    listing.limit,
                )
                .await?
                .map_err(|()| AppError::Internal("board column cursor"))?
            }
            None => query::Page {
                rows: Vec::new(),
                next_cursor: None,
            },
        };
        all_rows.extend(page.rows.iter().map(|(id, _)| (*id, None)));
        pages.push(page);
    }
    let cards = cards(
        &mut *listing.connection,
        project_id,
        &all_rows,
        listing.capabilities,
    )
    .await?
    .into_iter()
    .map(|card| (card.id, card))
    .collect::<HashMap<_, _>>();
    listing.connection.commit().await?;
    let columns_by = task_query::grouping_name(field.id);
    let mut base = listing.asked.clone();
    base.remove("columnsBy");
    base.insert("groupBy".into(), columns_by.clone());
    base.insert("sort".into(), "manual".into());
    let columns = columns
        .into_iter()
        .zip(pages)
        .map(|(group, page)| {
            let key = task_query::shown_key(field.id, &group.key);
            let mut filter = base.clone();
            filter.insert("inGroup".into(), key.clone());
            let status = (field.id == tasks::STATUS)
                .then(|| {
                    Some(WorkItemStatusView {
                        id: group.key.parse().ok()?,
                        name: group.label.clone(),
                        semantic_category: category_of.get(&group.key)?.clone(),
                    })
                })
                .flatten();
            BoardColumn {
                items: page
                    .rows
                    .iter()
                    .filter_map(|(id, _)| cards.get(id).cloned())
                    .collect(),
                has_more: page.next_cursor.is_some(),
                next_cursor: page.next_cursor,
                total: group.count,
                droppable: group.key == "none" || droppable.contains(&group.key),
                label: group.label,
                color: group.color,
                status,
                filter,
                key,
            }
        })
        .collect();
    Ok(Json(BoardResponse {
        columns_by,
        columns,
        collection_id: listing.collection_id,
        config: ViewConfig {
            group_by: Some(field.id),
            ..listing.config
        },
    })
    .into_response())
}

const BASE_FROM: &str = r#"
    FROM task_rows item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_types item_type
      ON item_type.project_id = item.project_id AND item_type.id = item.type_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
"#;

/// A task as list rows and board cards show it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TaskCard {
    id: Uuid,
    project_id: Uuid,
    key: String,
    number: i64,
    title: String,
    item_type: WorkItemTypeView,
    status: WorkItemStatusView,
    priority: String,
    assignee: Option<AssigneeView>,
    version: i64,
    archived_at: Option<String>,
    created_at: String,
    updated_at: String,
    labels: Vec<WorkItemLabelView>,
    capabilities: WorkItemCapabilities,
    parent_id: Option<Uuid>,
    milestone: Option<serde_json::Value>,
    start_date: Option<String>,
    due_date: Option<String>,
    estimate: Option<f64>,
    blocked_reason: Option<String>,
    open_blocker_count: i64,
    ready: bool,
    subtask_count: i64,
    subtasks_done: i64,
    claimed_by: Option<serde_json::Value>,
    /// Where the card sits in manual order (status, then rank), as a string
    /// that sorts the same way. Boards grouped by anything but status keep
    /// this order, so a moved card can be shown where it will land.
    board_order: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<GroupRef>,
    /// Custom field values by field ID, and the people, tasks and pages
    /// they name (AWD-89).
    field_values: serde_json::Value,
    field_references: serde_json::Value,
}

#[derive(Debug, FromRow)]
struct CardRow {
    id: Uuid,
    project_id: Uuid,
    project_key: String,
    number: i64,
    title: String,
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
    parent_work_item_id: Option<Uuid>,
    milestone: Option<serde_json::Value>,
    start_date: Option<String>,
    due_date: Option<String>,
    estimate: Option<f64>,
    blocked_reason: Option<String>,
    open_blocker_count: i64,
    subtask_count: i64,
    subtasks_done: i64,
    claimed_by: Option<serde_json::Value>,
    status_position: i32,
    board_rank: i64,
    field_values: serde_json::Value,
    field_references: serde_json::Value,
}

/// The cards for `ids`, in that order (an ID may repeat when grouped by
/// label). IDs not in the project are skipped.
pub(super) async fn cards(
    executor: impl sqlx::PgExecutor<'_>,
    project_id: Uuid,
    rows: &[(Uuid, Option<GroupRef>)],
    capabilities: WorkItemCapabilities,
) -> Result<Vec<TaskCard>, AppError> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    let ids = rows.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let found = sqlx::query_as::<_, CardRow>(&format!(
        r#"
        SELECT
            item.id, item.project_id, project.project_key, item.number, item.title,
            item.type_id, item_type.name AS type_name,
            item.status_id, status.name AS status_name, status.semantic_category,
            status.position AS status_position, item.board_rank,
            item.priority, item.assignee_actor_id,
            assignee.display_name AS assignee_display_name,
            assignee.handle AS assignee_handle,
            assignee_actor.kind AS assignee_kind,
            CASE WHEN assignee.actor_id IS NULL THEN NULL
                 ELSE assignee.enabled AND assignee_actor.status = 'active'
            END AS assignee_active,
            item.version, item.archived_at::text AS archived_at,
            item.created_at::text AS created_at, item.updated_at::text AS updated_at,
            {WORK_ITEM_LABELS_COLUMN},
            item.parent_work_item_id,
            (SELECT jsonb_build_object('id', milestone.id, 'name', milestone.name,
                                       'targetDate', milestone.target_date::text)
               FROM project_milestones milestone
              WHERE milestone.project_id = item.project_id AND milestone.id = item.milestone_id
            ) AS milestone,
            item.start_date::text AS start_date, item.due_date::text AS due_date,
            item.estimate, item.blocked_reason,
            {FIELD_VALUES_COLUMNS},
            {OPEN_BLOCKER_COUNT} AS open_blocker_count,
            (SELECT count(*) FROM task_rows child
              WHERE child.project_id = item.project_id
                AND child.parent_work_item_id = item.id AND child.archived_at IS NULL
            ) AS subtask_count,
            (SELECT count(*) FROM task_rows child
               JOIN work_item_statuses child_status
                 ON child_status.project_id = child.project_id AND child_status.id = child.status_id
              WHERE child.project_id = item.project_id
                AND child.parent_work_item_id = item.id AND child.archived_at IS NULL
                AND child_status.semantic_category IN ('completed', 'canceled')
            ) AS subtasks_done,
            (SELECT jsonb_build_object('id', claimer.id, 'kind', claimer.kind,
                                       'displayName', COALESCE(claimer_membership.display_name, 'Unknown'),
                                       'handle', COALESCE(claimer_membership.handle, ''),
                                       'avatarUrl', NULL)
               FROM work_item_claims claim
               JOIN actors claimer ON claimer.id = claim.actor_id
               LEFT JOIN project_memberships claimer_membership
                 ON claimer_membership.project_id = item.project_id
                AND claimer_membership.actor_id = claim.actor_id
              WHERE claim.work_item_id = item.id AND claim.expires_at > now()
            ) AS claimed_by
        {BASE_FROM}
        LEFT JOIN actors assignee_actor ON assignee_actor.id = assignee.actor_id
        WHERE item.project_id = $1 AND item.id = ANY($2)
        "#
    ))
    .bind(project_id)
    .bind(&ids)
    .fetch_all(executor)
    .await?;
    let mut by_id = found
        .into_iter()
        .map(|row| (row.id, row))
        .collect::<HashMap<_, _>>();
    let mut cards = Vec::with_capacity(rows.len());
    for (index, (id, group)) in rows.iter().enumerate() {
        // The last time an ID appears it can be moved out of the map.
        let last = rows[index + 1..].iter().all(|(other, _)| other != id);
        let row = if last {
            by_id.remove(id)
        } else {
            by_id.get(id).map(clone_row)
        };
        if let Some(row) = row {
            cards.push(card(row, group.clone(), capabilities)?);
        }
    }
    Ok(cards)
}

fn clone_row(row: &CardRow) -> CardRow {
    CardRow {
        id: row.id,
        project_id: row.project_id,
        project_key: row.project_key.clone(),
        number: row.number,
        title: row.title.clone(),
        type_id: row.type_id,
        type_name: row.type_name.clone(),
        status_id: row.status_id,
        status_name: row.status_name.clone(),
        semantic_category: row.semantic_category.clone(),
        priority: row.priority.clone(),
        assignee_actor_id: row.assignee_actor_id,
        assignee_display_name: row.assignee_display_name.clone(),
        assignee_handle: row.assignee_handle.clone(),
        assignee_kind: row.assignee_kind.clone(),
        assignee_active: row.assignee_active,
        version: row.version,
        archived_at: row.archived_at.clone(),
        created_at: row.created_at.clone(),
        updated_at: row.updated_at.clone(),
        labels: row.labels.clone(),
        parent_work_item_id: row.parent_work_item_id,
        milestone: row.milestone.clone(),
        start_date: row.start_date.clone(),
        due_date: row.due_date.clone(),
        estimate: row.estimate,
        blocked_reason: row.blocked_reason.clone(),
        open_blocker_count: row.open_blocker_count,
        subtask_count: row.subtask_count,
        subtasks_done: row.subtasks_done,
        claimed_by: row.claimed_by.clone(),
        status_position: row.status_position,
        board_rank: row.board_rank,
        field_values: row.field_values.clone(),
        field_references: row.field_references.clone(),
    }
}

fn card(
    row: CardRow,
    group: Option<GroupRef>,
    capabilities: WorkItemCapabilities,
) -> Result<TaskCard, AppError> {
    validate_priority(&row.priority)?;
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
        _ => return Err(AppError::Internal("invalid task card assignee projection")),
    };
    let labels = serde_json::from_value(row.labels)
        .map_err(|_| AppError::Internal("invalid task card label projection"))?;
    let finished = matches!(row.semantic_category.as_str(), "completed" | "canceled");
    let ready = !finished
        && row.archived_at.is_none()
        && row.blocked_reason.is_none()
        && row.open_blocker_count == 0;
    let card = TaskCard {
        id: row.id,
        project_id: row.project_id,
        key: format!("{}-{}", row.project_key, row.number),
        number: row.number,
        title: row.title,
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
        capabilities,
        parent_id: row.parent_work_item_id,
        milestone: row.milestone,
        start_date: row.start_date,
        due_date: row.due_date,
        estimate: row.estimate,
        blocked_reason: row.blocked_reason,
        open_blocker_count: row.open_blocker_count,
        ready,
        subtask_count: row.subtask_count,
        subtasks_done: row.subtasks_done,
        claimed_by: row.claimed_by,
        board_order: board_order(row.status_position, row.board_rank),
        group,
        field_values: row.field_values,
        field_references: row.field_references,
    };
    Ok(if capabilities.tasks {
        card
    } else {
        without_contents(card)
    })
}

/// For someone who may change tasks here but not see them (a bulk change's
/// response, say): the card's id, key, version, type and status, not its
/// contents, as a single task's view does.
fn without_contents(card: TaskCard) -> TaskCard {
    TaskCard {
        title: String::new(),
        priority: "no_priority".to_owned(),
        assignee: None,
        labels: Vec::new(),
        parent_id: None,
        milestone: None,
        start_date: None,
        due_date: None,
        estimate: None,
        blocked_reason: None,
        open_blocker_count: 0,
        ready: false,
        subtask_count: 0,
        subtasks_done: 0,
        claimed_by: None,
        field_values: serde_json::json!({}),
        field_references: serde_json::json!([]),
        ..card
    }
}

/// Status position, then rank, as fixed-width digits so that comparing the
/// strings compares the pair. The rank is shifted to be non-negative.
fn board_order(status_position: i32, board_rank: i64) -> String {
    let position = i64::from(status_position) - i64::from(i32::MIN);
    let rank = i128::from(board_rank) - i128::from(i64::MIN);
    format!("{position:010}:{rank:020}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_order_sorts_like_status_then_rank() {
        let mut pairs = [(2, -5), (1, i64::MAX), (1, i64::MIN), (2, 7), (1, 0)];
        let mut keys = pairs.map(|(position, rank)| board_order(position, rank));
        pairs.sort_unstable();
        keys.sort();
        assert_eq!(
            keys,
            pairs.map(|(position, rank)| board_order(position, rank))
        );
    }
}
