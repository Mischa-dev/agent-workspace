//! What an automation's webhook action may say about what an event is
//! about (X4's leftovers): whether the rule's owner can see it now, and a
//! task's, record's, page's, sheet's or board's details as they see them.
//!
//! A person's own outgoing webhooks are connections now (A2.6): they are
//! pushed by `delivery::push`, which re-checks access per event and sends
//! ids only.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

use crate::{
    AppState,
    error::AppError,
    events::{self, Subject},
};

/// Whether `owner` can see what the event is about right now, through
/// `connection` when they act through an agent connection.
pub(crate) async fn visible_to(
    pool: &PgPool,
    owner: Uuid,
    connection: Option<Uuid>,
    project_id: Uuid,
    subject: &Subject,
) -> Result<bool, AppError> {
    let seen = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT actor_can_access_project($1, $7, $2)
           AND ($3::uuid IS NULL OR conversation_visible_to($3, $1))
           AND ($4::uuid IS NULL OR pull_request_visible_to($4, $1))
           AND ($5::uuid IS NULL OR EXISTS (
               SELECT 1 FROM task_rows item WHERE item.id = $5 AND item.project_id = $2
           ))
           -- A task's or pull request's events only while the owner's role
           -- there still lets them see tasks, or code.
           AND ($5::uuid IS NULL OR EXISTS (
               SELECT 1 FROM project_memberships sees_task
               WHERE sees_task.project_id = $2 AND sees_task.actor_id = $1
                 AND sees_task.enabled AND role_allows(sees_task.role, sees_task.role_id, 'tasks.view')
           ))
           AND ($4::uuid IS NULL OR EXISTS (
               SELECT 1 FROM project_memberships sees_code
               WHERE sees_code.project_id = $2 AND sees_code.actor_id = $1
                 AND sees_code.enabled AND role_allows(sees_code.role, sees_code.role_id, 'code.view')
           ))
           -- A record's events (AWD-110) while it's in the project, in a
           -- collection that isn't archived.
           AND ($6::uuid IS NULL OR EXISTS (
               SELECT 1 FROM collection_records record
               JOIN collections collection
                 ON collection.project_id = record.project_id
                AND collection.id = record.collection_id
               WHERE record.id = $6 AND record.project_id = $2
                 AND collection.archived_at IS NULL
           ))
        "#,
    )
    .bind(owner)
    .bind(project_id)
    .bind(subject.conversation_id)
    .bind(subject.pull_request_id)
    .bind(subject.work_item_id)
    .bind(subject.record_id)
    .bind(connection)
    .fetch_one(crate::db::scoped(pool))
    .await?;
    if !seen {
        return Ok(false);
    }
    // A page, sheet or board (K3.5): as the thing registry shows it to the
    // owner in that project now, its module on for them there.
    let document = [
        ("page", subject.page_id),
        ("sheet", subject.sheet_id),
        ("whiteboard", subject.whiteboard_id),
    ]
    .into_iter()
    .find_map(|(kind, id)| id.map(|id| (kind, id)));
    let Some((kind, id)) = document else {
        return Ok(true);
    };
    let mut connection = crate::db::scoped_tx(pool).await?;
    let Some(modules) =
        crate::modules::state::for_project(&mut connection, project_id, owner).await?
    else {
        return Ok(false);
    };
    let projects = [project_id];
    let workspaces = std::collections::BTreeMap::new();
    let viewer = crate::things::Viewer {
        actor_id: owner,
        projects: &projects,
        task_projects: &[],
        modules,
        workspaces: &workspaces,
        addresses: crate::addresses::Addresses::LEGACY,
    };
    Ok(crate::things::get(&mut connection, &viewer, id)
        .await?
        .is_some_and(|thing| thing.kind == kind && thing.project_id == Some(project_id)))
}

#[derive(FromRow)]
struct TaskData {
    id: Uuid,
    key: String,
    title: String,
    priority: String,
    status: String,
    status_category: String,
    assignee_actor_id: Option<Uuid>,
    assignee_name: Option<String>,
    archived: bool,
    version: i64,
    updated_at: DateTime<Utc>,
}

