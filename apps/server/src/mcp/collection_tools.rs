//! Collection tools (AWD-90): five tools that work for every collection a
//! project makes, so the tool list stays the same however many there are.
//!
//! - `collection_list`: a project's collections with their record counts.
//! - `collection_get`: one collection's fields, views and definition.
//! - `collection_create` and `collection_update`: create a collection with its fields and views in
//!   one call, or change any of them (`collections.build`).
//! - `record_list`: records found on the server, a page at a time.
//! - `record_create_many` and `record_update_many`: add or change up to 100 records, all or nothing
//!   (`collections.edit`).
//!
//! The rules are `crate::collections`, the same the web app's routes use.
//! A collection is looked up only among the project's own
//! (`principal.project_id`), and fields, views and records only among the
//! collection's, so an ID from anywhere else never matches.

use std::collections::{HashMap, HashSet};

use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    McpPrincipal,
    common::{
        ARCHIVED_PROJECT_MESSAGE, StoredRole, ToolResult, insert_audit, link_url, mark_via,
        parse_id, role_allows, string_or_number,
    },
    talk::can_write_in,
    writes::{REQUEST_ID_MESSAGE, parse_request_id},
};
use crate::{
    AppState,
    collections::{
        self, ChangeOutcome, Collection, CollectionChange, CollectionSummary, CreateOutcome,
        NewCollection,
        definition::{self, CollectionDefinition, FieldWrite, FilterWrite, SortWrite, ViewWrite},
        query::{self, Archived, Spec},
        records::{self, HistoryEntry, ProblemKind, RecordInput, RecordLine, RecordRef, Written},
        views::{self, ViewConfig},
    },
    error::AppError,
    fields::{self, FieldView},
    idempotency::{IdempotencyKey, IdempotencyOutcome, RequestHash, reserve, resource_bound_scope},
    memory, outbox,
    policy::Action,
    work_item_commands::lock_membership,
    work_item_fields::{self, Writer},
    work_item_labels::echo,
};

const MESSAGES_SPACE_MESSAGE: &str =
    "The Messages space has no collections. Pick a project with project_list.";
const BUILD_MESSAGE: &str = "Your role here can't build collections (collections.build): it can't create them or change their fields and views. permissions_get says what it can do.";
const EDIT_MESSAGE: &str =
    "Your role here can't edit records (collections.edit). permissions_get says what it can do.";

fn collection_url(state: &AppState, project_id: Uuid, collection_id: Uuid) -> String {
    link_url(
        state,
        None,
        &crate::addresses::Target::space(
            project_id,
            crate::addresses::Place::Collection(collection_id),
        ),
    )
}

/// AWD-75: whether Collections is off in the project, for its workspace or
/// the caller's role there.
async fn module_off(state: &AppState, principal: &McpPrincipal) -> Result<bool, AppError> {
    let mut connection = crate::db::scoped_tx(&state.pool).await?;
    let projects = [principal.project_id];
    Ok(
        crate::modules::state::ProjectModules::load(&mut connection, principal.actor_id, &projects)
            .await?
            .with(&projects, "collections")
            .is_empty(),
    )
}

