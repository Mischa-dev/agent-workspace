//! Ways to look at tasks beyond one list (docs/plans/roadmap.md, W11):
//! saved views (V5), My work across every project (V6) and a project's
//! timeline (V8).
//!
//! A saved view keeps the list page's own query (filters, sort, grouping,
//! board columns and lanes) with its layout and visible fields, so opening
//! one is just opening the list with that query. "me" in a view means
//! whoever opens it, which is how one shared "My tasks" works for everyone.

use std::collections::BTreeMap;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use chrono::{Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{FromRow, PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

use super::{
    authorize_work_item, capabilities, list,
    planning::{Started, Write, begin, invalid, refusal, start_write},
    project_membership,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    idempotency::resource_bound_scope,
    policy::Action,
    work_item_commands::record_audit_for,
    work_item_planning::{self as planning, HAS_OPEN_BLOCKER, MilestoneView},
};

/// Most views one person keeps in a project, and most shared views a
/// project keeps, so the sidebar and the view menu stay usable.
const MAX_VIEWS: i64 = 100;
const MAX_NAME_CHARACTERS: usize = 80;
/// List columns a view can show or hide (select and title always show).
pub(crate) const VIEW_FIELDS: [&str; 6] =
    ["priority", "key", "status", "details", "due", "assignee"];

/// A view every project has.
pub(crate) struct BuiltinView {
    pub key: &'static str,
    pub name: &'static str,
    pub query: &'static [(&'static str, &'static str)],
}

pub(crate) const BUILTIN_VIEWS: [BuiltinView; 4] = [
    BuiltinView {
        key: "my_tasks",
        name: "My tasks",
        query: &[
            ("assigneeActorId", "me"),
            ("includeDone", "false"),
            ("sort", "due"),
        ],
    },
    BuiltinView {
        key: "ready_for_agents",
        name: "Ready for agents",
        query: &[
            ("ready", "true"),
            ("claimed", "false"),
            ("sort", "priority"),
        ],
    },
    BuiltinView {
        key: "blocked",
        name: "Blocked",
        query: &[
            ("blocked", "true"),
            ("includeDone", "false"),
            ("sort", "priority"),
        ],
    },
    BuiltinView {
        key: "recently_changed",
        name: "Recently changed",
        query: &[("sort", "updated")],
    },
];

// ---------------------------------------------------------------------------
// Saved views: reading

#[derive(Clone, Debug, FromRow)]
pub(crate) struct ViewRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub owner_actor_id: Option<Uuid>,
    pub builtin: Option<String>,
    pub name: String,
    pub query: Value,
    pub layout: String,
    pub fields: Option<Vec<String>>,
    pub position: i32,
    pub version: i64,
    pub pinned: bool,
    pub pin_position: Option<i32>,
}

/// Makes the four built-in views of a project if it doesn't have them yet.
pub(crate) async fn ensure_builtin_views(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
) -> Result<(), AppError> {
    let mut ids = Vec::new();
    let mut keys = Vec::new();
    let mut names = Vec::new();
    let mut queries = Vec::new();
    let mut positions = Vec::new();
    for (index, builtin) in BUILTIN_VIEWS.iter().enumerate() {
        ids.push(Uuid::now_v7());
        keys.push(builtin.key);
        names.push(builtin.name);
        queries.push(Value::Object(
            builtin
                .query
                .iter()
                .map(|(key, value)| ((*key).to_owned(), Value::String((*value).to_owned())))
                .collect(),
        ));
        positions.push(index as i32);
    }
    sqlx::query(
        r#"
        INSERT INTO saved_views (id, project_id, builtin, name, query, position)
        SELECT id, $1, builtin, name, query, position
        FROM unnest($2::uuid[], $3::text[], $4::text[], $5::jsonb[], $6::integer[])
             AS builtin(id, builtin, name, query, position)
        WHERE EXISTS (SELECT 1 FROM projects WHERE id = $1)
        ON CONFLICT (project_id, builtin) WHERE builtin IS NOT NULL DO NOTHING
        "#,
    )
    .bind(project_id)
    .bind(&ids)
    .bind(&keys)
    .bind(&names)
    .bind(&queries)
    .bind(&positions)
    .execute(executor)
    .await?;
    Ok(())
}

/// Views and whether the reader (`$2`) pinned them. Their role's defaults
/// say which start pinned (AWD-74); without any, My tasks and Ready for
/// agents do, until they unpin them.
const VIEW_SELECT: &str = r#"
    SELECT saved.id, saved.project_id, saved.owner_actor_id, saved.builtin, saved.name,
           saved.query, saved.layout, saved.fields, saved.position, saved.version,
           COALESCE(
               pin.pinned,
               saved.builtin = ANY(COALESCE(
                   role_default_pinned_views(saved.project_id, $2),
                   ARRAY['my_tasks', 'ready_for_agents']
               )),
               false
           ) AS pinned,
           pin.position AS pin_position
    FROM saved_views saved
    LEFT JOIN saved_view_pins pin ON pin.view_id = saved.id AND pin.actor_id = $2
"#;

/// The views `reader` can see in a project: the project's shared ones and
/// their own, built-ins first, then in the order they were made.
pub(crate) async fn visible_views(
    executor: impl PgExecutor<'_>,
    project_id: Uuid,
    reader: Uuid,
) -> Result<Vec<ViewRow>, AppError> {
    Ok(sqlx::query_as::<_, ViewRow>(&format!(
        r#"{VIEW_SELECT}
        WHERE saved.project_id = $1
          AND (saved.owner_actor_id IS NULL OR saved.owner_actor_id = $2)
        ORDER BY saved.builtin IS NULL, saved.position, lower(saved.name), saved.id
        LIMIT 300"#
    ))
    .bind(project_id)
    .bind(reader)
    .fetch_all(executor)
    .await?)
}

async fn view_for_update(
    connection: &mut PgConnection,
    project_id: Uuid,
    view_id: Uuid,
    reader: Uuid,
) -> Result<Option<ViewRow>, AppError> {
    Ok(sqlx::query_as::<_, ViewRow>(&format!(
        r#"{VIEW_SELECT}
        WHERE saved.project_id = $1 AND saved.id = $3
          AND (saved.owner_actor_id IS NULL OR saved.owner_actor_id = $2)
        FOR UPDATE OF saved"#
    ))
    .bind(project_id)
    .bind(reader)
    .bind(view_id)
    .fetch_optional(&mut *connection)
    .await?)
}

/// A view's stored query as parameters. Anything that isn't a string is
/// dropped; saving only ever stores strings.
pub(crate) fn query_params(query: &Value) -> BTreeMap<String, String> {
    query
        .as_object()
        .map(|object| {
            object
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_owned()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The query with "me" replaced by the reader.
pub(crate) fn resolved_params(
    params: &BTreeMap<String, String>,
    reader: Uuid,
) -> BTreeMap<String, String> {
    params
        .iter()
        .map(|(key, value)| (key.clone(), list::resolve_me(key, value, reader)))
        .collect()
}

/// Where a view opens: the task list with its query, layout and fields, and
/// `savedView` so the page shows which view it is.
pub(crate) fn view_href(row: &ViewRow, reader: Uuid) -> String {
    let mut params = resolved_params(&query_params(&row.query), reader);
    if row.layout == "board" {
        params.insert("view".to_owned(), "board".to_owned());
    }
    if let Some(fields) = &row.fields {
        params.insert("fields".to_owned(), fields.join(","));
    }
    params.insert("savedView".to_owned(), row.id.to_string());
    format!(
        "/projects/{}/work-items?{}",
        row.project_id,
        serde_urlencoded::to_string(&params).unwrap_or_default()
    )
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SavedViewView {
    pub(crate) id: Uuid,
    pub(crate) name: String,
    /// my_tasks, ready_for_agents, blocked or recently_changed.
    pub(crate) builtin: Option<String>,
    /// Shared with everyone in the project, rather than yours alone.
    pub(crate) shared: bool,
    /// As saved; "me" stands for whoever opens it.
    pub(crate) query: BTreeMap<String, String>,
    /// With "me" replaced by you.
    pub(crate) resolved_query: BTreeMap<String, String>,
    pub(crate) layout: String,
    pub(crate) fields: Option<Vec<String>>,
    pub(crate) pinned: bool,
    pub(crate) position: i32,
    pub(crate) can_edit: bool,
    pub(crate) can_delete: bool,
    pub(crate) version: i64,
    pub(crate) href: String,
}

pub(crate) fn saved_view(row: ViewRow, reader: Uuid, can_share: bool) -> SavedViewView {
    let params = query_params(&row.query);
    let shared = row.owner_actor_id.is_none();
    let can_edit = if shared { can_share } else { true };
    SavedViewView {
        href: view_href(&row, reader),
        resolved_query: resolved_params(&params, reader),
        query: params,
        id: row.id,
        name: row.name,
        can_delete: can_edit && row.builtin.is_none(),
        builtin: row.builtin,
        shared,
        layout: row.layout,
        fields: row.fields,
        pinned: row.pinned,
        position: row.pin_position.unwrap_or(row.position),
        can_edit,
        version: row.version,
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ViewsResponse {
    views: Vec<SavedViewView>,
    /// Whether you can save views for everyone in the project.
    can_share: bool,
}

pub(super) async fn list_views(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<ViewsResponse>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    if membership.project_archived_at.is_none() {
        ensure_builtin_views(&state.pool, project_id).await?;
    }
    let can_share = capabilities(&membership)?.edit && membership.project_archived_at.is_none();
    let rows = visible_views(&state.pool, project_id, principal.actor_id).await?;
    Ok(Json(ViewsResponse {
        views: rows
            .into_iter()
            .map(|row| saved_view(row, principal.actor_id, can_share))
            .collect(),
        can_share,
    }))
}

// ---------------------------------------------------------------------------
// Saved views: writing

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateViewRequest {
    name: String,
    #[serde(default)]
    shared: bool,
    #[serde(default)]
    query: BTreeMap<String, String>,
    #[serde(default = "default_layout")]
    layout: String,
    fields: Option<Vec<String>>,
}

fn default_layout() -> String {
    "list".to_owned()
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateViewRequest {
    expected_version: i64,
    name: Option<String>,
    shared: Option<bool>,
    query: Option<BTreeMap<String, String>>,
    layout: Option<String>,
    #[serde(default, deserialize_with = "super::deserialize_optional_field")]
    fields: Option<Option<Vec<String>>>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DeleteViewRequest {
    expected_version: Option<i64>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PinViewRequest {
    pinned: bool,
    position: Option<i32>,
}

pub(crate) fn normalize_view_name(value: &str) -> Option<String> {
    let name = value.split_whitespace().collect::<Vec<_>>().join(" ");
    (!name.is_empty() && name.chars().count() <= MAX_NAME_CHARACTERS).then_some(name)
}

pub(crate) fn normalize_fields(
    fields: Option<&[String]>,
) -> Result<Option<Vec<String>>, &'static str> {
    let Some(fields) = fields else {
        return Ok(None);
    };
    let mut kept = Vec::new();
    for field in fields {
        // A custom field's column is `field:<id>` (AWD-89).
        let custom = field
            .strip_prefix("field:")
            .is_some_and(|id| id.parse::<Uuid>().is_ok());
        if !custom && !VIEW_FIELDS.contains(&field.as_str()) {
            return Err("fields");
        }
        if !kept.contains(field) {
            kept.push(field.clone());
        }
    }
    Ok(Some(kept))
}

/// The tasks a view's query matches, for automation conditions (X5).
pub(crate) use super::list::matching_tasks;

/// Checks a query the way the list will read it, and that every ID in it
/// belongs to the project.
pub(crate) async fn check_view_query(
    pool: &PgPool,
    project_id: Uuid,
    reader: Uuid,
    query: &BTreeMap<String, String>,
) -> Result<(), AppError> {
    if query.len() > 30
        || query
            .iter()
            .any(|(key, value)| key.len() > 40 || value.len() > 1_000)
    {
        return Err(AppError::Validation("query"));
    }
    let mut spec = list::saved_query_spec(query, reader)?;
    spec.filters
        .resolve_field_me(pool, project_id, reader)
        .await?;
    list::validate_filter_ids(pool, project_id, &spec.filters).await?;
    spec.resolve_fields(pool, project_id).await
}

fn check_layout(layout: &str) -> Result<(), AppError> {
    if matches!(layout, "list" | "board") {
        Ok(())
    } else {
        Err(AppError::Validation("layout"))
    }
}

async fn count_views(
    write: &mut Write,
    project_id: Uuid,
    owner: Option<Uuid>,
) -> Result<i64, AppError> {
    Ok(sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM saved_views WHERE project_id = $1 AND owner_actor_id IS NOT DISTINCT FROM $2",
    )
    .bind(project_id)
    .bind(owner)
    .fetch_one(&mut *write.transaction)
    .await?)
}

fn can_share(write: &Write) -> Result<bool, AppError> {
    Ok(capabilities(&write.membership)?.edit)
}

pub(super) async fn create_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateViewRequest>,
) -> Result<Response, AppError> {
    let Some(name) = normalize_view_name(&request.name) else {
        return Ok(invalid(
            "name",
            "Give the view a name of 1 to 80 characters.",
        ));
    };
    check_layout(&request.layout)?;
    let fields = normalize_fields(request.fields.as_deref()).map_err(AppError::Validation)?;
    check_view_query(&state.pool, project_id, principal.actor_id, &request.query).await?;
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            if request.shared {
                Action::WorkItemEdit
            } else {
                Action::WorkItemView
            },
            None,
            resource_bound_scope("projects", project_id, "saved_views_create")?,
            &request,
        )
        .await
    );
    let owner = (!request.shared).then_some(principal.actor_id);
    if count_views(&mut write, project_id, owner).await? >= MAX_VIEWS {
        return Ok(refusal(
            StatusCode::CONFLICT,
            "too_many_views",
            "This project has 100 views already. Delete one you no longer use first.".to_owned(),
            json!({}),
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO saved_views (
            id, project_id, owner_actor_id, created_by_actor_id, name, query, layout, fields, position
        )
        SELECT $1, $2, $3, $4, $5, $6, $7, $8,
               COALESCE((SELECT max(position) + 1 FROM saved_views WHERE project_id = $2), 0)
        "#,
    )
    .bind(id)
    .bind(project_id)
    .bind(owner)
    .bind(principal.actor_id)
    .bind(&name)
    .bind(json!(request.query))
    .bind(&request.layout)
    .bind(&fields)
    .execute(&mut *write.transaction)
    .await?;
    let actor = write.actor.clone();
    record_audit_for(
        &mut write.transaction,
        &actor,
        "saved_view",
        "saved_view.created",
        id,
        json!({"shared": request.shared}),
    )
    .await?;
    let row = view_for_update(&mut write.transaction, project_id, id, principal.actor_id)
        .await?
        .ok_or(AppError::Internal("saved view just made is missing"))?;
    let view = saved_view(row, principal.actor_id, can_share(&write)?);
    write.finish(StatusCode::CREATED, &view).await
}

/// Locks a view the caller may change: their own, or a shared one when
/// they can edit the project's tasks.
async fn editable_view(
    write: &mut Write,
    project_id: Uuid,
    view_id: Uuid,
    reader: Uuid,
) -> Result<ViewRow, AppError> {
    let row = view_for_update(&mut write.transaction, project_id, view_id, reader)
        .await?
        .ok_or(AppError::NotFound("saved_view"))?;
    if row.owner_actor_id.is_none() && !can_share(write)? {
        return Err(AppError::Forbidden);
    }
    Ok(row)
}

fn view_version_conflict(current: SavedViewView) -> Response {
    refusal(
        StatusCode::CONFLICT,
        "version_conflict",
        "Someone changed this view first. You’re now looking at the latest version.".to_owned(),
        json!({ "resource": "saved_view", "currentVersion": current.version, "current": current }),
    )
}

pub(super) async fn update_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, view_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateViewRequest>,
) -> Result<Response, AppError> {
    let name = match request.name.as_deref() {
        Some(value) => match normalize_view_name(value) {
            Some(name) => Some(name),
            None => {
                return Ok(invalid(
                    "name",
                    "Give the view a name of 1 to 80 characters.",
                ));
            }
        },
        None => None,
    };
    if let Some(layout) = &request.layout {
        check_layout(layout)?;
    }
    let fields = match &request.fields {
        Some(fields) => Some(normalize_fields(fields.as_deref()).map_err(AppError::Validation)?),
        None => None,
    };
    if let Some(query) = &request.query {
        check_view_query(&state.pool, project_id, principal.actor_id, query).await?;
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemView,
            None,
            resource_bound_scope("saved_views", view_id, "update")?,
            &request,
        )
        .await
    );
    let row = editable_view(&mut write, project_id, view_id, principal.actor_id).await?;
    let sharing = can_share(&write)?;
    if row.version != request.expected_version {
        return Ok(view_version_conflict(saved_view(
            row,
            principal.actor_id,
            sharing,
        )));
    }
    // Sharing a personal view or taking a shared one back needs the right
    // to change what the whole project sees; built-ins stay shared.
    let owner = match request.shared {
        Some(true) if !sharing => return Err(AppError::Forbidden),
        Some(false) if row.builtin.is_some() => {
            return Ok(invalid(
                "shared",
                "Built-in views stay shared with the project.",
            ));
        }
        Some(false) if row.owner_actor_id.is_none() && !sharing => {
            return Err(AppError::Forbidden);
        }
        Some(true) => None,
        Some(false) => Some(principal.actor_id),
        None => row.owner_actor_id,
    };
    if owner != row.owner_actor_id && count_views(&mut write, project_id, owner).await? >= MAX_VIEWS
    {
        return Ok(refusal(
            StatusCode::CONFLICT,
            "too_many_views",
            "There are 100 views there already. Delete one you no longer use first.".to_owned(),
            json!({}),
        ));
    }
    sqlx::query(
        r#"
        UPDATE saved_views
        SET name = COALESCE($3, name),
            owner_actor_id = $4,
            query = COALESCE($5, query),
            layout = COALESCE($6, layout),
            fields = CASE WHEN $7 THEN $8 ELSE fields END,
            version = version + 1,
            updated_at = now()
        WHERE project_id = $1 AND id = $2
        "#,
    )
    .bind(project_id)
    .bind(view_id)
    .bind(&name)
    .bind(owner)
    .bind(request.query.as_ref().map(|query| json!(query)))
    .bind(&request.layout)
    .bind(fields.is_some())
    .bind(fields.flatten())
    .execute(&mut *write.transaction)
    .await?;
    let actor = write.actor.clone();
    record_audit_for(
        &mut write.transaction,
        &actor,
        "saved_view",
        "saved_view.updated",
        view_id,
        json!({}),
    )
    .await?;
    let row = view_for_update(
        &mut write.transaction,
        project_id,
        view_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::Internal("saved view just changed is missing"))?;
    let view = saved_view(row, principal.actor_id, sharing);
    write.finish(StatusCode::OK, &view).await
}

pub(super) async fn delete_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, view_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<DeleteViewRequest>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemView,
            None,
            resource_bound_scope("saved_views", view_id, "delete")?,
            &request,
        )
        .await
    );
    let row = editable_view(&mut write, project_id, view_id, principal.actor_id).await?;
    if row.builtin.is_some() {
        return Ok(refusal(
            StatusCode::CONFLICT,
            "builtin_view",
            "Built-in views can’t be deleted. Unpin it to take it out of your sidebar.".to_owned(),
            json!({}),
        ));
    }
    if request
        .expected_version
        .is_some_and(|expected| expected != row.version)
    {
        let sharing = can_share(&write)?;
        return Ok(view_version_conflict(saved_view(
            row,
            principal.actor_id,
            sharing,
        )));
    }
    sqlx::query("DELETE FROM saved_views WHERE project_id = $1 AND id = $2")
        .bind(project_id)
        .bind(view_id)
        .execute(&mut *write.transaction)
        .await?;
    let actor = write.actor.clone();
    record_audit_for(
        &mut write.transaction,
        &actor,
        "saved_view",
        "saved_view.deleted",
        view_id,
        json!({}),
    )
    .await?;
    write
        .finish(StatusCode::OK, &json!({ "deleted": true }))
        .await
}

pub(super) async fn pin_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, view_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<PinViewRequest>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::WorkItemView,
            None,
            resource_bound_scope("saved_views", view_id, "pin")?,
            &request,
        )
        .await
    );
    view_for_update(
        &mut write.transaction,
        project_id,
        view_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::NotFound("saved_view"))?;
    sqlx::query(
        r#"
        INSERT INTO saved_view_pins (project_id, view_id, actor_id, pinned, position)
        VALUES ($1, $2, $3, $4, COALESCE($5, 0))
        ON CONFLICT (view_id, actor_id) DO UPDATE
        SET pinned = EXCLUDED.pinned,
            position = COALESCE($5, saved_view_pins.position),
            updated_at = now()
        "#,
    )
    .bind(project_id)
    .bind(view_id)
    .bind(principal.actor_id)
    .bind(request.pinned)
    .bind(request.position)
    .execute(&mut *write.transaction)
    .await?;
    let row = view_for_update(
        &mut write.transaction,
        project_id,
        view_id,
        principal.actor_id,
    )
    .await?
    .ok_or(AppError::Internal("saved view just pinned is missing"))?;
    let view = saved_view(row, principal.actor_id, can_share(&write)?);
    write.finish(StatusCode::OK, &view).await
}