pub(crate) async fn task_data(
    state: &AppState,
    project_id: Uuid,
    work_item_id: Uuid,
) -> Result<Option<Value>, AppError> {
    let Some(task) = sqlx::query_as::<_, TaskData>(
        r#"
        SELECT item.id, project.project_key || '-' || item.number::text AS key, item.title,
               item.priority, status.name AS status, status.semantic_category AS status_category,
               item.assignee_actor_id,
               (SELECT display_name FROM users WHERE actor_id = item.assignee_actor_id)
                   AS assignee_name,
               item.archived_at IS NOT NULL AS archived, item.version, item.updated_at
        FROM task_rows item
        JOIN projects project ON project.id = item.project_id
        JOIN work_item_statuses status
          ON status.project_id = item.project_id AND status.id = item.status_id
        WHERE item.project_id = $1 AND item.id = $2
        "#,
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    else {
        return Ok(None);
    };
    let labels = sqlx::query_scalar::<_, String>(
        "SELECT name FROM work_item_active_labels WHERE project_id = $1 AND work_item_id = $2 ORDER BY lower(name)",
    )
    .bind(project_id)
    .bind(work_item_id)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    let origin = events::public_origin(state);
    Ok(Some(json!({
        "id": task.id,
        "key": task.key,
        "title": task.title,
        "status": task.status,
        "statusCategory": task.status_category,
        "priority": task.priority,
        "assignee": task.assignee_actor_id.map(|id| json!({ "id": id, "name": task.assignee_name })),
        "labels": labels,
        "archived": task.archived,
        "version": task.version,
        "updatedAt": task.updated_at,
        "url": format!(
            "{origin}{}",
            crate::addresses::Target::task(project_id, task.id).legacy_path()
        ),
    })))
}

/// A record as a webhook sends it (AWD-110): values by field name, as agents
/// read them.
pub(crate) async fn record_data(
    state: &AppState,
    project_id: Uuid,
    collection_id: Uuid,
    record_id: Uuid,
) -> Result<Option<Value>, AppError> {
    use crate::collections::{self, records};
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let Some(collection) = collections::get(&mut *connection, project_id, collection_id).await?
    else {
        return Ok(None);
    };
    let Some(record) = records::by_ids(
        &mut *connection,
        project_id,
        collection_id,
        &[record_id],
        false,
    )
    .await?
    .pop() else {
        return Ok(None);
    };
    let definitions =
        collections::field_definitions(&mut connection, project_id, collection_id, false).await?;
    let references = records::references(
        &mut connection,
        project_id,
        &definitions,
        std::slice::from_ref(&record),
    )
    .await?
    .into_iter()
    .map(|reference| (reference.id, reference))
    .collect();
    let line = records::RecordLine::of(&record, &definitions, &references, None);
    let origin = events::public_origin(state);
    Ok(Some(json!({
        "id": line.id,
        "number": line.number,
        "title": line.title,
        "collection": { "id": collection.id, "name": collection.name },
        "fields": line.fields,
        "archived": line.archived,
        "version": line.version,
        "updatedAt": line.updated_at,
        "url": format!(
            "{origin}{}",
            crate::addresses::Target::space(
                project_id,
                crate::addresses::Place::Record {
                    collection: collection_id,
                    record: record_id,
                },
            )
            .legacy_path()
        ),
    })))
}

/// A page, sheet or board as a webhook sends it (K3.5): its title, version
/// and address as the owner sees it through the thing registry, never its
/// content. `None` once it's gone or out of the owner's sight.
pub(crate) async fn document_data(
    state: &AppState,
    owner: Uuid,
    project_id: Uuid,
    subject: &Subject,
) -> Result<Option<(&'static str, Value)>, AppError> {
    let Some((key, id)) = [
        ("page", subject.page_id),
        ("sheet", subject.sheet_id),
        ("whiteboard", subject.whiteboard_id),
    ]
    .into_iter()
    .find_map(|(key, id)| id.map(|id| (key, id))) else {
        return Ok(None);
    };
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let modules = crate::modules::state::for_project(&mut connection, project_id, owner)
        .await?
        .unwrap_or_default();
    let projects = [project_id];
    let workspaces = std::collections::BTreeMap::new();
    let viewer = crate::things::Viewer {
        actor_id: owner,
        projects: &projects,
        task_projects: &[],
        modules,
        workspaces: &workspaces,
        addresses: crate::addresses::Addresses::LEGACY,
    };
    let Some(thing) = crate::things::get(&mut connection, &viewer, id).await? else {
        return Ok(None);
    };
    if thing.kind != key {
        return Ok(None);
    }
    let origin = events::public_origin(state);
    Ok(Some((
        key,
        json!({
            "id": thing.id,
            "title": thing.title,
            "version": thing.version,
            "updatedAt": thing.updated_at,
            "url": format!("{origin}{}", thing.href),
        }),
    )))
}
