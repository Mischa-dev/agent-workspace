//! The web app's routes for collections (AWD-90). Everyone in a project
//! reads its collections; `collections.edit` adds and changes records, and
//! `collections.build` creates collections and changes their fields and
//! views (AWD-74 roles). Agents use the tools in `mcp::collection_tools`,
//! over the same code.
//!
//! Every write takes the project lock first, then the collection's row, and
//! is idempotent by its `Idempotency-Key`. Open screens hear
//! `collection.updated` with the IDs of the records that changed, and read
//! what they show again over these routes.

use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, patch},
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::{
    ChangeOutcome, Collection, CollectionChange, CollectionSummary, CreateOutcome, NewCollection,
    definition::{self, FieldWrite, ViewWrite},
    query::{self, Archived, GroupCount, GroupRef, Spec},
    records::{self, ProblemKind, RecordInput, RecordRef, RecordView, Written},
    views::{
        self, ChangeViewOutcome, CreateViewOutcome, NewView, RemoveViewOutcome, SortRule,
        ViewChange, ViewConfig, ViewSummary, ViewType,
    },
};
use crate::{
    AppState,
    auth::{AuthenticatedActor, require_authenticated_write},
    conversations::parse_actor_kind,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    fields::{
        self, ChangeFieldOutcome, CreateFieldOutcome, FieldChange, FieldDefinition, FieldType,
        FieldView, NewField, OptionInput,
    },
    idempotency::{
        IdempotencyKey, IdempotencyOutcome, IdempotencyReservation, RequestHash, complete, reserve,
        resource_bound_scope,
    },
    outbox,
    policy::Action,
    views::filter::Filter,
    work_item_commands::{
        AuditAttribution, CommandActor, ProjectMembership, authorize_mutation, lock_membership,
        read_membership, record_audit_for,
    },
    work_item_fields::{ValueReference, Writer},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/v1/projects/{project_id}/collections",
            get(list_collections).post(create_collection),
        )
        .route(
            "/api/v1/projects/{project_id}/collections/{collection_id}",
            get(get_collection).patch(update_collection),
        )
        .route(
            "/api/v1/projects/{project_id}/collections/{collection_id}/fields",
            axum::routing::post(create_field),
        )
        .route(
            "/api/v1/projects/{project_id}/collections/{collection_id}/fields/{field_id}",
            patch(update_field),
        )
        .route(
            "/api/v1/projects/{project_id}/collections/{collection_id}/views",
            axum::routing::post(create_view),
        )
        .route(
            "/api/v1/projects/{project_id}/collections/{collection_id}/views/{view_id}",
            patch(update_view).delete(delete_view),
        )
        .route(
            "/api/v1/projects/{project_id}/collections/{collection_id}/records",
            get(list_records).post(create_record),
        )
        .route(
            "/api/v1/projects/{project_id}/collections/{collection_id}/records/{record_id}",
            get(get_record).patch(update_record),
        )
}

// ---------------------------------------------------------------------------
// Access

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
        actor_kind: parse_actor_kind(&principal.actor_kind)?,
        project_id,
        audit: AuditAttribution::Browser {
            session_id: principal.session_id,
            request_id,
        },
    })
}

fn allows(
    actor: &CommandActor,
    membership: &ProjectMembership,
    action: Action,
) -> Result<bool, AppError> {
    Ok(membership.enabled && authorize_mutation(actor, membership, action, None)?.is_allowed())
}

/// The project's key, when it can hold collections: the Messages space
/// can't.
async fn project_key(
    connection: &mut PgConnection,
    project_id: Uuid,
) -> Result<Option<String>, AppError> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT kind, project_key FROM projects WHERE id = $1",
    )
    .bind(project_id)
    .fetch_optional(&mut *connection)
    .await?
    .filter(|(kind, _)| kind != "direct")
    .map(|(_, key)| key))
}

/// What a reader may do in a project.
struct Reader {
    can_edit: bool,
    can_build: bool,
    project_archived: bool,
}

async fn reader(
    connection: &mut PgConnection,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
) -> Result<Reader, AppError> {
    let membership = read_membership(connection, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    let actor = browser_actor(headers, principal, project_id)?;
    if !allows(&actor, &membership, Action::ProjectView)? {
        return Err(AppError::NotFound("project"));
    }
    if project_key(connection, project_id).await?.is_none() {
        return Err(AppError::NotFound("project"));
    }
    Ok(Reader {
        can_edit: allows(&actor, &membership, Action::CollectionRecordEdit)?,
        can_build: allows(&actor, &membership, Action::CollectionBuild)?,
        project_archived: membership.project_archived_at.is_some(),
    })
}

/// A write that holds the project lock and an idempotency reservation.
struct Write {
    transaction: Transaction<'static, Postgres>,
    reservation: IdempotencyReservation,
    actor: CommandActor,
    project_key: String,
}

impl Write {
    async fn finish<T: Serialize>(
        mut self,
        status: StatusCode,
        body: &T,
    ) -> Result<Response, AppError> {
        complete(&mut self.transaction, &self.reservation, status, body).await?;
        self.transaction.commit().await?;
        Ok((status, Json(body)).into_response())
    }

    /// The collection's row, locked after the project lock.
    async fn collection(&mut self, collection_id: Uuid) -> Result<Collection, AppError> {
        super::lock(&mut self.transaction, self.actor.project_id, collection_id)
            .await?
            .ok_or(AppError::NotFound("collection"))
    }
}

enum Started {
    Replay(Response),
    Write(Box<Write>),
}

/// Checks the session, locks the project, authorizes `action` (someone who
/// can't see the project gets not found, someone who can but may not do
/// this gets forbidden) and reserves the idempotency key. A retried request
/// gets the first response back.
async fn start_write<T: Serialize>(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    project_id: Uuid,
    action: Action,
    scope: String,
    request: &T,
) -> Result<Started, AppError> {
    require_authenticated_write(state, headers, principal)?;
    let key = IdempotencyKey::from_headers(headers)?;
    let request_hash = RequestHash::for_value(request)?;
    let mut transaction = state.pool.begin().await?;
    let membership = lock_membership(&mut transaction, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    let actor = browser_actor(headers, principal, project_id)?;
    if !allows(&actor, &membership, Action::ProjectView)? {
        return Err(AppError::NotFound("project"));
    }
    let Some(project_key) = project_key(&mut transaction, project_id).await? else {
        return Err(AppError::NotFound("project"));
    };
    if !allows(&actor, &membership, action)? {
        return Err(AppError::Forbidden);
    }
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
            let body: Value = replay.decode()?;
            transaction.commit().await?;
            Ok(Started::Replay(
                (replay.status(), Json(body)).into_response(),
            ))
        }
        IdempotencyOutcome::Reserved(reservation) => Ok(Started::Write(Box::new(Write {
            transaction,
            reservation,
            actor,
            project_key,
        }))),
    }
}