// ---------------------------------------------------------------------------
// My work

/// Most tasks each My work section shows; the section's total says how many
/// there are, and its link opens the rest.
const MY_WORK_LIMIT: i64 = 100;

#[derive(Clone, Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyWorkTask {
    pub id: Uuid,
    pub project_id: Uuid,
    pub project_key: String,
    pub project_name: String,
    pub key: String,
    pub title: String,
    pub status_id: Uuid,
    pub status_name: String,
    pub status_category: String,
    pub priority: String,
    pub start_date: Option<NaiveDate>,
    pub due_date: Option<NaiveDate>,
    pub overdue: bool,
    pub blocked: bool,
    pub milestone_name: Option<String>,
    pub assignee_name: Option<String>,
    pub claimed_by_id: Option<Uuid>,
    pub claimed_by_name: Option<String>,
    pub claimed_by_kind: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyWorkSection {
    pub items: Vec<MyWorkTask>,
    pub total: i64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MyWork {
    /// Unfinished tasks assigned to you, soonest due first.
    pub assigned: MyWorkSection,
    /// Tasks you or your agents are working on right now.
    pub claimed: MyWorkSection,
    /// Tasks in review that you asked for or watch, assigned to someone else.
    pub review: MyWorkSection,
}

/// The tasks the My work sections read. `$1` is the reader, `$2` the
/// projects to look in.
const MY_WORK_SELECT: &str = r#"
    SELECT item.id, item.project_id, project.project_key, project.name AS project_name,
           project.project_key || '-' || item.number::text AS key, item.title,
           status.id AS status_id, status.name AS status_name,
           status.semantic_category AS status_category, item.priority,
           item.start_date, item.due_date,
           COALESCE(item.due_date < current_date
                    AND status.semantic_category NOT IN ('completed', 'canceled'), false) AS overdue,
           (item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER}) AS blocked,
           (SELECT milestone.name FROM project_milestones milestone
             WHERE milestone.project_id = item.project_id AND milestone.id = item.milestone_id
           ) AS milestone_name,
           assignee.display_name AS assignee_name,
           claim.actor_id AS claimed_by_id,
           COALESCE(claimer.display_name, CASE WHEN claim.actor_id IS NULL THEN NULL ELSE 'Unknown' END)
               AS claimed_by_name,
           claimer_actor.kind AS claimed_by_kind,
           item.updated_at::text AS updated_at
    FROM work_items item
    JOIN projects project ON project.id = item.project_id
    JOIN work_item_statuses status
      ON status.project_id = item.project_id AND status.id = item.status_id
    LEFT JOIN project_memberships assignee
      ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
    LEFT JOIN work_item_claims claim
      ON claim.work_item_id = item.id AND claim.expires_at > now()
    LEFT JOIN project_memberships claimer
      ON claimer.project_id = item.project_id AND claimer.actor_id = claim.actor_id
    LEFT JOIN actors claimer_actor ON claimer_actor.id = claim.actor_id
    WHERE item.project_id = ANY($2) AND item.archived_at IS NULL