enum Reading {
    Ok { project_key: String },
    Refused(&'static str),
}

/// Whether the caller may read the project's collections: anyone in it.
async fn read_access(state: &AppState, principal: &McpPrincipal) -> Result<Reading, AppError> {
    let messages_space = match memory::project_access(
        &state.pool,
        principal.actor_id,
        principal.project_id,
        Action::ProjectView,
    )
    .await
    {
        Ok(direct) => direct,
        Err(AppError::NotFound(_)) => return Ok(Reading::Refused("That project")),
        Err(error) => return Err(error),
    };
    if messages_space {
        return Ok(Reading::Refused(MESSAGES_SPACE_MESSAGE));
    }
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(principal.project_id)
            .fetch_one(crate::db::scoped(&state.pool))
            .await?;
    Ok(Reading::Ok { project_key })
}

/// Whether the caller may read tasks here: a collection whose records are
/// tasks reads only with that, as the task tools do.
async fn tasks_readable(state: &AppState, principal: &McpPrincipal) -> Result<bool, AppError> {
    match memory::project_access(
        crate::db::scoped(&state.pool),
        principal.actor_id,
        principal.project_id,
        Action::WorkItemView,
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(AppError::NotFound(_) | AppError::Forbidden) => Ok(false),
        Err(error) => Err(error),
    }
}

fn refused<T>(what: &'static str) -> ToolResult<T> {
    if what == MESSAGES_SPACE_MESSAGE {
        ToolResult::invalid("projectId", MESSAGES_SPACE_MESSAGE)
    } else {
        ToolResult::not_found(what)
    }
}

/// A write holding the project lock, with the role checked again under it.
struct Writing {
    transaction: Transaction<'static, Postgres>,
    project_key: String,
}

async fn begin_write<T>(
    state: &AppState,
    principal: &McpPrincipal,
    action: Action,
    denied: &str,
) -> Result<Result<Writing, ToolResult<T>>, AppError> {
    let project_id = principal.project_id;
    if !can_write_in(state, principal, project_id, action).await? {
        return Ok(Err(ToolResult::denied(denied)));
    }
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    mark_via(&mut transaction, principal).await?;
    let Some(membership) =
        lock_membership(&mut transaction, principal.actor_id, project_id).await?
    else {
        return Ok(Err(ToolResult::not_found("That project")));
    };
    if !membership.enabled {
        return Ok(Err(ToolResult::not_found("That project")));
    }
    if membership.project_archived_at.is_some() {
        return Ok(Err(ToolResult::conflict(ARCHIVED_PROJECT_MESSAGE)));
    }
    // Checked again under the project lock, with the role as it is now and
    // any agent limit.
    if !role_allows(
        principal,
        project_id,
        &StoredRole::of(&membership.role, membership.role_permissions.as_deref())
            .with_groups(membership.group_permissions.clone()),
        action,
    ) {
        return Ok(Err(ToolResult::denied(denied)));
    }
    let (kind, project_key) = sqlx::query_as::<_, (String, String)>(
        "SELECT kind, project_key FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .fetch_one(&mut *transaction)
    .await?;
    if kind == "direct" {
        return Ok(Err(ToolResult::invalid(
            "projectId",
            MESSAGES_SPACE_MESSAGE,
        )));
    }
    Ok(Ok(Writing {
        transaction,
        project_key,
    }))
}

fn request_key(value: Option<&str>) -> Option<IdempotencyKey> {
    match value {
        Some(value) => parse_request_id(value),
        None => parse_request_id(&Uuid::now_v7().to_string()),
    }
}

/// The collection `reference` names among the project's, or a not found
/// that lists them.
async fn find_collection<T>(
    connection: &mut PgConnection,
    project_id: Uuid,
    reference: &str,
) -> Result<Result<Collection, ToolResult<T>>, AppError> {
    match collections::find(&mut *connection, project_id, reference).await? {
        Some(collection) => Ok(Ok(collection)),
        None => Ok(Err(ToolResult::NotFound {
            message: format!(
                "No collection called \"{}\" here. Its collections: {}. collection_list shows them.",
                echo(reference),
                collections::active_names(&mut *connection, project_id).await?
            ),
        })),
    }
}

// ---------------------------------------------------------------------------
// collection_list

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CollectionListArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// Also list archived collections, which collection_update can restore.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectionLine {
    #[serde(flatten)]
    pub collection: CollectionSummary,
    pub url: String,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectionListValue {
    /// The project's collections in order, with how many records each has.
    pub collections: Vec<CollectionLine>,
    /// Whether you can create collections and change their fields and views.
    pub can_build: bool,
}

pub(crate) async fn collection_list(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: CollectionListArguments,
) -> ToolResult<CollectionListValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let result = async {
        if let Reading::Refused(what) = read_access(state, principal).await? {
            return Ok(refused(what));
        }
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("collection_list", "Collections"));
        }
        {
            let mut connection = crate::db::scoped_tx(&state.pool).await?;
            collections::tasks::ensure(&mut connection, principal.project_id, principal.actor_id)
                .await?;
            connection.commit().await?;
        }
        let listed = collections::list(
            crate::db::scoped(&state.pool),
            principal.project_id,
            arguments.include_archived.unwrap_or(false),
        )
        .await?;
        // Nothing in an archived project changes, whatever the role says.
        let project_open =
            sqlx::query_scalar::<_, bool>("SELECT archived_at IS NULL FROM projects WHERE id = $1")
                .bind(principal.project_id)
                .fetch_one(crate::db::scoped(&state.pool))
                .await?;
        let can_build = project_open
            && can_write_in(
                state,
                principal,
                principal.project_id,
                Action::CollectionBuild,
            )
            .await?;
        Ok::<_, AppError>(ToolResult::ok(CollectionListValue {
            collections: listed
                .into_iter()
                .map(|collection| CollectionLine {
                    url: collection
                        .id
                        .parse::<Uuid>()
                        .map(|id| collection_url(state, principal.project_id, id))
                        .unwrap_or_default(),
                    collection,
                })
                .collect(),
            can_build,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "collection_list"))
}

// ---------------------------------------------------------------------------
// collection_describe

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CollectionDescribeArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The collection, by name or ID.
    pub collection: String,
    /// Also list archived fields, which collection_update can restore.
    pub include_archived: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ViewLine {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub view_type: String,
    pub version: i64,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectionDescribeValue {
    pub collection: CollectionSummary,
    /// Its fields in order, each with its type and options. record_update_many
    /// sets values by these names; record_list filters on them.
    pub fields: Vec<FieldView>,
    /// Its saved views, in order. record_list reads through one by name.
    pub views: Vec<ViewLine>,
    /// Records that are archived (record_list with archived: "only").
    pub archived_records: i64,
    /// The collection by name, in the shape collection_create takes (name,
    /// description, titleName, fields, views): pass it there to make the
    /// same collection in another project.
    #[schemars(with = "Map<String, Value>")]
    pub definition: CollectionDefinition,
    pub can_edit_records: bool,
    pub can_build: bool,
    pub url: String,
}

pub(crate) async fn collection_describe(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: CollectionDescribeArguments,
) -> ToolResult<CollectionDescribeValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let project_id = principal.project_id;
    let result = async {
        if let Reading::Refused(what) = read_access(state, principal).await? {
            return Ok(refused(what));
        }
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("collection_describe", "Collections"));
        }
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        let collection =
            match find_collection(&mut connection, project_id, &arguments.collection).await? {
                Ok(collection) => collection,
                Err(refusal) => return Ok(refusal),
            };
        let definitions =
            collections::field_definitions(&mut connection, project_id, collection.id, true)
                .await?;
        let saved = views::list(&mut *connection, project_id, collection.id).await?;
        let definition = definition::describe(
            &mut connection,
            project_id,
            &collection,
            &definitions,
            &saved,
        )
        .await?;
        let (records, archived_records) = sqlx::query_as::<_, (i64, i64)>(
            "SELECT count(*) FILTER (WHERE archived_at IS NULL), count(*) FILTER (WHERE archived_at IS NOT NULL) \
             FROM collection_records WHERE collection_id = $1",
        )
        .bind(collection.id)
        .fetch_one(&mut *connection)
        .await?;
        drop(connection);
        let project_open = sqlx::query_scalar::<_, bool>(
            "SELECT archived_at IS NULL FROM projects WHERE id = $1",
        )
        .bind(project_id)
        .fetch_one(crate::db::scoped(&state.pool))
        .await?;
        let can_edit_records = project_open
            && !collection.archived
            && can_write_in(state, principal, project_id, Action::CollectionRecordEdit).await?;
        let can_build = project_open
            && can_write_in(state, principal, project_id, Action::CollectionBuild).await?;
        let include_archived = arguments.include_archived.unwrap_or(false);
        Ok::<_, AppError>(ToolResult::ok(CollectionDescribeValue {
            url: collection_url(state, project_id, collection.id),
            collection: CollectionSummary::of(&collection, records),
            fields: definitions
                .iter()
                .filter(|field| include_archived || !field.archived)
                .map(FieldView::from)
                .collect(),
            views: saved
                .iter()
                .map(|view| ViewLine {
                    id: view.id.to_string(),
                    name: view.name.clone(),
                    view_type: view.view_type.as_str().to_owned(),
                    version: view.version,
                })
                .collect(),
            archived_records,
            definition,
            can_edit_records,
            can_build,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "collection_describe"))
}

// ---------------------------------------------------------------------------
// collection_write

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CollectionWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The collection to change, by name or ID. Leave it out to create one.
    pub collection: Option<String>,
    /// A new collection's name, or a new name: 1 to 80 characters, unique
    /// among the project's collections.
    pub name: Option<String>,
    /// What it's for, on one line. "" clears it.
    pub description: Option<String>,
    /// What the title column is called, such as Name, Version or Tester.
    /// "Name" when left out.
    pub title_name: Option<String>,
    /// Fields to add or change, each matched by name (or by `field`).
    pub fields: Option<Vec<FieldWrite>>,
    /// Views to add, change or remove, each matched by name (or by `view`).
    /// A new collection without views starts with an "All records" table.
    pub views: Option<Vec<ViewWrite>>,
    /// Its place among the project's collections, 1 being first.
    pub position: Option<i32>,
    /// true archives it (hidden with its records, nothing deleted), false
    /// restores it.
    pub archived: Option<bool>,
    /// true makes its records tasks: each gets a status, assignee,
    /// priority, dates, labels and a key like AWD-12, and is then changed
    /// as a task (record_update_many, or task_create with collection and
    /// task_update).
    /// false turns it off: its tasks are put away with their keys and
    /// history, and come back when it is turned on again. Not while one of
    /// them is claimed or has a run.
    pub acts_like_task: Option<bool>,
    /// For a collection that acts like a task: the statuses its tasks use,
    /// by name or ID, from the space's workflow (workflow_get lists them).
    /// [] uses them all.
    pub task_statuses: Option<Vec<String>>,
    /// The status its new tasks start in, one of taskStatuses. "" starts
    /// them in the space's default (or the first status it uses).
    pub start_status: Option<String>,
    /// The version from collection_get: if the collection changed
    /// since, a write that would change anything (fields and views too) is
    /// a conflict. Leave it out to change the current one.
    pub expected_version: Option<i64>,
    /// A stable ID so a retry never creates or changes twice.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CollectionWriteValue {
    pub collection: CollectionSummary,
    pub created: bool,
    /// What changed, in order, such as `field "Severity" added`. Empty
    /// when nothing needed to change.
    pub changes: Vec<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

fn stale_conflict<T>(name: &str, version: i64) -> ToolResult<T> {
    ToolResult::conflict(format!(
        "\"{name}\" changed since you read it (now version {version}). Read it again with collection_get and retry."
    ))
}

fn apply_refusal<T>(problem: definition::ApplyProblem) -> ToolResult<T> {
    match problem.kind {
        definition::ProblemKind::Invalid => ToolResult::invalid(&problem.field, problem.message),
        definition::ProblemKind::NotFound => ToolResult::NotFound {
            message: problem.message,
        },
        definition::ProblemKind::Full => ToolResult::conflict(problem.message),
    }
}

/// taskStatuses and startStatus as a workflow change, the part left out
/// kept as it is; None when neither is given.
async fn workflow_change<T>(
    connection: &mut PgConnection,
    project_id: Uuid,
    arguments: &CollectionWriteArguments,
    current: &Collection,
) -> Result<Result<Option<collections::task_like::Workflow>, ToolResult<T>>, AppError> {
    if arguments.task_statuses.is_none() && arguments.start_status.is_none() {
        return Ok(Ok(None));
    }
    let statuses = match &arguments.task_statuses {
        None => current.task_statuses.clone(),
        Some(names) if names.is_empty() => None,
        Some(names) => {
            match collections::task_like::resolve_statuses(connection, project_id, names).await? {
                Ok(ids) => Some(ids),
                Err(message) => return Ok(Err(ToolResult::invalid("taskStatuses", message))),
            }
        }
    };
    let start = match arguments.start_status.as_deref().map(str::trim) {
        None => current.start_status_id,
        Some("") => None,
        Some(name) => {
            match collections::task_like::resolve_statuses(
                connection,
                project_id,
                &[name.to_owned()],
            )
            .await?
            {
                Ok(ids) => ids.first().copied(),
                Err(message) => return Ok(Err(ToolResult::invalid("startStatus", message))),
            }
        }
    };
    Ok(Ok(Some(collections::task_like::Workflow {
        statuses,
        start,
    })))
}

pub(crate) async fn collection_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: CollectionWriteArguments,
) -> ToolResult<CollectionWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let name = match arguments.name.as_deref() {
        Some(value) => match collections::normalize_name(value) {
            Some(name) => Some(name),
            None => {
                return ToolResult::invalid(
                    "name",
                    format!(
                        "Give the collection a name of 1 to {} characters on one line, that isn't an ID.",
                        collections::MAX_NAME_CHARACTERS
                    ),
                );
            }
        },
        None => None,
    };
    let description = match arguments.description.as_deref() {
        Some(value) => match collections::normalize_description(value) {
            Some(text) => Some(text),
            None => {
                return ToolResult::invalid(
                    "description",
                    format!(
                        "Keep the description to one line of at most {} characters.",
                        collections::MAX_DESCRIPTION_CHARACTERS
                    ),
                );
            }
        },
        None => None,
    };
    let title_name = match arguments.title_name.as_deref() {
        Some(value) => match collections::normalize_title_name(value) {
            Some(text) => Some(text),
            None => {
                return ToolResult::invalid(
                    "titleName",
                    format!(
                        "Call the title column 1 to {} characters, on one line.",
                        collections::MAX_TITLE_NAME_CHARACTERS
                    ),
                );
            }
        },
        None => None,
    };
    let field_writes = arguments.fields.clone().unwrap_or_default();
    let view_writes = arguments.views.clone().unwrap_or_default();
    if let Some(message) = definition::duplicate_name(&field_writes, &view_writes) {
        return ToolResult::invalid("fields", message);
    }
    let Some(key) = request_key(arguments.client_request_id.as_deref()) else {
        return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE);
    };
    let project_id = principal.project_id;
    let result = async {
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("collection_write", "Collections"));
        }
        let Writing {
            mut transaction,
            project_key,
        } = match begin_write(state, principal, Action::CollectionBuild, BUILD_MESSAGE).await? {
            Ok(writing) => writing,
            Err(refusal) => return Ok(refusal),
        };
        let scope = resource_bound_scope("mcp_collections", project_id, "collection_write")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation =
            match reserve(&mut transaction, principal.actor_id, &scope, &key, request_hash).await {
                Ok(IdempotencyOutcome::Replay(replay)) => {
                    let mut value = replay.decode::<CollectionWriteValue>()?;
                    transaction.commit().await?;
                    value.idempotent_replay = true;
                    return Ok(ToolResult::ok(value));
                }
                Ok(IdempotencyOutcome::Reserved(reservation)) => reservation,
                Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                    return Ok(ToolResult::invalid(
                        "clientRequestId",
                        "That clientRequestId was already used for a different change. Use a new one.",
                    ));
                }
                Err(AppError::Conflict("idempotency_key", "command in progress")) => {
                    return Ok(ToolResult::conflict(
                        "The same change is still being applied. Try again in a moment.",
                    ));
                }
                Err(error) => return Err(error),
            };
        let writer = Writer {
            actor_id: principal.actor_id,
            project_key: &project_key,
        };
        let mut changes = Vec::new();
        // The collection's version when the caller's expectedVersion is
        // behind it: a write that would change anything is then a conflict.
        let mut stale = None::<i64>;
        let (collection, created) = match arguments.collection.as_deref() {
            None => {
                // Create one, with its fields and views.
                let Some(name) = name else {
                    return Ok(ToolResult::invalid(
                        "name",
                        "Give the new collection a name, or pass collection to change one.",
                    ));
                };
                if arguments.archived.is_some() || arguments.expected_version.is_some() {
                    return Ok(ToolResult::invalid(
                        "collection",
                        "archived and expectedVersion change a collection that exists; name it with collection.",
                    ));
                }
                let collection = match collections::create(
                    &mut transaction,
                    project_id,
                    principal.actor_id,
                    NewCollection {
                        name,
                        description: description.unwrap_or_default(),
                        title_name: title_name
                            .unwrap_or_else(|| collections::DEFAULT_TITLE_NAME.to_owned()),
                        first_view: view_writes.is_empty(),
                    },
                )
                .await?
                {
                    CreateOutcome::Created(collection) => collection,
                    CreateOutcome::NameTaken(collection) => {
                        return Ok(ToolResult::conflict(format!(
                            "{project_key} already has a collection called \"{0}\". To change it, pass collection: \"{0}\".",
                            collection.name
                        )));
                    }
                    CreateOutcome::ProjectFull => {
                        return Ok(ToolResult::conflict(format!(
                            "{project_key} has {} collections already, or keeps {} with archived ones. Archive one first.",
                            collections::MAX_ACTIVE_COLLECTIONS,
                            collections::MAX_COLLECTION_ROWS
                        )));
                    }
                };
                // Switching on makes a task for each record.
                if arguments.acts_like_task == Some(true)
                    && !can_write_in(state, principal, project_id, Action::WorkItemCreate).await?
                {
                    return Ok(ToolResult::denied(
                        "Records that act like tasks are tasks: only people who can make tasks here can switch it on.",
                    ));
                }
                let workflow = match workflow_change(&mut transaction, project_id, &arguments, &collection).await? {
                    Ok(workflow) => workflow,
                    Err(refusal) => return Ok(refusal),
                };
                let collection = if arguments.position.is_some()
                    || arguments.acts_like_task == Some(true)
                    || workflow.is_some()
                {
                    match collections::change(
                        &mut transaction,
                        project_id,
                        collection.id,
                        principal.actor_id,
                        CollectionChange {
                            position: arguments.position,
                            acts_like_task: arguments.acts_like_task,
                            workflow,
                            ..CollectionChange::default()
                        },
                    )
                    .await?
                    {
                        ChangeOutcome::Changed { collection, .. } => *collection,
                        _ => collection,
                    }
                } else {
                    collection
                };
                changes.push(format!("collection \"{}\" created", collection.name));
                (collection, true)
            }
            Some(reference) => {
                let found = match find_collection(&mut transaction, project_id, reference).await? {
                    Ok(collection) => collection,
                    Err(refusal) => return Ok(refusal),
                };
                // Switching on makes a task for each record.
                if arguments.acts_like_task == Some(true)
                    && !can_write_in(state, principal, project_id, Action::WorkItemCreate).await?
                {
                    return Ok(ToolResult::denied(
                        "Records that act like tasks are tasks: only people who can make tasks here can switch it on.",
                    ));
                }
                let workflow = match workflow_change(&mut transaction, project_id, &arguments, &found).await? {
                    Ok(workflow) => workflow,
                    Err(refusal) => return Ok(refusal),
                };
                let change = CollectionChange {
                    name,
                    description,
                    title_name,
                    position: arguments.position,
                    archived: arguments.archived,
                    acts_like_task: arguments.acts_like_task,
                    workflow,
                    expected_version: arguments.expected_version,
                };
                // Read under the lock, before anything changes: field and
                // view changes are checked against this version too.
                let locked = collections::lock(&mut transaction, project_id, found.id)
                    .await?
                    .ok_or(AppError::NotFound("collection"))?;
                // The Tasks collection's name and settings are fixed. Its
                // fields are the project's task fields, changed by those who
                // may change the workflow.
                if let Some(message) = locked.read_only_message()
                    && !change.is_empty()
                {
                    return Ok(ToolResult::conflict(message));
                }
                if locked.is_tasks()
                    && !field_writes.is_empty()
                    && !can_write_in(state, principal, project_id, Action::WorkflowManage)
                        .await?
                {
                    return Ok(ToolResult::denied(
                        "Only people whose role can change the workflow can change the task fields.",
                    ));
                }
                if arguments
                    .expected_version
                    .is_some_and(|expected| expected != locked.version)
                {
                    stale = Some(locked.version);
                }
                let collection = if change.is_empty() {
                    locked
                } else {
                    match collections::change(
                        &mut transaction,
                        project_id,
                        found.id,
                        principal.actor_id,
                        change,
                    )
                    .await?
                    {
                        ChangeOutcome::Changed {
                            collection,
                            previous,
                            changed,
                        } => {
                            changes.push(match (previous.archived, collection.archived) {
                                (false, true) => format!("collection \"{}\" archived", collection.name),
                                (true, false) => format!("collection \"{}\" restored", collection.name),
                                _ => format!(
                                    "collection \"{}\" changed ({})",
                                    collection.name,
                                    changed.join(", ")
                                ),
                            });
                            *collection
                        }
                        ChangeOutcome::Unchanged(collection) => collection,
                        ChangeOutcome::NotFound => {
                            return Ok(ToolResult::not_found("That collection"));
                        }
                        ChangeOutcome::VersionConflict(collection) => {
                            return Ok(stale_conflict(&collection.name, collection.version));
                        }
                        ChangeOutcome::NameTaken(other) => {
                            return Ok(ToolResult::invalid(
                                "name",
                                format!("Another collection in {project_key} is already called \"{other}\"."),
                            ));
                        }
                        ChangeOutcome::ProjectFull => {
                            return Ok(ToolResult::conflict(format!(
                                "{project_key} has {} active collections already. Archive one first.",
                                collections::MAX_ACTIVE_COLLECTIONS
                            )));
                        }
                        ChangeOutcome::TaskLike(refusal) => {
                            return Ok(ToolResult::conflict(refusal));
                        }
                    }
                };
                (collection, false)
            }
        };
        if collection.archived && (!field_writes.is_empty() || !view_writes.is_empty()) {
            return Ok(ToolResult::conflict(format!(
                "\"{}\" is archived. Restore it (archived: false) to change its fields and views.",
                collection.name
            )));
        }
        if !field_writes.is_empty() || !view_writes.is_empty() {
            match definition::apply(
                &mut transaction,
                project_id,
                &collection,
                writer,
                &field_writes,
                &view_writes,
            )
            .await?
            {
                Ok(applied) => {
                    // Returning before the commit keeps none of it.
                    if let Some(version) = stale.filter(|_| !applied.is_empty()) {
                        return Ok(stale_conflict(&collection.name, version));
                    }
                    changes.extend(applied);
                }
                Err(problem) => return Ok(apply_refusal(problem)),
            }
        }
        if !changes.is_empty() {
            insert_audit(
                &mut transaction,
                principal,
                project_id,
                if created {
                    "collection.mcp_created"
                } else {
                    "collection.mcp_updated"
                },
                "collection",
                collection.id,
                json!({ "name": collection.name, "changes": changes }),
            )
            .await?;
            outbox::enqueue_collection_updated(&mut transaction, project_id, collection.id, &[])
                .await?;
        }
        let current = collections::get(&mut *transaction, project_id, collection.id)
            .await?
            .ok_or(AppError::Internal("written collection is missing"))?;
        let records = collections::record_count(&mut *transaction, collection.id).await?;
        let value = CollectionWriteValue {
            collection: CollectionSummary::of(&current, records),
            created,
            changes,
            url: collection_url(state, project_id, collection.id),
            idempotent_replay: false,
        };
        crate::idempotency::complete(
            &mut transaction,
            &reservation,
            axum::http::StatusCode::OK,
            &value,
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "collection_write"))
}