macro_rules! begin {
    ($started:expr) => {
        match $started? {
            Started::Replay(response) => return Ok(response),
            Started::Write(write) => *write,
        }
    };
}

/// An error with a message written for the person, and optional details.
fn refusal(status: StatusCode, code: &str, message: impl Into<String>, details: Value) -> Response {
    (
        status,
        Json(json!({ "error": { "code": code, "message": message.into(), "details": details } })),
    )
        .into_response()
}

fn invalid(field: &str, message: impl Into<String>) -> Response {
    refusal(
        StatusCode::UNPROCESSABLE_ENTITY,
        "validation_failed",
        message,
        json!({ "field": field }),
    )
}

fn archived_collection(collection: &Collection) -> Response {
    refusal(
        StatusCode::CONFLICT,
        "collection_archived",
        format!("{} is archived. Restore it to change it.", collection.name),
        Value::Null,
    )
}

/// The built-in Tasks collection's records, fields and settings change on
/// task screens; only its views change here.
fn task_fields_forbidden() -> Response {
    refusal(
        StatusCode::FORBIDDEN,
        "forbidden",
        "Only people whose role can change the workflow (owners and admins, unless a custom role says otherwise) can change the task fields.",
        Value::Null,
    )
}

fn builtin_collection() -> Response {
    refusal(
        StatusCode::CONFLICT,
        "builtin_collection",
        super::tasks::READ_ONLY_MESSAGE,
        Value::Null,
    )
}

// ---------------------------------------------------------------------------
// Collections

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IncludeArchivedQuery {
    include_archived: Option<bool>,
}

async fn list_collections(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<IncludeArchivedQuery>,
) -> Result<Json<Value>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let reader = reader(&mut connection, &headers, &principal, project_id).await?;
    if !reader.project_archived {
        super::tasks::ensure(&mut connection, project_id, principal.actor_id).await?;
    }
    let collections = super::list(
        &mut *connection,
        project_id,
        query.include_archived.unwrap_or(false),
    )
    .await?;
    Ok(Json(json!({
        "collections": collections,
        "canBuild": reader.can_build && !reader.project_archived,
    })))
}

/// A collection with its fields (archived ones too, marked) and views, as
/// its screen reads it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CollectionBody {
    collection: CollectionSummary,
    fields: Vec<FieldView>,
    views: Vec<ViewSummary>,
    archived_records: i64,
    can_edit_records: bool,
    can_build: bool,
}

async fn collection_body(
    connection: &mut PgConnection,
    project_id: Uuid,
    collection: &Collection,
    can_edit: bool,
    can_build: bool,
) -> Result<CollectionBody, AppError> {
    let definitions =
        super::field_definitions(&mut *connection, project_id, collection.id, true).await?;
    let views = views::list(&mut *connection, project_id, collection.id).await?;
    // Each query binds exactly the parameters it names: an extra bind fails
    // on a reused prepared statement.
    let (records, archived_records) = sqlx::query_as::<_, (i64, i64)>(
        "SELECT count(*) FILTER (WHERE archived_at IS NULL), count(*) FILTER (WHERE archived_at IS NOT NULL) \
         FROM collection_records WHERE collection_id = $1",
    )
    .bind(collection.id)
    .fetch_one(&mut *connection)
    .await?;
    Ok(CollectionBody {
        collection: CollectionSummary::of(collection, records),
        fields: definitions.iter().map(FieldView::from).collect(),
        views: views
            .iter()
            .map(|view| ViewSummary::of(view, &definitions))
            .collect(),
        archived_records,
        // The Tasks collection's records are tasks, changed on task screens.
        can_edit_records: can_edit && !collection.archived,
        can_build,
    })
}

