use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    api::select_run_setup,
    auth::AuthenticatedActor,
    domain::allocate_conversation_sequence,
    error::{ApiPath, AppError},
    external_events::enqueue_external_event,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    mcp::McpPrincipal,
    message_content::MessageContent,
    outbox,
    policy::{
        Action, ActorKind, Decision, Membership, MembershipRole, ProjectScope, Resource, RoleGrant,
        Subject, authorize,
    },
    provider::run_config::RunConfig,
};

pub fn router() -> Router<AppState> {
    Router::new().route(
        "/api/v1/projects/{project_id}/external-execution-requests",
        get(list_sponsored_execution_requests),
    )
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExternalExecutionRequestArguments {
    pub conversation_id: String,
    pub repository_id: String,
    pub sponsor_actor_id: String,
    pub prompt: String,
    pub client_request_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, schemars::JsonSchema)]
#[serde(
    tag = "outcome",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum ExternalExecutionRequestResult {
    Requested {
        request_id: String,
        approval_id: String,
        status: String,
        version: i64,
        idempotent_replay: bool,
        canonical_path: String,
    },
    NotFound {
        retry_category: String,
    },
    PermissionDenied {
        retry_category: String,
    },
    Conflict {
        retry_category: String,
    },
    InvalidRequest {
        field: String,
        retry_category: String,
    },
    TemporarilyUnavailable {
        retry_category: String,
    },
}

impl ExternalExecutionRequestResult {
    pub(crate) fn denied() -> Self {
        Self::PermissionDenied {
            retry_category: "reauthenticate_or_request_scope".to_owned(),
        }
    }

    fn invalid(field: &str) -> Self {
        Self::InvalidRequest {
            field: field.to_owned(),
            retry_category: "correct_input".to_owned(),
        }
    }

    fn unavailable() -> Self {
        Self::TemporarilyUnavailable {
            retry_category: "retry_with_backoff".to_owned(),
        }
    }

    fn replayed(mut self) -> Self {
        if let Self::Requested {
            idempotent_replay, ..
        } = &mut self
        {
            *idempotent_replay = true;
        }
        self
    }
}

#[derive(Debug, FromRow)]
struct ExternalAuthorization {
    membership_role: String,
    membership_role_permissions: Option<Vec<String>>,
}

#[derive(Debug, FromRow)]
struct SponsorAuthorization {
    membership_role: String,
    membership_role_permissions: Option<Vec<String>>,
}