// ---------------------------------------------------------------------------
// record_query

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordQueryArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The collection, by name or ID.
    pub collection: String,
    /// A saved view, by name or ID: start from its filters, sort and
    /// grouping. Filters, sort and groupBy given here replace its own.
    pub view: Option<String>,
    /// Only these records, by number (#12) or ID: up to 100.
    pub records: Option<Vec<String>>,
    /// Field filters, all of which must hold.
    pub filters: Option<Vec<FilterWrite>>,
    /// Words that must each appear in the title, a text or web address
    /// value, or a chosen option's name; or a record's number.
    pub search: Option<String>,
    /// Up to 3 sort keys: a field's name, or title, number or updated.
    /// Records come by number when left out.
    pub sort: Option<Vec<SortWrite>>,
    /// A choice, status, person, checkbox or date field to group by: the
    /// answer counts the records in each group.
    pub group_by: Option<String>,
    /// Grouped by a date: "day", "week" (from Monday) or "month" (the
    /// default).
    pub group_bucket: Option<crate::fields::DateBucket>,
    /// What to add up beside each group's count, and over every record:
    /// "sum", "average", "min" or "max" of measureField. Left out, a
    /// chart view's own measure.
    pub measure: Option<crate::views::definition::MeasureOp>,
    /// The number field measure reads, by name or ID.
    pub measure_field: Option<String>,
    /// The fields to return, by name. Left out, every field with a value;
    /// [] returns numbers and titles only.
    pub fields: Option<Vec<String>>,
    /// "exclude" (the default), "include" or "only".
    pub archived: Option<String>,
    /// 1 to 200 records a page; 50 when left out.
    pub limit: Option<i64>,
    /// nextCursor from the last page, to read on with the same query.
    pub cursor: Option<String>,
    /// With records: each one's history too (up to 10 records).
    pub include_history: Option<bool>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct GroupLine {
    pub name: String,
    pub count: i64,
    /// With a measure: its value over the group's records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordHistory {
    pub number: i64,
    pub history: Vec<HistoryEntry>,
}