async fn get_collection(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id)): ApiPath<(Uuid, Uuid)>,
) -> Result<Json<CollectionBody>, AppError> {
    let mut connection = state.pool.acquire().await?;
    let reader = reader(&mut connection, &headers, &principal, project_id).await?;
    let collection = super::get(&mut *connection, project_id, collection_id)
        .await?
        .ok_or(AppError::NotFound("collection"))?;
    let writable = !reader.project_archived;
    Ok(Json(
        collection_body(
            &mut connection,
            project_id,
            &collection,
            reader.can_edit && writable,
            reader.can_build && writable,
        )
        .await?,
    ))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateCollectionRequest {
    name: String,
    #[serde(default)]
    description: String,
    title_name: Option<String>,
    /// Fields and views to start with, by name, as a definition holds them.
    #[serde(default)]
    fields: Vec<FieldWrite>,
    #[serde(default)]
    views: Vec<ViewWrite>,
}

fn name_message() -> String {
    format!(
        "Give it a name of 1 to {} characters, on one line.",
        super::MAX_NAME_CHARACTERS
    )
}

fn definition_problem(problem: definition::ApplyProblem) -> Response {
    match problem.kind {
        definition::ProblemKind::Invalid => invalid(&problem.field, problem.message),
        definition::ProblemKind::NotFound => refusal(
            StatusCode::NOT_FOUND,
            "not_found",
            problem.message,
            json!({ "field": problem.field }),
        ),
        definition::ProblemKind::Full => refusal(
            StatusCode::CONFLICT,
            "collection_limit",
            problem.message,
            json!({ "field": problem.field }),
        ),
    }
}

async fn create_collection(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateCollectionRequest>,
) -> Result<Response, AppError> {
    let Some(name) = super::normalize_name(&request.name) else {
        return Ok(invalid("name", name_message()));
    };
    let Some(description) = super::normalize_description(&request.description) else {
        return Ok(invalid(
            "description",
            format!(
                "The description is one line of at most {} characters.",
                super::MAX_DESCRIPTION_CHARACTERS
            ),
        ));
    };
    let title_name = match request.title_name.as_deref() {
        Some(value) => match super::normalize_title_name(value) {
            Some(title_name) => title_name,
            None => {
                return Ok(invalid(
                    "titleName",
                    format!(
                        "Call the title column 1 to {} characters, on one line.",
                        super::MAX_TITLE_NAME_CHARACTERS
                    ),
                ));
            }
        },
        None => super::DEFAULT_TITLE_NAME.to_owned(),
    };
    if let Some(message) = definition::duplicate_name(&request.fields, &request.views) {
        return Ok(invalid("fields", message));
    }
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionBuild,
            resource_bound_scope("projects", project_id, "collections_create")?,
            &request,
        )
        .await
    );
    let collection = match super::create(
        &mut write.transaction,
        project_id,
        principal.actor_id,
        NewCollection {
            name,
            description,
            title_name,
            first_view: request.views.is_empty(),
        },
    )
    .await?
    {
        CreateOutcome::Created(collection) => collection,
        CreateOutcome::NameTaken(collection) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "collection_name_taken",
                format!("A collection is already called \"{}\".", collection.name),
                json!({ "field": "name" }),
            ));
        }
        CreateOutcome::ProjectFull => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "collection_limit",
                format!(
                    "This project has {} collections already, or keeps {} with archived ones. Archive one first.",
                    super::MAX_ACTIVE_COLLECTIONS,
                    super::MAX_COLLECTION_ROWS
                ),
                Value::Null,
            ));
        }
    };
    let actor_id = write.actor.actor_id;
    let project_key = write.project_key.clone();
    if !request.fields.is_empty() || !request.views.is_empty() {
        let writer = Writer {
            actor_id,
            project_key: &project_key,
        };
        if let Err(problem) = definition::apply(
            &mut write.transaction,
            project_id,
            &collection,
            writer,
            &request.fields,
            &request.views,
        )
        .await?
        {
            return Ok(definition_problem(problem));
        }
    }
    record_audit_for(
        &mut write.transaction,
        &write.actor,
        "collection",
        "collection.created",
        collection.id,
        json!({
            "name": collection.name,
            "fields": request.fields.len(),
            "views": request.views.len(),
        }),
    )
    .await?;
    outbox::enqueue_collection_updated(&mut write.transaction, project_id, collection.id, &[])
        .await?;
    let collection = write.collection(collection.id).await?;
    let body = collection_body(&mut write.transaction, project_id, &collection, true, true).await?;
    write.finish(StatusCode::CREATED, &body).await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateCollectionRequest {
    expected_version: i64,
    name: Option<String>,
    description: Option<String>,
    title_name: Option<String>,
    position: Option<i32>,
    archived: Option<bool>,
    acts_like_task: Option<bool>,
}

async fn update_collection(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateCollectionRequest>,
) -> Result<Response, AppError> {
    let name = match request.name.as_deref() {
        Some(value) => match super::normalize_name(value) {
            Some(name) => Some(name),
            None => return Ok(invalid("name", name_message())),
        },
        None => None,
    };
    let description = match request.description.as_deref() {
        Some(value) => match super::normalize_description(value) {
            Some(description) => Some(description),
            None => {
                return Ok(invalid(
                    "description",
                    format!(
                        "The description is one line of at most {} characters.",
                        super::MAX_DESCRIPTION_CHARACTERS
                    ),
                ));
            }
        },
        None => None,
    };
    let title_name = match request.title_name.as_deref() {
        Some(value) => match super::normalize_title_name(value) {
            Some(title_name) => Some(title_name),
            None => {
                return Ok(invalid(
                    "titleName",
                    format!(
                        "Call the title column 1 to {} characters, on one line.",
                        super::MAX_TITLE_NAME_CHARACTERS
                    ),
                ));
            }
        },
        None => None,
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionBuild,
            resource_bound_scope("collections", collection_id, "update")?,
            &request,
        )
        .await
    );
    if write.collection(collection_id).await?.is_tasks() {
        return Ok(builtin_collection());
    }
    // Switching on makes a task for each record.
    if request.acts_like_task == Some(true)
        && !super::task_writes::may_create_tasks(&mut write.transaction, &write.actor).await?
    {
        return Ok(refusal(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Records that act like tasks are tasks: only people who can make tasks here can switch it on.",
            Value::Null,
        ));
    }
    let outcome = super::change(
        &mut write.transaction,
        project_id,
        collection_id,
        principal.actor_id,
        CollectionChange {
            name,
            description,
            title_name,
            position: request.position,
            archived: request.archived,
            acts_like_task: request.acts_like_task,
            expected_version: Some(request.expected_version),
        },
    )
    .await?;
    let collection = match outcome {
        ChangeOutcome::Changed {
            collection,
            previous,
            changed,
        } => {
            let action = match (previous.archived, collection.archived) {
                (false, true) => "collection.archived",
                (true, false) => "collection.restored",
                _ => "collection.updated",
            };
            record_audit_for(
                &mut write.transaction,
                &write.actor,
                "collection",
                action,
                collection.id,
                json!({
                    "changedFields": changed,
                    "previous": { "name": previous.name },
                    "previousVersion": previous.version,
                }),
            )
            .await?;
            outbox::enqueue_collection_updated(
                &mut write.transaction,
                project_id,
                collection.id,
                &[],
            )
            .await?;
            *collection
        }
        ChangeOutcome::Unchanged(collection) => collection,
        ChangeOutcome::NotFound => return Err(AppError::NotFound("collection")),
        ChangeOutcome::VersionConflict(collection) => {
            return Err(AppError::VersionConflict {
                resource: "collection",
                current_version: collection.version,
            });
        }
        ChangeOutcome::NameTaken(other) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "collection_name_taken",
                format!("Another collection is already called \"{other}\"."),
                json!({ "field": "name" }),
            ));
        }
        ChangeOutcome::ProjectFull => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "collection_limit",
                format!(
                    "This project has {} active collections already. Archive one first.",
                    super::MAX_ACTIVE_COLLECTIONS
                ),
                Value::Null,
            ));
        }
        ChangeOutcome::TaskLike(message) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "acts_like_task",
                message,
                Value::Null,
            ));
        }
    };
    let body = collection_body(&mut write.transaction, project_id, &collection, true, true).await?;
    write.finish(StatusCode::OK, &body).await
}