"#;

/// Unfinished, in review that you asked for or watch, and assigned to someone
/// else (yours is under Assigned).
const REVIEW_WHERE: &str = r#"
    AND status.semantic_category IN ('review', 'verification')
    AND item.assignee_actor_id IS DISTINCT FROM $1
    AND (item.created_by_actor_id = $1
         OR EXISTS (SELECT 1 FROM work_item_watchers watcher
                     WHERE watcher.work_item_id = item.id AND watcher.actor_id = $1))
"#;

/// Claimed by the reader or by an agent they connected.
const CLAIMED_WHERE: &str = r#"
    AND (claim.actor_id = $1
         OR claim.actor_id IN (SELECT connection.actor_id FROM external_agent_connections connection
                                WHERE connection.created_by_actor_id = $1
                                  AND connection.status <> 'removed'))
"#;

const ASSIGNED_WHERE: &str = r#"
    AND item.assignee_actor_id = $1
    AND status.semantic_category NOT IN ('completed', 'canceled')
"#;

/// The projects a person reads tasks in: enabled memberships in workspaces
/// they're still in, archived projects left out.
pub(crate) async fn my_projects(
    executor: impl PgExecutor<'_>,
    actor_id: Uuid,
) -> Result<Vec<Uuid>, AppError> {
    Ok(sqlx::query_scalar::<_, Uuid>(
        r#"
        SELECT project.id
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $1 AND membership.enabled
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $1
         AND organization_membership.enabled
        WHERE project.archived_at IS NULL
        ORDER BY project.id
        LIMIT 500
        "#,
    )
    .bind(actor_id)
    .fetch_all(executor)
    .await?)
}