#[derive(Debug, FromRow)]
struct SponsoredApprovalContext {
    project_id: Uuid,
    connection_id: Uuid,
    requesting_actor_id: Uuid,
    sponsoring_actor_id: Uuid,
    conversation_id: Uuid,
    repository_id: Uuid,
    prompt: String,
    request_status: String,
    request_version: i64,
    request_expired: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SponsoredExecutionList {
    requests: Vec<SponsoredExecutionProjection>,
}

#[derive(Debug, FromRow, Serialize)]
#[serde(rename_all = "camelCase")]
struct SponsoredExecutionProjection {
    id: Uuid,
    approval_id: Uuid,
    requesting_actor_id: Uuid,
    requester_display_name: String,
    requester_handle: String,
    conversation_id: Uuid,
    repository_id: Uuid,
    prompt: String,
    status: String,
    version: i64,
    agent_run_id: Option<Uuid>,
    expires_at: String,
    created_at: String,
}

pub(crate) async fn request_sponsored_execution(
    state: &AppState,
    principal: &McpPrincipal,
    mut arguments: ExternalExecutionRequestArguments,
) -> ExternalExecutionRequestResult {
    if principal.actor_kind != ActorKind::ExternalAgent
        || principal.external_agent_connection_id.is_none()
        || !principal.can_write()
        || !principal.has_scope("worker_request")
    {
        return ExternalExecutionRequestResult::denied();
    }
    let conversation_id = match arguments.conversation_id.parse::<Uuid>() {
        Ok(value) => value,
        Err(_) => return ExternalExecutionRequestResult::invalid("conversationId"),
    };
    let repository_id = match arguments.repository_id.parse::<Uuid>() {
        Ok(value) => value,
        Err(_) => return ExternalExecutionRequestResult::invalid("repositoryId"),
    };
    let sponsor_actor_id = match arguments.sponsor_actor_id.parse::<Uuid>() {
        Ok(value) if value != principal.actor_id => value,
        _ => return ExternalExecutionRequestResult::invalid("sponsorActorId"),
    };
    arguments.prompt = arguments.prompt.trim().to_owned();
    if MessageContent::from_plain_text(arguments.prompt.clone()).is_err() {
        return ExternalExecutionRequestResult::invalid("prompt");
    }
    let key = match IdempotencyKey::parse(&arguments.client_request_id) {
        Ok(value) => value,
        Err(_) => return ExternalExecutionRequestResult::invalid("clientRequestId"),
    };
    match request_sponsored_execution_inner(
        state,
        principal,
        arguments,
        conversation_id,
        repository_id,
        sponsor_actor_id,
        key,
    )
    .await
    {
        Ok(result) => result,
        Err(AppError::NotFound(_)) => ExternalExecutionRequestResult::NotFound {
            retry_category: "do_not_retry".to_owned(),
        },
        Err(
            AppError::Forbidden | AppError::AuthenticationRequired | AppError::InvalidCredentials,
        ) => ExternalExecutionRequestResult::denied(),
        Err(AppError::Conflict(_, _) | AppError::RepositoryBindingConflict) => {
            ExternalExecutionRequestResult::Conflict {
                retry_category: "refetch_before_retry".to_owned(),
            }
        }
        Err(AppError::RepositoryChoiceRequired { .. } | AppError::ExecutionProfileRequired) => {
            ExternalExecutionRequestResult::Conflict {
                retry_category: "ask_sponsor_to_configure_execution".to_owned(),
            }
        }
        Err(error) => {
            tracing::error!(
                category = error.log_category(),
                "external execution request failed"
            );
            ExternalExecutionRequestResult::unavailable()
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn request_sponsored_execution_inner(
    state: &AppState,
    principal: &McpPrincipal,
    arguments: ExternalExecutionRequestArguments,
    conversation_id: Uuid,
    repository_id: Uuid,
    sponsor_actor_id: Uuid,
    key: IdempotencyKey,
) -> Result<ExternalExecutionRequestResult, AppError> {
    let connection_id = principal
        .external_agent_connection_id
        .ok_or(AppError::Forbidden)?;
    let mut transaction = crate::db::scoped_tx(&state.pool).await?;
    let external = sqlx::query_as::<_, ExternalAuthorization>(
        r#"
        SELECT membership.role AS membership_role,
               workspace_role_permissions(membership.role_id) AS membership_role_permissions
        FROM mcp_client_credentials credential
        JOIN external_agent_connections connection
          ON connection.id = credential.external_agent_connection_id
        JOIN actors actor ON actor.id = credential.actor_id
        JOIN projects project ON project.id = $2
        JOIN project_memberships membership
          ON membership.project_id = project.id
         AND membership.actor_id = credential.actor_id
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = credential.actor_id
        WHERE credential.id = $1
          -- The credential stays anchored to its home project; the run may be
          -- in any project this connection can act in.
          AND agent_connection_can_access(connection.id, project.id)
          AND credential.actor_id = $3
          AND credential.external_agent_connection_id = $4
          AND credential.status = 'active' AND credential.revoked_at IS NULL
          AND credential.expires_at > now()
          AND credential.scopes @> ARRAY['write', 'worker_request']::text[]
          AND connection.status = 'active'
          AND connection.actor_id = credential.actor_id
          AND connection.project_id = credential.project_id
          AND connection.scopes = credential.scopes
          AND actor.kind = 'external_agent' AND actor.status = 'active'
          AND project.archived_at IS NULL
          AND membership.enabled AND organization_membership.enabled
        FOR SHARE OF credential, connection, actor, project, membership,
                     organization_membership
        "#,
    )
    .bind(principal.credential_id)
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .bind(connection_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::Forbidden)?;
    require_policy(
        principal.actor_id,
        ActorKind::ExternalAgent,
        principal.project_id,
        &external.membership_role,
        external.membership_role_permissions.as_deref(),
        Action::WorkerInvoke,
    )?;

    let active_thread = sqlx::query_scalar::<_, bool>(
        r#"
        SELECT conversation.kind = 'thread'
               AND conversation.archived_at IS NULL
               AND parent.archived_at IS NULL
        FROM conversations conversation
        JOIN conversations parent ON parent.id = conversation.parent_channel_id
        WHERE conversation.id = $1 AND conversation.project_id = $2
          AND conversation_visible_to(conversation.id, $3)
        FOR SHARE OF conversation, parent
        "#,
    )
    .bind(conversation_id)
    .bind(principal.project_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("conversation"))?;
    if !active_thread {
        return Err(AppError::NotFound("conversation"));
    }

    let sponsor = sqlx::query_as::<_, SponsorAuthorization>(
        r#"
        SELECT membership.role AS membership_role,
               workspace_role_permissions(membership.role_id) AS membership_role_permissions
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $2
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
        JOIN actors actor ON actor.id = membership.actor_id
        JOIN users sponsor_user ON sponsor_user.actor_id = actor.id
        WHERE project.id = $1 AND project.archived_at IS NULL
          AND membership.enabled AND organization_membership.enabled
          AND actor.kind = 'human' AND actor.status = 'active'
          AND sponsor_user.status = 'active'
        FOR SHARE OF project, membership, organization_membership, actor, sponsor_user
        "#,
    )
    .bind(principal.project_id)
    .bind(sponsor_actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("sponsor"))?;
    require_policy(
        sponsor_actor_id,
        ActorKind::Human,
        principal.project_id,
        &sponsor.membership_role,
        sponsor.membership_role_permissions.as_deref(),
        Action::WorkerInvoke,
    )?;
    select_run_setup(
        &mut transaction,
        principal.project_id,
        conversation_id,
        sponsor_actor_id,
        Some(repository_id),
    )
    .await?;

    let scope =
        idempotency::resource_bound_scope("external_execution_thread", conversation_id, "request")?;
    let request_hash = RequestHash::for_value(&arguments)?;
    let reservation = match idempotency::reserve(
        &mut transaction,
        principal.actor_id,
        &scope,
        &key,
        request_hash,
    )
    .await?
    {
        IdempotencyOutcome::Replay(replay) => {
            let result = replay
                .decode::<ExternalExecutionRequestResult>()?
                .replayed();
            transaction.commit().await?;
            return Ok(result);
        }
        IdempotencyOutcome::Reserved(value) => value,
    };

    let request_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO external_execution_requests (
            id, project_id, external_agent_connection_id,
            requesting_actor_id, sponsoring_actor_id,
            conversation_id, repository_id, prompt
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        "#,
    )
    .bind(request_id)
    .bind(principal.project_id)
    .bind(connection_id)
    .bind(principal.actor_id)
    .bind(sponsor_actor_id)
    .bind(conversation_id)
    .bind(repository_id)
    .bind(&arguments.prompt)
    .execute(&mut *transaction)
    .await?;
    // A1.14: its approval is a request to the sponsor, about the run request.
    let workspace_id =
        sqlx::query_scalar::<_, Uuid>("SELECT organization_id FROM projects WHERE id = $1")
            .bind(principal.project_id)
            .fetch_one(&mut *transaction)
            .await?;
    let raised = crate::requests::open(
        &mut transaction,
        crate::requests::Origin {
            account_id: principal.actor_id,
            workspace_id,
            space_id: Some(principal.project_id),
            run_id: None,
            host_request_ref: None,
            conversation_id: Some(conversation_id),
            work_item_id: None,
            subject: Some((REQUEST_SUBJECT, request_id)),
            field: "request",
        },
        &serde_json::json!({
            "kind": "approval",
            "title": "An agent asks to run work in your name.",
            "options": [
                {"id": "approve", "label": "Approve", "style": "approve"},
                {"id": "deny", "label": "Deny", "style": "deny"},
            ],
            "allowText": false,
            "audience": {"accounts": [sponsor_actor_id]},
        }),
    )
    .await
    .map_err(|error| match error {
        crate::requests::OpenError::App(error) => error,
        crate::requests::OpenError::Refused { .. } => {
            AppError::Invalid("sponsored execution request")
        }
    })?;
    crate::requests::announce_raised(&mut transaction, raised.request_id).await?;
    let approval_id = raised.request_id;
    enqueue_external_event(
        &mut transaction,
        connection_id,
        principal.project_id,
        "external_execution.requested",
        "Sponsored Worker execution is awaiting human approval",
        "external_execution_request",
        request_id,
        &request_href(principal.project_id, request_id),
    )
    .await?;
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, authentication_class, action,
            target_kind, target_id, outcome, channel, client_name,
            mcp_credential_id, external_agent_connection_id, metadata
        ) VALUES (
            $1, $2, $3, 'mcp_bearer', 'external_execution.requested',
            'external_execution_request', $4, 'succeeded', 'mcp', $5,
            $6, $7, jsonb_build_object(
                'conversation_id', $8, 'repository_id', $9, 'sponsor_actor_id', $10
            )
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(principal.project_id)
    .bind(request_id)
    .bind(&principal.client_name)
    .bind(principal.credential_id)
    .bind(connection_id)
    .bind(conversation_id)
    .bind(repository_id)
    .bind(sponsor_actor_id)
    .execute(&mut *transaction)
    .await?;
    let result = ExternalExecutionRequestResult::Requested {
        request_id: request_id.to_string(),
        approval_id: approval_id.to_string(),
        status: "pending".to_owned(),
        version: 1,
        idempotent_replay: false,
        canonical_path: request_href(principal.project_id, request_id),
    };
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::ACCEPTED,
        &result,
    )
    .await?;
    transaction.commit().await?;
    Ok(result)
}

async fn list_sponsored_execution_requests(
    State(state): State<AppState>,
    principal: AuthenticatedActor,
    ApiPath(project_id): ApiPath<Uuid>,
) -> Result<Json<SponsoredExecutionList>, AppError> {
    let (role, role_permissions) = sqlx::query_as::<_, (String, Option<Vec<String>>)>(
        r#"
        SELECT membership.role, workspace_role_permissions(membership.role_id)
        FROM projects project
        JOIN project_memberships membership
          ON membership.project_id = project.id AND membership.actor_id = $2
        JOIN organization_memberships organization_membership
          ON organization_membership.organization_id = project.organization_id
         AND organization_membership.actor_id = $2
         AND organization_membership.enabled
        WHERE project.id = $1 AND project.archived_at IS NULL AND membership.enabled
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_optional(crate::db::scoped(&state.pool))
    .await?
    .ok_or(AppError::NotFound("project"))?;
    require_policy(
        principal.actor_id,
        ActorKind::Human,
        project_id,
        &role,
        role_permissions.as_deref(),
        Action::WorkerInvoke,
    )?;
    let requests = sqlx::query_as::<_, SponsoredExecutionProjection>(
        r#"
        SELECT request.id, approval.id AS approval_id,
               request.requesting_actor_id,
               requester.display_name AS requester_display_name,
               requester.handle AS requester_handle,
               request.conversation_id, request.repository_id, request.prompt,
               request.status, request.version, request.agent_run_id,
               request.expires_at::text AS expires_at,
               request.created_at::text AS created_at
        FROM external_execution_requests request
        JOIN requests approval
          ON approval.subject_kind = 'run_request' AND approval.subject_id = request.id
        JOIN project_memberships requester
          ON requester.project_id = request.project_id
         AND requester.actor_id = request.requesting_actor_id
        WHERE request.project_id = $1 AND request.sponsoring_actor_id = $2
        ORDER BY (request.status = 'pending') DESC, request.created_at DESC, request.id DESC
        LIMIT 100
        "#,
    )
    .bind(project_id)
    .bind(principal.actor_id)
    .fetch_all(crate::db::scoped(&state.pool))
    .await?;
    Ok(Json(SponsoredExecutionList { requests }))
}

/// What a sponsored run request's approval is about (`requests.subject_kind`).
pub(crate) const REQUEST_SUBJECT: &str = "run_request";

/// A sponsored run request's approval settled (A1.14). Approved, the run
/// starts in the sponsor's name, or the answer is refused when it can no
/// longer start; denied, unanswered in time or withdrawn, the run request
/// ends there. The agent that asked hears which.
pub(crate) async fn approval_settled(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    row: &crate::requests::RequestRow,
) -> Result<(), AppError> {
    let Some(external_request_id) = row.subject_id else {
        return Ok(());
    };
    let approved = row.status == "answered"
        && row
            .answer
            .as_ref()
            .and_then(|answer| answer.get("optionId"))
            .and_then(serde_json::Value::as_str)
            == Some("approve");
    if !approved {
        let status = match row.status.as_str() {
            "answered" => "denied",
            "expired" => "expired",
            _ => "canceled",
        };
        let ended = sqlx::query_as::<_, (Uuid, Uuid)>(
            r#"
            UPDATE external_execution_requests
            SET status = $2, version = version + 1, resolved_at = now(), updated_at = now()
            WHERE id = $1 AND status = 'pending'
            RETURNING external_agent_connection_id, project_id
            "#,
        )
        .bind(external_request_id)
        .bind(status)
        .fetch_optional(&mut **transaction)
        .await?;
        if let Some((connection_id, project_id)) = ended
            && status == "denied"
        {
            enqueue_external_event(
                transaction,
                connection_id,
                project_id,
                "external_execution.denied",
                "Sponsored Worker execution was denied",
                "external_execution_request",
                external_request_id,
                &request_href(project_id, external_request_id),
            )
            .await?;
        }
        return Ok(());
    }
    let context = sqlx::query_as::<_, SponsoredApprovalContext>(
        r#"
        -- visibility: by id, for the sponsor answering the run request's
        -- request (requests::may_answer checked them); locks the thread the
        -- run will start in.
        SELECT request.project_id,
               request.external_agent_connection_id AS connection_id,
               request.requesting_actor_id,
               request.sponsoring_actor_id,
               request.conversation_id,
               request.repository_id,
               request.prompt,
               request.status AS request_status,
               request.version AS request_version,
               request.expires_at <= now() AS request_expired
        FROM external_execution_requests request
        JOIN external_agent_connections connection
          ON connection.id = request.external_agent_connection_id
         AND connection.project_id = request.project_id
         AND connection.actor_id = request.requesting_actor_id
         AND connection.status = 'active'
        JOIN actors requester ON requester.id = request.requesting_actor_id
         AND requester.kind = 'external_agent' AND requester.status = 'active'
        JOIN project_memberships requester_membership
          ON requester_membership.project_id = request.project_id
         AND requester_membership.actor_id = request.requesting_actor_id
         AND requester_membership.enabled
        JOIN projects project ON project.id = request.project_id
         AND project.archived_at IS NULL
        JOIN conversations conversation ON conversation.id = request.conversation_id
         AND conversation.project_id = request.project_id
         AND conversation.kind = 'thread' AND conversation.archived_at IS NULL
        JOIN conversations parent ON parent.id = conversation.parent_channel_id
         AND parent.archived_at IS NULL
        JOIN repositories repository ON repository.id = request.repository_id
         AND repository.project_id = request.project_id
         AND repository.enabled AND repository.trusted
         AND repository.archived_at IS NULL
         AND repository.preparation_state = 'ready'
        JOIN project_memberships sponsor_membership
          ON sponsor_membership.project_id = request.project_id
         AND sponsor_membership.actor_id = request.sponsoring_actor_id
         AND sponsor_membership.enabled
        JOIN actors sponsor ON sponsor.id = request.sponsoring_actor_id
         AND sponsor.kind = 'human' AND sponsor.status = 'active'
        JOIN users sponsor_user ON sponsor_user.actor_id = sponsor.id
         AND sponsor_user.status = 'active'
        JOIN organization_memberships sponsor_organization_membership
          ON sponsor_organization_membership.organization_id = project.organization_id
         AND sponsor_organization_membership.actor_id = sponsor.id
         AND sponsor_organization_membership.enabled
        WHERE request.id = $1 AND request.sponsoring_actor_id = $2
        FOR UPDATE OF request, conversation
        FOR SHARE OF connection, requester, requester_membership, project, parent,
                     repository, sponsor_membership, sponsor, sponsor_user,
                     sponsor_organization_membership
        "#,
    )
    .bind(external_request_id)
    .bind(row.answered_by_account_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(AppError::Conflict(
        "external_execution_request",
        "this run request can no longer start",
    ))?;
    if context.request_expired {
        return Err(AppError::Conflict(
            "external_execution_request",
            "sponsored execution request expired",
        ));
    }
    if context.request_status != "pending" {
        return Err(AppError::Conflict(
            "external_execution_request",
            "sponsored execution request already resolved",
        ));
    }
    let run_id = launch_sponsored_run(state, transaction, &context, external_request_id).await?;
    sqlx::query(
        r#"
        UPDATE external_execution_requests
        SET status = 'launched', version = version + 1, agent_run_id = $2,
            resolved_at = now(), updated_at = now()
        WHERE id = $1 AND status = 'pending' AND version = $3
        "#,
    )
    .bind(external_request_id)
    .bind(run_id)
    .bind(context.request_version)
    .execute(&mut **transaction)
    .await?;
    enqueue_external_event(
        transaction,
        context.connection_id,
        context.project_id,
        "external_execution.approved",
        "Sponsored Worker execution was approved and queued",
        "external_execution_request",
        external_request_id,
        &request_href(context.project_id, external_request_id),
    )
    .await?;
    // The built-in host's holder starts it (K5.3).
    sqlx::query("SELECT pg_notify(realtime_channel('control'), 'host:kick')")
        .execute(&mut **transaction)
        .await?;
    Ok(())
}

async fn launch_sponsored_run(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    context: &impl SponsoredRunContext,
    external_request_id: Uuid,
) -> Result<Uuid, AppError> {
    // H1.24: no run starts in a suspended or deleting workspace.
    crate::workspace::lifecycle::hold_open_for_space(
        &mut *transaction,
        context.project_id(),
        "run_start",
    )
    .await?;
    // Approvals come from the browser and phone, not an app: no cap, as for
    // a start there.
    crate::hosts::builtin::admit(transaction, context.conversation_id(), None).await?;
    let setup = select_run_setup(
        transaction,
        context.project_id(),
        context.conversation_id(),
        context.sponsoring_actor_id(),
        Some(context.repository_id()),
    )
    .await?;
    let bound = sqlx::query(
        r#"
        UPDATE conversations
        SET repository_id = COALESCE(repository_id, $2), updated_at = now()
        WHERE id = $1 AND project_id = $3
          AND (repository_id IS NULL OR repository_id = $2)
        "#,
    )
    .bind(context.conversation_id())
    .bind(setup.repository_id)
    .bind(context.project_id())
    .execute(&mut **transaction)
    .await?;
    if bound.rows_affected() != 1 {
        return Err(AppError::RepositoryBindingConflict);
    }
    let (prompt_content, prompt, mentions) =
        MessageContent::from_plain_text(context.prompt().to_owned())
            .map_err(|_| AppError::Validation("Message content"))?
            .into_parts();
    debug_assert!(mentions.is_empty());
    let prompt_content = serde_json::to_value(prompt_content)
        .map_err(|_| AppError::Internal("serialize sponsored prompt Message"))?;
    let prompt_message_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, project_id, conversation_id, author_actor_id,
            content_version, content, plain_text
        ) VALUES ($1, $2, $3, $4, 1, $5, $6)
        "#,
    )
    .bind(prompt_message_id)
    .bind(context.project_id())
    .bind(context.conversation_id())
    .bind(context.requesting_actor_id())
    .bind(prompt_content)
    .bind(&prompt)
    .execute(&mut **transaction)
    .await?;
    let sequence = allocate_conversation_sequence(transaction, context.conversation_id()).await?;
    sqlx::query(
        r#"
        INSERT INTO conversation_entries (id, conversation_id, sequence, kind, message_id)
        VALUES ($1, $2, $3, 'message', $4)
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(context.conversation_id())
    .bind(sequence)
    .bind(prompt_message_id)
    .execute(&mut **transaction)
    .await?;
    // Settings freeze when the run is created, by the same rule as every
    // other start: the sponsor's and the project's run settings over the
    // sponsor's account defaults, each checked against the harness.
    let config = crate::hosts::builtin::resolve_config(
        state,
        transaction,
        context.project_id(),
        context.sponsoring_actor_id(),
        &setup,
        &RunConfig::default(),
    )
    .await?;
    let created = crate::hosts::builtin::create(
        transaction,
        crate::hosts::builtin::NewRun {
            project_id: context.project_id(),
            conversation_id: context.conversation_id(),
            setup: &setup,
            requested_by: context.requesting_actor_id(),
            sponsored_by: Some(context.sponsoring_actor_id()),
            external_execution_request_id: Some(external_request_id),
            predecessor_run_id: None,
            prompt: &prompt,
            prompt_message_id: Some(prompt_message_id),
            authorization: serde_json::json!({
                "actor_id": context.requesting_actor_id(),
                "sponsor_actor_id": context.sponsoring_actor_id(),
                "project_id": context.project_id(),
                "external_execution_request_id": external_request_id,
                "scope": ["repository.read", "repository.write", "provider.turn"]
            }),
            config: &config,
            made_through_app: false,
        },
    )
    .await?;
    outbox::enqueue_message_created(
        transaction,
        context.project_id(),
        context.conversation_id(),
        prompt_message_id,
        sequence,
        context.requesting_actor_id(),
    )
    .await?;
    Ok(created.run_id)
}

trait SponsoredRunContext {
    fn project_id(&self) -> Uuid;
    fn requesting_actor_id(&self) -> Uuid;
    fn sponsoring_actor_id(&self) -> Uuid;
    fn conversation_id(&self) -> Uuid;
    fn repository_id(&self) -> Uuid;
    fn prompt(&self) -> &str;
}

impl SponsoredRunContext for SponsoredApprovalContext {
    fn project_id(&self) -> Uuid {
        self.project_id
    }

    fn requesting_actor_id(&self) -> Uuid {
        self.requesting_actor_id
    }

    fn sponsoring_actor_id(&self) -> Uuid {
        self.sponsoring_actor_id
    }

    fn conversation_id(&self) -> Uuid {
        self.conversation_id
    }

    fn repository_id(&self) -> Uuid {
        self.repository_id
    }

    fn prompt(&self) -> &str {
        &self.prompt
    }
}

/// A person's or agent's stored role; workers never come through here.
fn parse_role(value: &str, permissions: Option<&[String]>) -> Result<RoleGrant, AppError> {
    match RoleGrant::stored(value, permissions) {
        Ok(grant) if grant.role() != MembershipRole::Worker => Ok(grant),
        _ => Err(AppError::Forbidden),
    }
}

fn require_policy(
    actor_id: Uuid,
    actor_kind: ActorKind,
    project_id: Uuid,
    role: &str,
    role_permissions: Option<&[String]>,
    action: Action,
) -> Result<(), AppError> {
    let role = parse_role(role, role_permissions)?;
    let subject = Subject::new(
        actor_id,
        actor_kind,
        true,
        Some(Membership::project(project_id, role, true)),
        None,
    );
    if matches!(
        authorize(
            &subject,
            action,
            Resource::Project(ProjectScope::project(project_id)),
        ),
        Decision::Allow
    ) {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Where an external execution request's events point.
fn request_href(project_id: Uuid, request_id: Uuid) -> String {
    format!("/projects/{project_id}/external-execution-requests/{request_id}")
}