// ---------------------------------------------------------------------------
// Fields

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateFieldRequest {
    name: String,
    #[serde(rename = "type")]
    field_type: String,
    #[serde(default)]
    description: String,
    options: Option<Vec<OptionInput>>,
    target: Option<String>,
    position: Option<i32>,
}

fn field_name_message() -> String {
    format!(
        "Give the field a name of 1 to {} characters, on one line.",
        fields::MAX_NAME_CHARACTERS
    )
}

fn field_description_message() -> String {
    format!(
        "The description is one line of at most {} characters.",
        fields::MAX_DESCRIPTION_CHARACTERS
    )
}

async fn create_field(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CreateFieldRequest>,
) -> Result<Response, AppError> {
    let Some(name) = fields::normalize_name(&request.name) else {
        return Ok(invalid("name", field_name_message()));
    };
    let Some(field_type) = FieldType::parse(&request.field_type) else {
        return Ok(invalid(
            "type",
            "Pick a type: text, number, date, single_choice, multiple_choice, person, url, checkbox or link.",
        ));
    };
    let Some(description) = fields::normalize_description(&request.description) else {
        return Ok(invalid("description", field_description_message()));
    };
    let config = match fields::normalize_config(
        field_type,
        None,
        request.options.as_deref(),
        request.target.as_deref(),
    ) {
        Ok(config) => config,
        Err(problem) => return Ok(invalid(problem.field, problem.message)),
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionBuild,
            resource_bound_scope("collections", collection_id, "fields_create")?,
            &request,
        )
        .await
    );
    let collection = write.collection(collection_id).await?;
    if collection.archived {
        return Ok(archived_collection(&collection));
    }
    // Tasks' fields are the project's task fields, changed by those who may
    // change the workflow; the built-in ones keep their names.
    if collection.acts_like_task && super::tasks::is_builtin_name(&name) {
        return Ok(invalid(
            "name",
            format!(
                "{} already has a built-in field called \"{name}\".",
                collection.name
            ),
        ));
    }
    if collection.is_tasks()
        && !super::task_writes::may_change_fields(&mut write.transaction, &write.actor).await?
    {
        return Ok(task_fields_forbidden());
    }
    let kind = collection.field_kind();
    let field = match fields::create(
        &mut write.transaction,
        project_id,
        kind,
        principal.actor_id,
        NewField {
            name,
            description,
            field_type,
            config,
            position: request.position,
        },
    )
    .await?
    {
        CreateFieldOutcome::Created(field) => field,
        CreateFieldOutcome::NameTaken(field) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "field_name_taken",
                format!("A field is already called \"{}\".", field.name),
                json!({ "field": "name" }),
            ));
        }
        CreateFieldOutcome::ProjectFull => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "field_limit",
                format!(
                    "This collection has {} fields already, or keeps {} with archived ones. Archive one first.",
                    fields::MAX_ACTIVE_FIELDS,
                    fields::MAX_FIELD_ROWS
                ),
                Value::Null,
            ));
        }
    };
    record_audit_for(
        &mut write.transaction,
        &write.actor,
        "field_definition",
        "field_definition.created",
        field.id,
        json!({
            "name": field.name,
            "type": field.field_type.as_str(),
            "recordKind": kind.as_str(),
            "collectionId": collection_id,
        }),
    )
    .await?;
    super::touch(&mut write.transaction, project_id, collection_id).await?;
    outbox::enqueue_collection_updated(&mut write.transaction, project_id, collection_id, &[])
        .await?;
    write
        .finish(StatusCode::CREATED, &FieldView::from(&field))
        .await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateFieldRequest {
    expected_version: i64,
    name: Option<String>,
    description: Option<String>,
    options: Option<Vec<OptionInput>>,
    position: Option<i32>,
    archived: Option<bool>,
}