async fn my_work_section(
    pool: &PgPool,
    reader: Uuid,
    projects: &[Uuid],
    condition: &str,
    order: &str,
) -> Result<MyWorkSection, AppError> {
    let select = MY_WORK_SELECT.replace("{HAS_OPEN_BLOCKER}", HAS_OPEN_BLOCKER);
    let items =
        sqlx::query_as::<_, MyWorkTask>(&format!("{select} {condition} ORDER BY {order} LIMIT $3"))
            .bind(reader)
            .bind(projects)
            .bind(MY_WORK_LIMIT)
            .fetch_all(pool)
            .await?;
    let total = sqlx::query_scalar::<_, i64>(&format!(
        r#"SELECT count(*) FROM work_items item
           JOIN work_item_statuses status
             ON status.project_id = item.project_id AND status.id = item.status_id
           LEFT JOIN work_item_claims claim
             ON claim.work_item_id = item.id AND claim.expires_at > now()
           WHERE item.project_id = ANY($2) AND item.archived_at IS NULL {condition}
             AND $1::uuid IS NOT NULL"#
    ))
    .bind(reader)
    .bind(projects)
    .fetch_one(pool)
    .await?;
    Ok(MyWorkSection { items, total })
}

const PRIORITY_RANK: &str = "CASE item.priority WHEN 'urgent' THEN 0 WHEN 'high' THEN 1 \
     WHEN 'medium' THEN 2 WHEN 'low' THEN 3 ELSE 4 END";

