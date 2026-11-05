//! HTTP routes for a project's task fields (AWD-89). Everyone in the
//! project reads them; owners and admins add, rename, reorder, archive and
//! restore them, and change a choice field's options. The rules live in
//! `crate::fields`, which MCP's field tools call too.

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    authorize_work_item, capabilities,
    planning::{IncludeArchivedQuery, Started, begin, invalid, is_manager, refusal, start_write},
    project_membership,
};
use crate::{
    AppState,
    auth::AuthenticatedActor,
    error::{ApiJson, ApiPath, ApiQuery, AppError},
    fields::{
        self, ChangeFieldOutcome, CreateFieldOutcome, FieldChange, FieldType, FieldView, NewField,
        OptionInput,
    },
    idempotency::resource_bound_scope,
    policy::Action,
    work_item_commands::record_audit_for,
    work_item_fields::{self, KIND},
};

pub(super) async fn list_fields(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiQuery(query): ApiQuery<IncludeArchivedQuery>,
) -> Result<Json<Value>, AppError> {
    let membership = project_membership(&state.pool, principal.actor_id, project_id)
        .await?
        .ok_or(AppError::NotFound("project"))?;
    authorize_work_item(&principal, &membership, Action::WorkItemView, None)?;
    let definitions = work_item_fields::definitions(
        &state.pool,
        project_id,
        query.include_archived.unwrap_or(false),
    )
    .await?;
    let can_manage = is_manager(&membership)?
        && capabilities(&membership)?.edit
        && membership.project_archived_at.is_none();
    Ok(Json(json!({
        "fields": definitions.iter().map(FieldView::from).collect::<Vec<_>>(),
        "canManage": can_manage,
    })))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateFieldRequest {
    name: String,
    #[serde(rename = "type")]
    field_type: String,
    #[serde(default)]
    description: String,
    options: Option<Vec<OptionInput>>,
    target: Option<String>,
    position: Option<i32>,
}

fn managers_only() -> Response {
    refusal(
        StatusCode::FORBIDDEN,
        "forbidden",
        "Only the project's owners and admins can change its fields.".to_owned(),
        Value::Null,
    )
}

fn full_message() -> String {
    format!(
        "This project already has {} task fields, or has kept {} with archived ones. Archive one first.",
        fields::MAX_ACTIVE_FIELDS,
        fields::MAX_FIELD_ROWS
    )
}

pub(super) async fn create_field(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
    ApiJson(request): ApiJson<CreateFieldRequest>,
) -> Result<Response, AppError> {
    let Some(name) = fields::normalize_name(&request.name) else {
        return Ok(invalid(
            "name",
            format!(
                "Give the field a name of 1 to {} characters, on one line.",
                fields::MAX_NAME_CHARACTERS
            ),
        ));
    };
    let Some(field_type) = FieldType::parse(&request.field_type) else {
        return Ok(invalid(
            "type",
            "Pick a type: text, number, date, single_choice, multiple_choice, person, url, checkbox or link.",
        ));
    };
    let Some(description) = fields::normalize_description(&request.description) else {
        return Ok(invalid(
            "description",
            format!(
                "The description is one line of at most {} characters.",
                fields::MAX_DESCRIPTION_CHARACTERS
            ),
        ));
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
            Action::WorkItemEdit,
            None,
            resource_bound_scope("projects", project_id, "fields_create")?,
            &request,
        )
        .await
    );
    if !is_manager(&write.membership)? {
        return Ok(managers_only());
    }
    let outcome = fields::create(
        &mut write.transaction,
        project_id,
        KIND,
        principal.actor_id,
        NewField {
            name,
            description,
            field_type,
            config,
            position: request.position,
        },
    )
    .await?;
    let field = match outcome {
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
                full_message(),
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
        json!({ "name": field.name, "type": field.field_type.as_str(), "recordKind": KIND.as_str() }),
    )
    .await?;
    write
        .finish(StatusCode::CREATED, &FieldView::from(&field))
        .await
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UpdateFieldRequest {
    expected_version: i64,
    name: Option<String>,
    description: Option<String>,
    options: Option<Vec<OptionInput>>,
    position: Option<i32>,
    archived: Option<bool>,
}

pub(super) async fn update_field(
    State(state): State<AppState>,
    headers: HeaderMap,
    principal: AuthenticatedActor,
    ApiPath((project_id, field_id)): ApiPath<(Uuid, Uuid)>,
    ApiJson(request): ApiJson<UpdateFieldRequest>,
) -> Result<Response, AppError> {
    let name = match request.name.as_deref() {
        Some(name) => match fields::normalize_name(name) {
            Some(name) => Some(name),
            None => {
                return Ok(invalid(
                    "name",
                    format!(
                        "Give the field a name of 1 to {} characters, on one line.",
                        fields::MAX_NAME_CHARACTERS
                    ),
                ));
            }
        },
        None => None,
    };
    let description = match request.description.as_deref() {
        Some(text) => match fields::normalize_description(text) {
            Some(text) => Some(text),
            None => {
                return Ok(invalid(
                    "description",
                    "The description is one line of at most 300 characters.",
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
            Action::WorkItemEdit,
            None,
            resource_bound_scope("field_definitions", field_id, "update")?,
            &request,
        )
        .await
    );
    if !is_manager(&write.membership)? {
        return Ok(managers_only());
    }
    let definitions =
        work_item_fields::definitions(&mut *write.transaction, project_id, true).await?;
    let Some(current) = definitions.iter().find(|field| field.id == field_id) else {
        return Err(AppError::NotFound("field"));
    };
    let config = match request.options.as_deref() {
        Some(options) => {
            match fields::normalize_config(
                current.field_type,
                Some(&current.config),
                Some(options),
                None,
            ) {
                Ok(config) => Some(config),
                Err(problem) => return Ok(invalid(problem.field, problem.message)),
            }
        }
        None => None,
    };
    let outcome = fields::change(
        &mut write.transaction,
        project_id,
        KIND,
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
    .await?;
    let field = match outcome {
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
                }),
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
                full_message(),
                Value::Null,
            ));
        }
    };
    write.finish(StatusCode::OK, &FieldView::from(&field)).await
}

/// The web app's `fieldValues` ({field ID or name: value}), checked and
/// resolved in this transaction, or the refusal to send.
pub(super) async fn web_field_values(
    connection: &mut sqlx::PgConnection,
    project_id: Uuid,
    actor_id: Uuid,
    written: &serde_json::Map<String, Value>,
) -> Result<Result<work_item_fields::FieldValueChanges, Response>, AppError> {
    let project_key =
        sqlx::query_scalar::<_, String>("SELECT project_key FROM projects WHERE id = $1")
            .bind(project_id)
            .fetch_one(&mut *connection)
            .await?;
    let definitions = work_item_fields::definitions(&mut *connection, project_id, true).await?;
    let written = written
        .iter()
        .map(|(field, value)| (field.clone(), value.clone()))
        .collect::<Vec<_>>();
    Ok(work_item_fields::resolve(
        connection,
        project_id,
        work_item_fields::Writer {
            actor_id,
            project_key: &project_key,
        },
        &definitions,
        "fieldValues",
        &written,
    )
    .await?
    .map_err(|problem| invalid(&problem.field, problem.message)))
}