async fn update_field(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id, field_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateFieldRequest>,
) -> Result<Response, AppError> {
    let name = match request.name.as_deref() {
        Some(value) => match fields::normalize_name(value) {
            Some(name) => Some(name),
            None => return Ok(invalid("name", field_name_message())),
        },
        None => None,
    };
    let description = match request.description.as_deref() {
        Some(value) => match fields::normalize_description(value) {
            Some(description) => Some(description),
            None => return Ok(invalid("description", field_description_message())),
        },
        None => None,
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionBuild,
            resource_bound_scope("field_definitions", field_id, "update")?,
            &request,
        )
        .await
    );
    let collection = write.collection(collection_id).await?;
    if collection.archived {
        return Ok(archived_collection(&collection));
    }
    if collection.acts_like_task {
        if super::tasks::is_builtin_field(field_id) {
            return Ok(builtin_collection());
        }
        if let Some(name) = &name
            && super::tasks::is_builtin_name(name)
        {
            return Ok(invalid(
                "name",
                format!(
                    "{} already has a built-in field called \"{name}\".",
                    collection.name
                ),
            ));
        }
    }
    if collection.is_tasks()
        && !super::task_writes::may_change_fields(&mut write.transaction, &write.actor).await?
    {
        return Ok(task_fields_forbidden());
    }
    let kind = collection.field_kind();
    let definitions =
        super::field_definitions(&mut write.transaction, project_id, collection_id, true).await?;
    let Some(current) = definitions.iter().find(|field| field.id == field_id) else {
        return Err(AppError::NotFound("field"));
    };
    let config = match request.options.as_deref() {
        Some(options) => match fields::normalize_config(
            current.field_type,
            Some(&current.config),
            Some(options),
            None,
        ) {
            Ok(config) => Some(config),
            Err(problem) => return Ok(invalid(problem.field, problem.message)),
        },
        None => None,
    };
    let field = match fields::change(
        &mut write.transaction,
        project_id,
        kind,
        field_id,
        principal.actor_id,
        FieldChange {
            name,
            description,
            config,
            position: request.position,
            archived: request.archived,
            expected_version: Some(request.expected_version),
        },
    )
    .await?
    {
        ChangeFieldOutcome::Changed {
            field,
            previous,
            changed,
        } => {
            let action = match (previous.archived, field.archived) {
                (false, true) => "field_definition.archived",
                (true, false) => "field_definition.restored",
                _ => "field_definition.updated",
            };
            record_audit_for(
                &mut write.transaction,
                &write.actor,
                "field_definition",
                action,
                field.id,
                json!({
                    "changedFields": changed,
                    "previous": { "name": previous.name },
                    "previousVersion": previous.version,
                    "collectionId": collection_id,
                }),
            )
            .await?;
            super::touch(&mut write.transaction, project_id, collection_id).await?;
            outbox::enqueue_collection_updated(
                &mut write.transaction,
                project_id,
                collection_id,
                &[],
            )
            .await?;
            field
        }
        ChangeFieldOutcome::Unchanged(field) => field,
        ChangeFieldOutcome::NotFound => return Err(AppError::NotFound("field")),
        ChangeFieldOutcome::VersionConflict(field) => {
            return Err(AppError::VersionConflict {
                resource: "field",
                current_version: field.version,
            });
        }
        ChangeFieldOutcome::NameTaken(other) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "field_name_taken",
                format!("Another field is already called \"{other}\"."),
                json!({ "field": "name" }),
            ));
        }
        ChangeFieldOutcome::ProjectFull => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "field_limit",
                format!(
                    "This collection has {} active fields already. Archive one first.",
                    fields::MAX_ACTIVE_FIELDS
                ),
                Value::Null,
            ));
        }
    };
    write.finish(StatusCode::OK, &FieldView::from(&field)).await
}

// ---------------------------------------------------------------------------
// Views

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateViewRequest {
    name: String,
    #[serde(rename = "type", default = "table")]
    view_type: String,
    #[serde(default)]
    config: ViewConfig,
    position: Option<i32>,
}

fn table() -> String {
    ViewType::Table.as_str().to_owned()
}

/// Checks a view's settings against the collection's fields and project.
async fn checked_config(
    connection: &mut PgConnection,
    project_id: Uuid,
    config: &ViewConfig,
    definitions: &[FieldDefinition],
) -> Result<Option<Response>, AppError> {
    if let Err(message) = views::check(config, definitions) {
        return Ok(Some(invalid("config", message)));
    }
    if let Err(message) =
        views::check_references(connection, project_id, config, definitions).await?
    {
        return Ok(Some(invalid("config", message)));
    }
    Ok(None)
}

fn view_name_message() -> String {
    format!(
        "Give the view a name of 1 to {} characters, on one line.",
        super::MAX_NAME_CHARACTERS
    )
}

async fn create_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CreateViewRequest>,
) -> Result<Response, AppError> {
    let Some(name) = super::normalize_name(&request.name) else {
        return Ok(invalid("name", view_name_message()));
    };
    let Some(view_type) = views::parse_view_type(&request.view_type) else {
        return Ok(invalid("type", views::VIEW_TYPE_MESSAGE));
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionBuild,
            resource_bound_scope("collections", collection_id, "views_create")?,
            &request,
        )
        .await
    );
    let collection = write.collection(collection_id).await?;
    if collection.archived {
        return Ok(archived_collection(&collection));
    }
    let definitions =
        super::field_definitions(&mut write.transaction, project_id, collection_id, true).await?;
    let config = match request.config.clone().for_type(view_type, &definitions) {
        Ok(config) => config,
        Err(message) => return Ok(invalid("config", message)),
    };
    if let Some(refused) =
        checked_config(&mut write.transaction, project_id, &config, &definitions).await?
    {
        return Ok(refused);
    }
    let view = match views::create(
        &mut write.transaction,
        project_id,
        collection_id,
        principal.actor_id,
        NewView {
            name,
            view_type,
            config,
            position: request.position,
        },
    )
    .await?
    {
        CreateViewOutcome::Created(view) => view,
        CreateViewOutcome::NameTaken(view) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "view_name_taken",
                format!("A view is already called \"{}\".", view.name),
                json!({ "field": "name" }),
            ));
        }
        CreateViewOutcome::Full => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "view_limit",
                format!(
                    "This collection has {} views already. Remove one first.",
                    views::MAX_VIEWS
                ),
                Value::Null,
            ));
        }
    };
    record_audit_for(
        &mut write.transaction,
        &write.actor,
        "collection",
        "collection.view_created",
        collection_id,
        json!({ "view": view.id, "name": view.name }),
    )
    .await?;
    super::touch(&mut write.transaction, project_id, collection_id).await?;
    outbox::enqueue_collection_updated(&mut write.transaction, project_id, collection_id, &[])
        .await?;
    write
        .finish(StatusCode::CREATED, &ViewSummary::of(&view, &definitions))
        .await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateViewRequest {
    expected_version: i64,
    name: Option<String>,
    config: Option<ViewConfig>,
    position: Option<i32>,
}