#[derive(Clone, Debug, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordQueryValue {
    pub collection: String,
    /// What the title is called in this collection.
    pub title_name: String,
    pub records: Vec<RecordLine>,
    /// Records the query finds, across every page.
    pub total: i64,
    /// With a measure: its value over every record the query finds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
    /// With groupBy: each group with records, in order, and how many.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<GroupLine>,
    /// Pass it as cursor, with the same query, for the next page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub history: Vec<RecordHistory>,
}

pub(crate) async fn record_query(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RecordQueryArguments,
) -> ToolResult<RecordQueryValue> {
    if !principal.can_read() {
        return ToolResult::denied("This connection can't read.");
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    let Some(archived) = Archived::parse(arguments.archived.as_deref().unwrap_or("")) else {
        return ToolResult::invalid("archived", "Use exclude, include or only.");
    };
    let Some(words) = query::search_words(arguments.search.as_deref().unwrap_or("")) else {
        return ToolResult::invalid(
            "search",
            format!(
                "Search for at most {} words in {} characters.",
                query::MAX_SEARCH_WORDS,
                query::MAX_SEARCH_CHARACTERS
            ),
        );
    };
    let limit = arguments.limit.unwrap_or(query::DEFAULT_LIMIT);
    if !(1..=query::MAX_LIMIT).contains(&limit) {
        return ToolResult::invalid(
            "limit",
            format!("Ask for 1 to {} records a page.", query::MAX_LIMIT),
        );
    }
    let named = match &arguments.records {
        None => None,
        Some(written) if written.is_empty() || written.len() > query::MAX_NAMED_RECORDS => {
            return ToolResult::invalid(
                "records",
                format!(
                    "Name 1 to {} records, by number or ID.",
                    query::MAX_NAMED_RECORDS
                ),
            );
        }
        Some(written) => {
            let mut references = Vec::with_capacity(written.len());
            for value in written {
                let Some(reference) = RecordRef::parse(value) else {
                    return ToolResult::invalid(
                        "records",
                        format!(
                            "Name records by number (#12) or ID; {} isn't one.",
                            echo(value)
                        ),
                    );
                };
                references.push(reference);
            }
            Some(references)
        }
    };
    let history_wanted = arguments.include_history.unwrap_or(false);
    if history_wanted && named.as_ref().is_none_or(|named| named.len() > 10) {
        return ToolResult::invalid(
            "includeHistory",
            "History comes with records: name up to 10 of them.",
        );
    }
    let project_id = principal.project_id;
    let result = async {
        let project_key = match read_access(state, principal).await? {
            Reading::Ok { project_key } => project_key,
            Reading::Refused(what) => return Ok(refused(what)),
        };
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("record_query", "Collections"));
        }
        let mut connection = crate::db::scoped_tx(&state.pool).await?;
        let collection =
            match find_collection(&mut connection, project_id, &arguments.collection).await? {
                Ok(collection) => collection,
                Err(refusal) => return Ok(refusal),
            };
        if collection.acts_like_task && !tasks_readable(state, principal).await? {
            return Ok(ToolResult::not_found("That collection"));
        }
        let definitions =
            collections::field_definitions(&mut connection, project_id, collection.id, true)
                .await?;
        // What a view reads by: the fields, and a task's other built-ins.
        let view_fields =
            collections::view_fields(&mut connection, project_id, collection.id, true).await?;
        let writer = Writer {
            actor_id: principal.actor_id,
            project_key: &project_key,
        };
        // The view to start from, then what this call says instead.
        let mut config = match arguments.view.as_deref() {
            Some(reference) => {
                let saved = views::visible(
                    &mut connection,
                    project_id,
                    collection.id,
                    principal.actor_id,
                )
                .await?;
                match views::find(&saved, reference) {
                    Some(view) => view.config.active(&view_fields),
                    None => {
                        return Ok(ToolResult::NotFound {
                            message: format!(
                                "{} has no view called \"{}\". Its views: {}.",
                                collection.name,
                                echo(reference),
                                views::names(&saved)
                            ),
                        });
                    }
                }
            }
            None => ViewConfig::default(),
        };
        if let Some(filters) = &arguments.filters {
            match definition::clauses_from_written(
                &mut connection,
                project_id,
                writer,
                &collection,
                &definitions,
                filters,
            )
            .await?
            {
                Ok(clauses) => config.filters = clauses.into(),
                Err(message) => return Ok(ToolResult::invalid("filters", message)),
            }
        }
        if let Some(sorts) = &arguments.sort {
            match definition::sorts_from_written(&collection, &definitions, sorts) {
                Ok(rules) => config.sort = rules,
                Err(message) => return Ok(ToolResult::invalid("sort", message)),
            }
        }
        if let Some(group_by) = arguments.group_by.as_deref() {
            match definition::group_from_written(&collection, &definitions, group_by) {
                Ok(group) => config.group_by = group,
                Err(message) => return Ok(ToolResult::invalid("groupBy", message)),
            }
        }
        if arguments.group_bucket.is_some() {
            config.group_bucket = arguments.group_bucket;
        }
        let measured = arguments.measure.is_some() || arguments.measure_field.is_some();
        if measured {
            let field = match arguments.measure_field.as_deref() {
                None => None,
                Some(name) => match fields::find(&definitions, name) {
                    Some(field) if !field.archived => Some(field.id),
                    _ => {
                        return Ok(ToolResult::invalid(
                            "measureField",
                            format!("{} has no field \"{name}\".", collection.name),
                        ));
                    }
                },
            };
            let mut chart = config.chart.unwrap_or_default();
            chart.measure = crate::views::definition::Measure {
                op: arguments.measure.unwrap_or(if field.is_some() {
                    crate::views::definition::MeasureOp::Sum
                } else {
                    crate::views::definition::MeasureOp::Count
                }),
                field,
            };
            config.chart = Some(chart);
        }
        if let Err(message) = views::check(&config, &view_fields) {
            let at = if measured { "measure" } else { "filters" };
            return Ok(ToolResult::invalid(at, message));
        }
        let only_fields = match &arguments.fields {
            None => None,
            Some(names) => {
                let mut ids = HashSet::new();
                for name in names {
                    match fields::find(&definitions, name) {
                        Some(field) if !field.archived => {
                            ids.insert(field.id);
                        }
                        _ => {
                            return Ok(ToolResult::invalid(
                                "fields",
                                format!(
                                    "{} has no field called \"{}\". Its fields: {}.",
                                    collection.name,
                                    echo(name),
                                    fields::active_names(&definitions)
                                ),
                            ));
                        }
                    }
                }
                Some(ids)
            }
        };
        let mut spec = Spec::from_config(&config, &view_fields, principal.actor_id)
            .searching(words.clone(), &view_fields);
        spec.archived = archived;
        let source = crate::collections::source(&collection);
        if let Some(named) = &named {
            let ids = records::ids_of(
                &mut *connection,
                project_id,
                collection.id,
                named,
                collection.acts_like_task,
            )
            .await?;
            let mut found = Vec::with_capacity(ids.len());
            for (reference, id) in named.iter().zip(ids) {
                match id {
                    Some(id) => found.push(id),
                    None => {
                        return Ok(ToolResult::NotFound {
                            message: format!(
                                "{} has no record {}.",
                                collection.name,
                                reference.shown()
                            ),
                        });
                    }
                }
            }
            spec.only = Some(found);
            // Named records show whether archived or not, unless asked.
            if arguments.archived.is_none() {
                spec.archived = Archived::Include;
            }
        }
        let Ok(page) = query::page(
            &mut connection,
            source.as_ref(),
            &spec,
            arguments.cursor.as_deref(),
            limit,
        )
        .await?
        else {
            return Ok(ToolResult::invalid("cursor", query::CURSOR_MESSAGE));
        };
        let total = query::total(&mut connection, source.as_ref(), &spec).await?;
        let groups = query::group_counts(&mut connection, source.as_ref(), &spec).await?;
        let value = query::measure_total(&mut connection, source.as_ref(), &spec).await?;
        let ids = page.rows.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let found = records::by_ids(
            &mut *connection,
            project_id,
            collection.id,
            &ids,
            collection.acts_like_task,
        )
        .await?;
        let references = work_item_fields::references(
            &mut connection,
            project_id,
            &definitions,
            found
                .iter()
                .map(|record| record.shown_values(&definitions))
                .collect::<Vec<_>>()
                .iter(),
        )
        .await?;
        // A record grouped by several choices is a line in each of its
        // groups: one per (record, group) the page found, in its order.
        let by_id = found
            .iter()
            .map(|record| (record.id, record))
            .collect::<HashMap<Uuid, _>>();
        let mut history = Vec::new();
        if history_wanted {
            for record in &found {
                history.push(RecordHistory {
                    number: record.number,
                    history: records::history(
                        &mut *connection,
                        project_id,
                        record.id,
                        records::HISTORY_LIMIT,
                    )
                    .await?,
                });
            }
        }
        Ok::<_, AppError>(ToolResult::ok(RecordQueryValue {
            collection: collection.name.clone(),
            title_name: collection.title_name.clone(),
            records: page
                .rows
                .into_iter()
                .filter_map(|(id, group)| {
                    let record = by_id.get(&id)?;
                    let mut line =
                        RecordLine::of(record, &definitions, &references, only_fields.as_ref());
                    line.group = group.map(|group| group.label);
                    Some(line)
                })
                .collect(),
            total,
            value,
            groups: groups
                .into_iter()
                .map(|group| GroupLine {
                    name: group.label,
                    count: group.count,
                    value: group.value,
                })
                .collect(),
            next_cursor: page.next_cursor,
            history,
        }))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "record_query"))
}