/// Everything that's yours across `projects`.
pub(crate) async fn my_work(
    pool: &PgPool,
    reader: Uuid,
    projects: &[Uuid],
) -> Result<MyWork, AppError> {
    let projects = &crate::search::projects_showing_tasks(pool, reader, projects).await?[..];
    let due_first = format!(
        "item.due_date IS NULL, item.due_date, {PRIORITY_RANK}, item.updated_at DESC, item.id"
    );
    Ok(MyWork {
        assigned: my_work_section(pool, reader, projects, ASSIGNED_WHERE, &due_first).await?,
        claimed: my_work_section(
            pool,
            reader,
            projects,
            CLAIMED_WHERE,
            "claim.claimed_at DESC, item.id",
        )
        .await?,
        review: my_work_section(
            pool,
            reader,
            projects,
            REVIEW_WHERE,
            "item.updated_at, item.id",
        )
        .await?,
    })
}

pub(super) async fn get_my_work(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
) -> Result<Json<MyWork>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let projects = my_projects(&state.pool, principal.actor_id).await?;
    Ok(Json(
        my_work(&state.pool, principal.actor_id, &projects).await?,
    ))
}

// ---------------------------------------------------------------------------
// Timeline

/// The longest stretch one timeline request covers, and the most tasks it
/// draws; a busier project narrows by milestone or dates.
const MAX_TIMELINE_DAYS: i64 = 731;
const MAX_TIMELINE_TASKS: i64 = 1_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TimelineQuery {
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    milestone_id: Option<Uuid>,
    include_done: Option<bool>,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimelineTask {
    id: Uuid,
    key: String,
    title: String,
    status_id: Uuid,
    status_name: String,
    status_category: String,
    priority: String,
    start_date: Option<NaiveDate>,
    due_date: Option<NaiveDate>,
    estimate: Option<f64>,
    milestone_id: Option<Uuid>,
    parent_id: Option<Uuid>,
    assignee_name: Option<String>,
    blocked: bool,
    version: i64,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimelineLink {
    blocking_id: Uuid,
    blocked_id: Uuid,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TimelineResponse {
    from: NaiveDate,
    to: NaiveDate,
    estimate_unit: String,
    milestones: Vec<MilestoneView>,
    tasks: Vec<TimelineTask>,
    /// Blocks between the tasks shown: the blocking task must finish first.
    dependencies: Vec<TimelineLink>,
    /// Dependencies AI planning suggests and nobody has accepted yet, drawn
    /// dashed. Only those the reader may see (W10).
    suggested_dependencies: Vec<TimelineLink>,
    /// Tasks in range beyond the most one request draws.
    truncated: bool,
    /// Tasks (in the chosen milestone, or the project) with no dates at all.
    undated: i64,
    can_edit: bool,
}

pub(super) async fn get_timeline(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<TimelineQuery>,
) -> Result<Json<TimelineResponse>, AppError> {
    state
        .rate_limits
        .check_expensive_read(principal.session_id)?;
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let today = Utc::now().date_naive();
    let from = query.from.unwrap_or(today - Duration::days(14));
    let to = query.to.unwrap_or(today + Duration::days(120));
    if to < from || (to - from).num_days() > MAX_TIMELINE_DAYS {
        return Err(AppError::Validation("to"));
    }
    let mut connection = state.pool.acquire().await?;
    if let Some(milestone_id) = query.milestone_id
        && planning::milestone(&mut connection, project_id, milestone_id)
            .await?
            .is_none()
    {
        return Err(AppError::Validation("milestoneId"));
    }
    let include_done = query.include_done.unwrap_or(true);
    let mut tasks = sqlx::query_as::<_, TimelineTask>(&format!(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key, item.title,
               status.id AS status_id, status.name AS status_name,
               status.semantic_category AS status_category, item.priority,
               item.start_date, item.due_date, item.estimate, item.milestone_id,
               item.parent_work_item_id AS parent_id,
               assignee.display_name AS assignee_name,
               (item.blocked_reason IS NOT NULL OR {HAS_OPEN_BLOCKER}) AS blocked,
               item.version
        FROM work_items item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        LEFT JOIN project_memberships assignee
          ON assignee.project_id = item.project_id AND assignee.actor_id = item.assignee_actor_id
        WHERE item.project_id = $1 AND item.archived_at IS NULL
          AND (item.start_date IS NOT NULL OR item.due_date IS NOT NULL)
          AND COALESCE(item.start_date, item.due_date) <= $3
          AND COALESCE(item.due_date, item.start_date) >= $2
          AND ($4::uuid IS NULL OR item.milestone_id = $4)
          AND ($5 OR status.semantic_category NOT IN ('completed', 'canceled'))
        ORDER BY COALESCE(item.start_date, item.due_date), item.number
        LIMIT $6
        "#
    ))
    .bind(project_id)
    .bind(from)
    .bind(to)
    .bind(query.milestone_id)
    .bind(include_done)
    .bind(MAX_TIMELINE_TASKS + 1)
    .fetch_all(&mut *connection)
    .await?;
    let truncated = tasks.len() as i64 > MAX_TIMELINE_TASKS;
    tasks.truncate(MAX_TIMELINE_TASKS as usize);
    let ids = tasks.iter().map(|task| task.id).collect::<Vec<_>>();
    let dependencies = sqlx::query_as::<_, TimelineLink>(
        r#"
        SELECT blocking_work_item_id AS blocking_id, blocked_work_item_id AS blocked_id
        FROM work_item_dependencies
        WHERE project_id = $1 AND blocking_project_id = $1
          AND blocked_work_item_id = ANY($2) AND blocking_work_item_id = ANY($2)
        ORDER BY blocking_work_item_id, blocked_work_item_id
        "#,
    )
    .bind(project_id)
    .bind(&ids)
    .fetch_all(&mut *connection)
    .await?;
    // Suggestions are checked against what this person may see, as on the graph.
    let suggested_dependencies = crate::ai::ready::suggested_blocks(
        &mut *connection,
        crate::work_item_planning::Reader {
            actor_id: principal.actor_id,
            connection_id: None,
        },
        project_id,
        &ids,
    )
    .await?
    .into_iter()
    .map(|(blocking_id, blocked_id)| TimelineLink {
        blocking_id,
        blocked_id,
    })
    .collect();
    let undated = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT count(*) FROM work_items item
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.archived_at IS NULL
          AND item.start_date IS NULL AND item.due_date IS NULL
          AND ($2::uuid IS NULL OR item.milestone_id = $2)
          AND status.semantic_category NOT IN ('completed', 'canceled')
        "#,
    )
    .bind(project_id)
    .bind(query.milestone_id)
    .fetch_one(&mut *connection)
    .await?;
    let mut milestones = planning::list_milestones(&mut connection, project_id, false).await?;
    if let Some(milestone_id) = query.milestone_id {
        milestones.retain(|milestone| milestone.id == milestone_id);
    }
    let (_, estimate_unit) =
        crate::work_item_workflow::task_settings(&mut connection, project_id).await?;
    Ok(Json(TimelineResponse {
        from,
        to,
        estimate_unit,
        milestones,
        tasks,
        dependencies,
        suggested_dependencies,
        truncated,
        undated,
        can_edit: capabilities(&membership)?.edit && membership.project_archived_at.is_none(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_views_read_as_list_queries() {
        let reader = Uuid::now_v7();
        for builtin in &BUILTIN_VIEWS {
            let params = builtin
                .query
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>();
            assert!(
                list::saved_query_spec(&params, reader).is_ok(),
                "{} is a list query",
                builtin.key
            );
        }
    }

    #[test]
    fn me_means_whoever_opens_the_view() {
        let reader = Uuid::now_v7();
        let other = Uuid::now_v7();
        let params = BTreeMap::from([
            ("assigneeActorId".to_owned(), "me".to_owned()),
            ("notAssigneeActorId".to_owned(), format!("none,me,{other}")),
            ("sort".to_owned(), "due".to_owned()),
        ]);
        let resolved = resolved_params(&params, reader);
        assert_eq!(resolved["assigneeActorId"], reader.to_string());
        assert_eq!(
            resolved["notAssigneeActorId"],
            format!("none,{reader},{other}")
        );
        assert_eq!(resolved["sort"], "due");
        assert!(list::saved_query_spec(&params, reader).is_ok());
    }

    #[test]
    fn saved_queries_refuse_what_the_list_refuses() {
        let reader = Uuid::now_v7();
        for (key, value) in [
            ("sort", "sideways"),
            ("groupBy", "mood"),
            ("columnsBy", "parent"),
            ("lanes", "status"),
            ("dueBefore", "tomorrow"),
            ("cursor", "abc"),
            ("color", "blue"),
        ] {
            let params = BTreeMap::from([(key.to_owned(), value.to_owned())]);
            assert!(
                list::saved_query_spec(&params, reader).is_err(),
                "{key}={value} is refused"
            );
        }
        let board = BTreeMap::from([
            ("columnsBy".to_owned(), "assignee".to_owned()),
            ("lanes".to_owned(), "priority".to_owned()),
            ("labelIds".to_owned(), "none".to_owned()),
        ]);
        assert!(list::saved_query_spec(&board, reader).is_ok());
        // Several values, and "none of", read like the list reads them.
        let several = BTreeMap::from([
            ("priority".to_owned(), "high,urgent".to_owned()),
            (
                "notSemanticCategory".to_owned(),
                "completed,canceled".to_owned(),
            ),
            ("anyLabelIds".to_owned(), format!("none,{}", Uuid::nil())),
            ("notMilestoneId".to_owned(), "none".to_owned()),
        ]);
        assert!(list::saved_query_spec(&several, reader).is_ok());
        for (key, value) in [
            ("priority", "high,whenever"),
            ("statusId", "none"),
            ("labelIds", "none,00000000-0000-0000-0000-000000000000"),
            ("notTypeId", "a,b"),
        ] {
            let params = BTreeMap::from([(key.to_owned(), value.to_owned())]);
            assert!(
                list::saved_query_spec(&params, reader).is_err(),
                "{key}={value} is refused"
            );
        }
    }

    #[test]
    fn fields_are_known_columns_each_once() {
        assert_eq!(
            normalize_fields(Some(&[
                "due".to_owned(),
                "due".to_owned(),
                "key".to_owned()
            ])),
            Ok(Some(vec!["due".to_owned(), "key".to_owned()]))
        );
        assert_eq!(normalize_fields(Some(&["title".to_owned()])), Err("fields"));
        assert_eq!(normalize_fields(None), Ok(None));
    }

    #[test]
    fn view_names_are_one_short_line() {
        assert_eq!(
            normalize_view_name("  Ready   now ").as_deref(),
            Some("Ready now")
        );
        assert!(normalize_view_name("   ").is_none());
        assert!(normalize_view_name(&"x".repeat(81)).is_none());
    }
}