async fn update_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id, view_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateViewRequest>,
) -> Result<Response, AppError> {
    let name = match request.name.as_deref() {
        Some(value) => match super::normalize_name(value) {
            Some(name) => Some(name),
            None => return Ok(invalid("name", view_name_message())),
        },
        None => None,
    };
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionBuild,
            resource_bound_scope("collection_views", view_id, "update")?,
            &request,
        )
        .await
    );
    let collection = write.collection(collection_id).await?;
    if collection.archived {
        return Ok(archived_collection(&collection));
    }
    let definitions =
        super::field_definitions(&mut write.transaction, project_id, collection_id, true).await?;
    if let Some(config) = &request.config
        && let Some(refused) =
            checked_config(&mut write.transaction, project_id, config, &definitions).await?
    {
        return Ok(refused);
    }
    let view = match views::change(
        &mut write.transaction,
        project_id,
        collection_id,
        view_id,
        ViewChange {
            name,
            config: request.config.clone(),
            position: request.position,
            expected_version: Some(request.expected_version),
        },
    )
    .await?
    {
        ChangeViewOutcome::Changed { view, changed } => {
            record_audit_for(
                &mut write.transaction,
                &write.actor,
                "collection",
                "collection.view_updated",
                collection_id,
                json!({ "view": view.id, "name": view.name, "changedFields": changed }),
            )
            .await?;
            super::touch(&mut write.transaction, project_id, collection_id).await?;
            outbox::enqueue_collection_updated(
                &mut write.transaction,
                project_id,
                collection_id,
                &[],
            )
            .await?;
            view
        }
        ChangeViewOutcome::Unchanged(view) => view,
        ChangeViewOutcome::NotFound => return Err(AppError::NotFound("view")),
        ChangeViewOutcome::VersionConflict(view) => {
            return Err(AppError::VersionConflict {
                resource: "view",
                current_version: view.version,
            });
        }
        ChangeViewOutcome::NameTaken(other) => {
            return Ok(refusal(
                StatusCode::CONFLICT,
                "view_name_taken",
                format!("Another view is already called \"{other}\"."),
                json!({ "field": "name" }),
            ));
        }
    };
    write
        .finish(StatusCode::OK, &ViewSummary::of(&view, &definitions))
        .await
}

async fn delete_view(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id, view_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<Response, AppError> {
    let mut write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionBuild,
            resource_bound_scope("collection_views", view_id, "remove")?,
            &(collection_id, view_id),
        )
        .await
    );
    let collection = write.collection(collection_id).await?;
    if collection.archived {
        return Ok(archived_collection(&collection));
    }
    let view =
        match views::remove(&mut write.transaction, project_id, collection_id, view_id).await? {
            RemoveViewOutcome::Removed(view) => view,
            RemoveViewOutcome::NotFound => return Err(AppError::NotFound("view")),
            RemoveViewOutcome::LastView => {
                return Ok(refusal(
                    StatusCode::CONFLICT,
                    "last_view",
                    "A collection keeps at least one view. Add another before removing this one.",
                    Value::Null,
                ));
            }
        };
    record_audit_for(
        &mut write.transaction,
        &write.actor,
        "collection",
        "collection.view_removed",
        collection_id,
        json!({ "view": view.id, "name": view.name }),
    )
    .await?;
    super::touch(&mut write.transaction, project_id, collection_id).await?;
    outbox::enqueue_collection_updated(&mut write.transaction, project_id, collection_id, &[])
        .await?;
    write
        .finish(StatusCode::OK, &json!({ "removed": view.id }))
        .await
}

// ---------------------------------------------------------------------------
// Records

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecordsQuery {
    /// A saved view to read through; the parts below replace its own.
    view: Option<Uuid>,
    /// A JSON array of `{"field", "op", "values"}`, values as stored.
    filters: Option<String>,
    /// A JSON array of `{"field", "direction"}`.
    sort: Option<String>,
    /// A field's ID, or "" for no grouping.
    group_by: Option<String>,
    /// Grouped by a date: "day", "week" or "month".
    group_bucket: Option<crate::fields::DateBucket>,
    /// Only records in this group (a key from `groups`): a board's column.
    group: Option<String>,
    /// A date field's ID, with `from` and `to`: only records whose date is
    /// from `from` up to (not including) `to`, both YYYY-MM-DD. A
    /// calendar's month.
    date_field: Option<Uuid>,
    from: Option<String>,
    to: Option<String>,
    search: Option<String>,
    archived: Option<String>,
    cursor: Option<String>,
    limit: Option<i64>,
}