// ---------------------------------------------------------------------------
// record_write

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordWriteEntry {
    /// The record to change: its number (#12) or ID. Leave it out to add
    /// one.
    #[serde(default, deserialize_with = "string_or_number")]
    #[schemars(with = "Option<String>")]
    pub record: Option<String>,
    /// Its title: the collection's title column (see titleName).
    pub title: Option<String>,
    /// Values by field name, such as {"Severity": "High", "Tester": "@dana",
    /// "Build": "AWD-12", "Seen on": "2026-11-11"}. null clears one.
    pub fields: Option<Map<String, Value>>,
    /// true archives it (hidden, nothing deleted), false restores it.
    pub archived: Option<bool>,
    /// The version from record_list. Leave it out to change the current
    /// one.
    pub expected_version: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordWriteArguments {
    /// The project's ID, link or key (such as "AWD").
    pub project_id: String,
    /// The collection, by name or ID.
    pub collection: String,
    /// 1 to 100 records to add or change, all or nothing: if one is
    /// refused, none is written.
    pub records: Vec<RecordWriteEntry>,
    /// A stable ID so a retry never adds the records twice.
    pub client_request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RecordWriteValue {
    pub collection: String,
    /// Each record as it is now, in the order written.
    pub records: Vec<RecordLine>,
    pub created: usize,
    pub changed: usize,
    /// Records that already looked like that.
    pub unchanged: usize,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub idempotent_replay: bool,
}

/// Writes records of a collection whose records are tasks, each as a task
/// change (`collections::task_writes`, as the web app's record routes do),
/// all or nothing in the caller's transaction.
async fn task_records_write(
    transaction: &mut Transaction<'_, Postgres>,
    principal: &McpPrincipal,
    project_key: &str,
    collection: &Collection,
    inputs: &[RecordInput],
) -> Result<Result<Vec<Written>, records::WriteProblem>, AppError> {
    let project_id = principal.project_id;
    let actor = super::tasks::command_actor(principal, project_id);
    // Agents name built-in fields, options, people and labels as they read
    // them; the task change takes them by ID.
    let builtins = collections::tasks::fields(
        &mut *transaction,
        project_id,
        collection.field_kind(),
        false,
    )
    .await?
    .into_iter()
    .filter(|field| collections::tasks::is_builtin_field(field.id))
    .collect::<Vec<_>>();
    let mut written = Vec::with_capacity(inputs.len());
    for (index, input) in inputs.iter().enumerate() {
        let (named, rest): (Vec<_>, Vec<_>) = input
            .values
            .iter()
            .cloned()
            .partition(|(reference, _)| fields::find(&builtins, reference).is_some());
        let resolved = match work_item_fields::resolve_for(
            &mut *transaction,
            project_id,
            Writer {
                actor_id: principal.actor_id,
                project_key,
            },
            &builtins,
            "fields",
            &named,
            None,
        )
        .await?
        {
            Ok(resolved) => resolved,
            Err(problem) => {
                return Ok(Err(records::WriteProblem {
                    index,
                    kind: ProblemKind::Invalid,
                    field: problem.field,
                    message: problem.message,
                    current_version: None,
                }));
            }
        };
        let input = &RecordInput {
            values: resolved
                .entries
                .into_iter()
                .map(|entry| {
                    (
                        entry.field.id.to_string(),
                        entry.value.unwrap_or(Value::Null),
                    )
                })
                .chain(rest)
                .collect(),
            ..input.clone()
        };
        let before = match input.record {
            Some(reference) => records::find(
                &mut **transaction,
                project_id,
                collection.id,
                reference,
                true,
            )
            .await?
            .map(|record| record.version),
            None => None,
        };
        let task_id = match collections::task_writes::write(
            transaction,
            &actor,
            project_key,
            collection,
            input,
        )
        .await?
        {
            Ok(task_id) => task_id,
            Err(mut problem) => {
                problem.index = index;
                return Ok(Err(problem));
            }
        };
        let record = records::find(
            &mut **transaction,
            project_id,
            collection.id,
            RecordRef::Id(task_id),
            true,
        )
        .await?
        .ok_or(AppError::Internal("written task is missing"))?;
        written.push(match before {
            None => Written::Created(record),
            Some(version) if version == record.version => Written::Unchanged(record),
            Some(_) => Written::Changed {
                record,
                changed: Vec::new(),
            },
        });
    }
    Ok(Ok(written))
}

pub(crate) async fn record_write(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: RecordWriteArguments,
) -> ToolResult<RecordWriteValue> {
    if !principal.can_write() {
        return ToolResult::read_only();
    }
    if parse_id(&arguments.project_id) != Some(principal.project_id) {
        return ToolResult::not_found("That project");
    }
    if arguments.records.is_empty() || arguments.records.len() > records::MAX_RECORDS_PER_WRITE {
        return ToolResult::invalid(
            "records",
            format!(
                "Write 1 to {} records at a time.",
                records::MAX_RECORDS_PER_WRITE
            ),
        );
    }
    let mut inputs = Vec::with_capacity(arguments.records.len());
    for (index, entry) in arguments.records.iter().enumerate() {
        let record = match entry.record.as_deref() {
            Some(value) => match RecordRef::parse(value) {
                Some(reference) => Some(reference),
                None => {
                    return ToolResult::invalid(
                        &format!("records[{index}].record"),
                        format!(
                            "Name a record by number (#12) or ID; {} isn't one.",
                            echo(value)
                        ),
                    );
                }
            },
            None => None,
        };
        inputs.push(RecordInput {
            record,
            title: entry.title.clone(),
            values: entry
                .fields
                .iter()
                .flatten()
                .map(|(field, value)| (field.clone(), value.clone()))
                .collect(),
            archived: entry.archived,
            expected_version: entry.expected_version,
        });
    }
    let Some(key) = request_key(arguments.client_request_id.as_deref()) else {
        return ToolResult::invalid("clientRequestId", REQUEST_ID_MESSAGE);
    };
    let project_id = principal.project_id;
    let result = async {
        if module_off(state, principal).await? {
            return Ok(ToolResult::module_off("record_write", "Collections"));
        }
        // Fields only builders change (T1.19).
        let may_build = can_write_in(state, principal, project_id, Action::CollectionBuild).await?;
        let Writing {
            mut transaction,
            project_key,
        } = match begin_write(state, principal, Action::CollectionRecordEdit, EDIT_MESSAGE).await? {
            Ok(writing) => writing,
            Err(refusal) => return Ok(refusal),
        };
        let found =
            match find_collection(&mut transaction, project_id, &arguments.collection).await? {
                Ok(collection) => collection,
                Err(refusal) => return Ok(refusal),
            };
        let scope = resource_bound_scope("mcp_collections", found.id, "record_write")?;
        let request_hash = RequestHash::for_value(&arguments)?;
        let reservation = match reserve(
            &mut transaction,
            principal.actor_id,
            &scope,
            &key,
            request_hash,
        )
        .await
        {
            Ok(IdempotencyOutcome::Replay(replay)) => {
                let mut value = replay.decode::<RecordWriteValue>()?;
                transaction.commit().await?;
                value.idempotent_replay = true;
                return Ok(ToolResult::ok(value));
            }
            Ok(IdempotencyOutcome::Reserved(reservation)) => reservation,
            Err(AppError::Conflict("idempotency_key", "payload mismatch")) => {
                return Ok(ToolResult::invalid(
                    "clientRequestId",
                    "That clientRequestId was already used for a different change. Use a new one.",
                ));
            }
            Err(AppError::Conflict("idempotency_key", "command in progress")) => {
                return Ok(ToolResult::conflict(
                    "The same change is still being applied. Try again in a moment.",
                ));
            }
            Err(error) => return Err(error),
        };
        let Some(collection) = collections::lock(&mut transaction, project_id, found.id).await?
        else {
            return Ok(ToolResult::not_found("That collection"));
        };
        if collection.archived {
            return Ok(ToolResult::conflict(format!(
                "\"{}\" is archived. Restore it with collection_update (archived: false) first.",
                collection.name
            )));
        }
        let definitions =
            collections::field_definitions(&mut transaction, project_id, collection.id, true)
                .await?;
        let writer = Writer {
            actor_id: principal.actor_id,
            project_key: &project_key,
        };
        let written = if collection.acts_like_task {
            // Its records are tasks: each is a task change, as on the web.
            task_records_write(
                &mut transaction,
                principal,
                &project_key,
                &collection,
                &inputs,
            )
            .await?
        } else {
            records::write(
                &mut transaction,
                project_id,
                &collection,
                writer,
                may_build,
                &definitions,
                "fields",
                &inputs,
            )
            .await?
        };
        let written = match written {
            Ok(written) => written,
            Err(problem) => {
                let at = format!("records[{}].{}", problem.index, problem.field);
                let message = format!("Nothing was written. {}", problem.message);
                return Ok(match problem.kind {
                    ProblemKind::Invalid => ToolResult::invalid(&at, message),
                    ProblemKind::NotFound => ToolResult::NotFound { message },
                    ProblemKind::Conflict | ProblemKind::Full => ToolResult::conflict(message),
                });
            }
        };
        let changed_ids = written
            .iter()
            .filter(|written| !matches!(written, Written::Unchanged(_)))
            .map(|written| written.record().id)
            .collect::<Vec<_>>();
        if !changed_ids.is_empty() {
            outbox::enqueue_collection_updated(
                &mut transaction,
                project_id,
                collection.id,
                &changed_ids,
            )
            .await?;
        }
        let references = work_item_fields::references(
            &mut transaction,
            project_id,
            &definitions,
            written
                .iter()
                .map(|written| written.record().shown_values(&definitions))
                .collect::<Vec<_>>()
                .iter(),
        )
        .await?;
        let value = RecordWriteValue {
            collection: collection.name.clone(),
            records: written
                .iter()
                .map(|written| RecordLine::of(written.record(), &definitions, &references, None))
                .collect(),
            created: written
                .iter()
                .filter(|written| matches!(written, Written::Created(_)))
                .count(),
            changed: written
                .iter()
                .filter(|written| matches!(written, Written::Changed { .. }))
                .count(),
            unchanged: written
                .iter()
                .filter(|written| matches!(written, Written::Unchanged(_)))
                .count(),
            idempotent_replay: false,
        };
        crate::idempotency::complete(
            &mut transaction,
            &reservation,
            axum::http::StatusCode::OK,
            &value,
        )
        .await?;
        transaction.commit().await?;
        Ok::<_, AppError>(ToolResult::ok(value))
    }
    .await;
    result.unwrap_or_else(|error| ToolResult::from_error(error, "record_write"))
}