/// One record in a page, with its group when grouped.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordRow {
    #[serde(flatten)]
    record: RecordView,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<GroupRef>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordPage {
    records: Vec<RecordRow>,
    /// The people, tasks and pages the values name.
    references: Vec<ValueReference>,
    total: i64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    groups: Vec<GroupCount>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

async fn list_records(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id)): ApiPath<(Uuid, Uuid)>,
    ApiQuery(query): ApiQuery<RecordsQuery>,
) -> Result<Response, AppError> {
    let mut connection = state.pool.acquire().await?;
    reader(&mut connection, &headers, &principal, project_id).await?;
    let collection = super::get(&mut *connection, project_id, collection_id)
        .await?
        .ok_or(AppError::NotFound("collection"))?;
    let definitions =
        super::field_definitions(&mut connection, project_id, collection_id, true).await?;
    let mut config = match query.view {
        Some(view_id) => views::list(&mut *connection, project_id, collection_id)
            .await?
            .into_iter()
            .find(|view| view.id == view_id)
            .ok_or(AppError::NotFound("view"))?
            .config
            .active(&definitions),
        None => ViewConfig::default(),
    };
    if let Some(filters) = query.filters.as_deref() {
        match serde_json::from_str::<Filter>(filters) {
            Ok(filter) => config.filters = filter,
            Err(_) => {
                return Ok(invalid(
                    "filters",
                    "Filters are a JSON list of {field, op, values}, or {\"and\": [...]} or {\"or\": [...]} of them.",
                ));
            }
        }
    }
    if let Some(sort) = query.sort.as_deref() {
        match serde_json::from_str::<Vec<SortRule>>(sort) {
            Ok(rules) => config.sort = rules,
            Err(_) => {
                return Ok(invalid(
                    "sort",
                    "Sort is a JSON list of {field, direction}.",
                ));
            }
        }
    }
    if let Some(group_by) = query.group_by.as_deref() {
        config.group_by = if group_by.trim().is_empty() {
            None
        } else {
            match group_by.trim().parse::<Uuid>() {
                Ok(id) => Some(id),
                Err(_) => return Ok(invalid("groupBy", "Group by a field's ID.")),
            }
        };
    }
    if query.group_bucket.is_some() {
        config.group_bucket = query.group_bucket;
    }
    if let Err(message) = views::check(&config, &definitions) {
        return Ok(invalid("filters", message));
    }
    let Some(archived) = Archived::parse(query.archived.as_deref().unwrap_or("")) else {
        return Ok(invalid("archived", "archived is exclude, include or only."));
    };
    let Some(words) = query::search_words(query.search.as_deref().unwrap_or("")) else {
        return Ok(invalid(
            "search",
            format!(
                "Search for at most {} words in {} characters.",
                query::MAX_SEARCH_WORDS,
                query::MAX_SEARCH_CHARACTERS
            ),
        ));
    };
    let mut spec =
        Spec::from_config(&config, &definitions, principal.actor_id).searching(words, &definitions);
    spec.archived = archived;
    let source = super::source(&collection);
    if let Some(group) = query.group {
        if spec.group.is_none() {
            return Ok(invalid("group", "A group is read within a grouping."));
        }
        if group.is_empty() || group.len() > query::MAX_GROUP_KEY_LENGTH {
            return Ok(invalid(
                "group",
                "Name a group by a key the groups list gives.",
            ));
        }
        spec.in_group = Some(group);
    }
    match (query.date_field, query.from.as_deref(), query.to.as_deref()) {
        (None, None, None) => {}
        (Some(field), Some(from), Some(to)) => {
            match query::date_range(&definitions, field, from, to) {
                Ok(clauses) => spec.narrow(clauses),
                Err(message) => return Ok(invalid("dateField", message)),
            }
        }
        _ => {
            return Ok(invalid(
                "dateField",
                "A date range takes dateField, from and to together.",
            ));
        }
    }
    let limit = query.limit.unwrap_or(query::DEFAULT_LIMIT);
    let Ok(page) = query::page(
        &mut *connection,
        source.as_ref(),
        &spec,
        query.cursor.as_deref(),
        limit,
    )
    .await?
    else {
        return Ok(invalid("cursor", query::CURSOR_MESSAGE));
    };
    let total = query::total(&mut *connection, source.as_ref(), &spec).await?;
    let groups = query::group_counts(&mut *connection, source.as_ref(), &spec).await?;
    let ids = page.rows.iter().map(|(id, _)| *id).collect::<Vec<_>>();
    let found = records::by_ids(
        &mut *connection,
        project_id,
        collection_id,
        &ids,
        collection.acts_like_task,
    )
    .await?;
    let references = records::references(&mut connection, project_id, &definitions, &found).await?;
    let mut groups_by_id = page.rows.into_iter().collect::<HashMap<_, _>>();
    Ok(Json(RecordPage {
        records: found
            .iter()
            .map(|record| RecordRow {
                record: RecordView::of(record, &definitions),
                group: groups_by_id.remove(&record.id).flatten(),
            })
            .collect(),
        references,
        total,
        groups,
        next_cursor: page.next_cursor,
    })
    .into_response())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordBody {
    record: RecordView,
    references: Vec<ValueReference>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    history: Vec<records::HistoryEntry>,
}

async fn get_record(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id, record_id)): ApiPath<(Uuid, Uuid, Uuid)>,
) -> Result<Json<RecordBody>, AppError> {
    let mut connection = state.pool.acquire().await?;
    reader(&mut connection, &headers, &principal, project_id).await?;
    let tasks = super::get(&mut *connection, project_id, collection_id)
        .await?
        .ok_or(AppError::NotFound("collection"))?
        .acts_like_task;
    let record = records::find(
        &mut *connection,
        project_id,
        collection_id,
        RecordRef::Id(record_id),
        tasks,
    )
    .await?
    .ok_or(AppError::NotFound("record"))?;
    let definitions =
        super::field_definitions(&mut connection, project_id, collection_id, true).await?;
    let references = records::references(
        &mut connection,
        project_id,
        &definitions,
        std::slice::from_ref(&record),
    )
    .await?;
    let history = records::history(
        &mut *connection,
        project_id,
        record.id,
        records::HISTORY_LIMIT,
    )
    .await?;
    Ok(Json(RecordBody {
        record: RecordView::of(&record, &definitions),
        references,
        history,
    }))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRecordRequest {
    title: Option<String>,
    /// Values by field ID (or name), as the table edits them.
    #[serde(default)]
    values: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UpdateRecordRequest {
    expected_version: i64,
    title: Option<String>,
    #[serde(default)]
    values: Map<String, Value>,
    archived: Option<bool>,
}

fn record_problem(problem: records::WriteProblem) -> Result<Response, AppError> {
    Ok(match problem.kind {
        ProblemKind::Invalid => invalid(&problem.field, problem.message),
        ProblemKind::NotFound => return Err(AppError::NotFound("record")),
        ProblemKind::Conflict => match problem.current_version {
            Some(current_version) => {
                return Err(AppError::VersionConflict {
                    resource: "record",
                    current_version,
                });
            }
            None => refusal(
                StatusCode::CONFLICT,
                "record_archived",
                problem.message,
                json!({ "field": problem.field }),
            ),
        },
        ProblemKind::Full => refusal(
            StatusCode::CONFLICT,
            "collection_full",
            problem.message,
            Value::Null,
        ),
    })
}

/// Writes one record and answers with it, or with why not.
async fn write_record(
    mut write: Write,
    project_id: Uuid,
    collection_id: Uuid,
    input: RecordInput,
    status: StatusCode,
) -> Result<Response, AppError> {
    let collection = write.collection(collection_id).await?;
    if collection.archived {
        return Ok(archived_collection(&collection));
    }
    if collection.acts_like_task {
        return write_task_record(write, project_id, &collection, input, status).await;
    }
    let definitions =
        super::field_definitions(&mut write.transaction, project_id, collection_id, true).await?;
    let project_key = write.project_key.clone();
    let writer = Writer {
        actor_id: write.actor.actor_id,
        project_key: &project_key,
    };
    let written = match records::write(
        &mut write.transaction,
        project_id,
        &collection,
        writer,
        &definitions,
        "values",
        std::slice::from_ref(&input),
    )
    .await?
    {
        Ok(written) => written,
        Err(problem) => return record_problem(problem),
    };
    let Some(written) = written.into_iter().next() else {
        return Err(AppError::Internal("record write returned nothing"));
    };
    if !matches!(written, Written::Unchanged(_)) {
        outbox::enqueue_collection_updated(
            &mut write.transaction,
            project_id,
            collection_id,
            &[written.record().id],
        )
        .await?;
    }
    let record = written.record();
    let references = records::references(
        &mut write.transaction,
        project_id,
        &definitions,
        std::slice::from_ref(record),
    )
    .await?;
    let body = RecordBody {
        record: RecordView::of(record, &definitions),
        references,
        history: Vec::new(),
    };
    write.finish(status, &body).await
}

/// A write to a collection whose records are tasks: a task change
/// (`super::task_writes`), answered with the task as a record.
async fn write_task_record(
    mut write: Write,
    project_id: Uuid,
    collection: &super::Collection,
    input: RecordInput,
    status: StatusCode,
) -> Result<Response, AppError> {
    let collection_id = collection.id;
    let project_key = write.project_key.clone();
    let task_id = match super::task_writes::write(
        &mut write.transaction,
        &write.actor,
        &project_key,
        collection,
        &input,
    )
    .await?
    {
        Ok(task_id) => task_id,
        Err(problem) => return record_problem(problem),
    };
    outbox::enqueue_collection_updated(
        &mut write.transaction,
        project_id,
        collection_id,
        &[task_id],
    )
    .await?;
    let record = records::find(
        &mut *write.transaction,
        project_id,
        collection_id,
        RecordRef::Id(task_id),
        true,
    )
    .await?
    .ok_or(AppError::Internal("written task is missing"))?;
    let definitions =
        super::field_definitions(&mut write.transaction, project_id, collection_id, true).await?;
    let references = records::references(
        &mut write.transaction,
        project_id,
        &definitions,
        std::slice::from_ref(&record),
    )
    .await?;
    let body = RecordBody {
        record: RecordView::of(&record, &definitions),
        references,
        history: Vec::new(),
    };
    write.finish(status, &body).await
}

fn written_values(values: &Map<String, Value>) -> Vec<(String, Value)> {
    values
        .iter()
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect()
}

async fn create_record(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<CreateRecordRequest>,
) -> Result<Response, AppError> {
    let write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionRecordEdit,
            resource_bound_scope("collections", collection_id, "records_create")?,
            &request,
        )
        .await
    );
    write_record(
        write,
        project_id,
        collection_id,
        RecordInput {
            record: None,
            title: request.title.clone(),
            values: written_values(&request.values),
            archived: None,
            expected_version: None,
        },
        StatusCode::CREATED,
    )
    .await
}

async fn update_record(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, collection_id, record_id)): ApiPath<(Uuid, Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateRecordRequest>,
) -> Result<Response, AppError> {
    let write = begin!(
        start_write(
            &state,
            &headers,
            &principal,
            project_id,
            Action::CollectionRecordEdit,
            resource_bound_scope("collection_records", record_id, "update")?,
            &request,
        )
        .await
    );
    write_record(
        write,
        project_id,
        collection_id,
        RecordInput {
            record: Some(RecordRef::Id(record_id)),
            title: request.title.clone(),
            values: written_values(&request.values),
            archived: request.archived,
            expected_version: Some(request.expected_version),
        },
        StatusCode::OK,
    )
    .await
}
