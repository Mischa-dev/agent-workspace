use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
};
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Postgres, Transaction};
use uuid::Uuid;

use crate::{
    AppState,
    api::{ApprovalResponse, select_run_setup},
    auth::AuthenticatedActor,
    conversations::{ApprovalCapabilities, ApprovalRow, ApprovalView},
    domain::{RunState, allocate_conversation_sequence, transition_run},
    error::{ApiPath, AppError},
    external_events::enqueue_external_event,
    idempotency::{self, IdempotencyKey, IdempotencyOutcome, RequestHash},
    mcp::McpPrincipal,
    message_content::MessageContent,
    outbox,
    policy::{
        Action, ActorKind, ApprovalEligibility, ApprovalRequirements, Decision, Membership,
        MembershipRole, ProjectScope, Resource, RoleGrant, Subject, authorize,
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
    approval_status: String,
    approval_response: Option<serde_json::Value>,
    responded_by_actor_id: Option<Uuid>,
    sponsor_role: String,
    sponsor_role_permissions: Option<Vec<String>>,
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
    let mut transaction = state.pool.begin().await?;
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
    let approval_id = Uuid::now_v7();
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
    sqlx::query(
        r#"
        INSERT INTO agent_approvals (
            id, agent_run_id, external_execution_request_id,
            requesting_actor_id, eligible_actor_id, provider_request_id,
            kind, summary, options, eligible_role, human_required
        ) VALUES (
            $1, NULL, $2, $3, $4, $5,
            'approval', 'External agent requested sponsored Worker execution.',
            '["approve", "deny"]'::jsonb, $6, true
        )
        "#,
    )
    .bind(approval_id)
    .bind(request_id)
    .bind(principal.actor_id)
    .bind(sponsor_actor_id)
    .bind(format!("external-execution:{request_id}"))
    .bind(&sponsor.membership_role)
    .execute(&mut *transaction)
    .await?;
    crate::requests::legacy::approval_written(&mut *transaction, approval_id).await?;
    enqueue_external_event(
        &mut transaction,
        connection_id,
        principal.project_id,
        "external_execution.requested",
        "Sponsored Worker execution is awaiting human approval",
        "external_execution_request",
        request_id,
        &format!(
            "/projects/{}/external-execution-requests/{request_id}",
            principal.project_id
        ),
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
        canonical_path: format!(
            "/projects/{}/external-execution-requests/{request_id}",
            principal.project_id
        ),
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
    .fetch_optional(&state.pool)
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
        JOIN agent_approvals approval
          ON approval.external_execution_request_id = request.id
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
    .fetch_all(&state.pool)
    .await?;
    Ok(Json(SponsoredExecutionList { requests }))
}

pub(crate) async fn respond_to_sponsored_execution(
    state: &AppState,
    headers: &HeaderMap,
    principal: &AuthenticatedActor,
    approval_id: Uuid,
    external_request_id: Uuid,
    response: ApprovalResponse,
) -> Result<ApprovalView, AppError> {
    if response.answer.is_some()
        || !matches!(response.decision.as_deref(), Some("approve" | "deny"))
    {
        return Err(AppError::Invalid("sponsored execution decision"));
    }
    let decision = response
        .decision
        .as_deref()
        .ok_or(AppError::Invalid("sponsored execution decision"))?;
    let response_json = serde_json::json!({"decision": decision});
    let request_hash = RequestHash::for_value(&response)?;

    let mut transaction = state.pool.begin().await?;
    let context = sqlx::query_as::<_, SponsoredApprovalContext>(
        r#"
        SELECT request.project_id,
               request.external_agent_connection_id AS connection_id,
               request.requesting_actor_id,
               request.sponsoring_actor_id,
               request.conversation_id,
               request.repository_id,
               request.prompt,
               request.status AS request_status,
               request.version AS request_version,
               request.expires_at <= now() AS request_expired,
               approval.status AS approval_status,
               approval.response AS approval_response,
               approval.responded_by_actor_id,
               sponsor_membership.role AS sponsor_role,
               workspace_role_permissions(sponsor_membership.role_id) AS sponsor_role_permissions
        FROM external_execution_requests request
        JOIN agent_approvals approval
          ON approval.external_execution_request_id = request.id
         AND approval.id = $2
         AND approval.eligible_actor_id = $3
         AND approval.human_required
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
        WHERE request.id = $1 AND request.sponsoring_actor_id = $3
        FOR UPDATE OF request, approval, conversation
        FOR SHARE OF connection, requester, requester_membership, project, parent,
                     repository, sponsor_membership, sponsor, sponsor_user,
                     sponsor_organization_membership
        "#,
    )
    .bind(external_request_id)
    .bind(approval_id)
    .bind(principal.actor_id)
    .fetch_optional(&mut *transaction)
    .await?
    .ok_or(AppError::NotFound("agent_approval"))?;
    let sponsor_role = parse_role(
        &context.sponsor_role,
        context.sponsor_role_permissions.as_deref(),
    )?;
    let sponsor = Subject::new(
        principal.actor_id,
        ActorKind::Human,
        true,
        Some(Membership::project(context.project_id, sponsor_role, true)),
        None,
    );
    if !matches!(
        authorize(
            &sponsor,
            Action::ApprovalRespond,
            Resource::Approval {
                scope: ProjectScope::approval(
                    context.project_id,
                    context.conversation_id,
                    external_request_id,
                    context.repository_id,
                    approval_id,
                ),
                requirements: ApprovalRequirements {
                    requesting_actor_id: context.requesting_actor_id,
                    eligibility: ApprovalEligibility::Actor(context.sponsoring_actor_id),
                    human_required: true,
                    separation_of_duty: true,
                },
            },
        ),
        Decision::Allow
    ) {
        return Err(AppError::NotFound("agent_approval"));
    }
    if context.request_expired {
        return Err(AppError::Conflict(
            "external_execution_request",
            "sponsored execution request expired",
        ));
    }

    let key = IdempotencyKey::from_headers(headers)?;
    let scope = idempotency::resource_bound_scope("approval", approval_id, "respond")?;
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
            let reference = replay.decode::<SponsoredApprovalReference>()?;
            if reference.approval_id != approval_id {
                return Err(AppError::Internal("sponsored approval replay mismatch"));
            }
            transaction.commit().await?;
            return fetch_sponsored_approval(state, approval_id, context.project_id).await;
        }
        IdempotencyOutcome::Reserved(value) => value,
    };
    if context.request_status != "pending" || context.approval_status != "pending" {
        if context.responded_by_actor_id == Some(principal.actor_id)
            && context.approval_response == Some(response_json.clone())
        {
            idempotency::complete(
                &mut transaction,
                &reservation,
                StatusCode::OK,
                &SponsoredApprovalReference { approval_id },
            )
            .await?;
            transaction.commit().await?;
            return fetch_sponsored_approval(state, approval_id, context.project_id).await;
        }
        return Err(AppError::Conflict(
            "external_execution_request",
            "sponsored execution request already resolved",
        ));
    }

    let run_id = if decision == "approve" {
        Some(launch_sponsored_run(state, &mut transaction, &context, external_request_id).await?)
    } else {
        None
    };
    sqlx::query(
        r#"
        UPDATE agent_approvals
        SET status = 'resolved', responded_by_actor_id = $2,
            response = $3, responded_at = now()
        WHERE id = $1 AND status = 'pending'
        "#,
    )
    .bind(approval_id)
    .bind(principal.actor_id)
    .bind(&response_json)
    .execute(&mut *transaction)
    .await?;
    crate::requests::legacy::approval_written(&mut *transaction, approval_id).await?;
    sqlx::query(
        r#"
        UPDATE external_execution_requests
        SET status = $2, version = version + 1, agent_run_id = $3,
            resolved_at = now(), updated_at = now()
        WHERE id = $1 AND status = 'pending' AND version = $4
        "#,
    )
    .bind(external_request_id)
    .bind(if run_id.is_some() {
        "launched"
    } else {
        "denied"
    })
    .bind(run_id)
    .bind(context.request_version)
    .execute(&mut *transaction)
    .await?;
    enqueue_external_event(
        &mut transaction,
        context.connection_id,
        context.project_id,
        if run_id.is_some() {
            "external_execution.approved"
        } else {
            "external_execution.denied"
        },
        if run_id.is_some() {
            "Sponsored Worker execution was approved and queued"
        } else {
            "Sponsored Worker execution was denied"
        },
        "external_execution_request",
        external_request_id,
        &format!(
            "/projects/{}/external-execution-requests/{external_request_id}",
            context.project_id
        ),
    )
    .await?;
    let request_id = headers
        .get(crate::REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|value| value.len() <= 128);
    sqlx::query(
        r#"
        INSERT INTO audit_events (
            id, actor_id, project_id, session_id, authentication_class,
            action, target_kind, target_id, outcome, request_id, channel,
            external_agent_connection_id, metadata
        ) VALUES (
            $1, $2, $3, $4, 'password_session',
            'external_execution.responded', 'external_execution_request',
            $5, 'succeeded', $6, 'browser', $7,
            jsonb_build_object('decision', $8, 'agent_run_id', $9)
        )
        "#,
    )
    .bind(Uuid::now_v7())
    .bind(principal.actor_id)
    .bind(context.project_id)
    .bind(principal.session_id)
    .bind(external_request_id)
    .bind(request_id)
    .bind(context.connection_id)
    .bind(decision)
    .bind(run_id)
    .execute(&mut *transaction)
    .await?;
    idempotency::complete(
        &mut transaction,
        &reservation,
        StatusCode::OK,
        &SponsoredApprovalReference { approval_id },
    )
    .await?;
    transaction.commit().await?;
    if run_id.is_some() {
        crate::scheduler::kick(state.clone());
    }
    fetch_sponsored_approval(state, approval_id, context.project_id).await
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SponsoredApprovalReference {
    approval_id: Uuid,
}

async fn launch_sponsored_run(
    state: &AppState,
    transaction: &mut Transaction<'_, Postgres>,
    context: &impl SponsoredRunContext,
    external_request_id: Uuid,
) -> Result<Uuid, AppError> {
    if sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM agent_runs
            WHERE conversation_id = $1
              AND state IN ('created', 'queued', 'starting', 'working', 'blocked', 'recovering')
        )
        "#,
    )
    .bind(context.conversation_id())
    .fetch_one(&mut **transaction)
    .await?
    {
        return Err(AppError::Conflict(
            "agent_run",
            "Thread already has an active run",
        ));
    }
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
    // Settings freeze when the run is created, as for every other start:
    // the sponsor's account defaults, each one that no longer fits the
    // catalog falling back to the harness's own.
    let config = crate::provider::resolve_stored_run_config(
        state,
        setup.organization_id,
        &setup.adapter_kind,
        &setup.run_defaults,
        &RunConfig::default(),
    )
    .await?;
    let run_id = Uuid::now_v7();
    sqlx::query(
        r#"
        INSERT INTO agent_runs (
            id, conversation_id, execution_worker_id, requesting_actor_id,
            sponsoring_actor_id, external_execution_request_id,
            execution_profile_id, repository_id, state, prompt,
            authorization_snapshot, prompt_message_id, config
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'created', $9, $10, $11, $12)
        "#,
    )
    .bind(run_id)
    .bind(context.conversation_id())
    .bind(setup.execution_worker_id)
    .bind(context.requesting_actor_id())
    .bind(context.sponsoring_actor_id())
    .bind(external_request_id)
    .bind(setup.execution_profile_id)
    .bind(setup.repository_id)
    .bind(&prompt)
    .bind(serde_json::json!({
        "actor_id": context.requesting_actor_id(),
        "sponsor_actor_id": context.sponsoring_actor_id(),
        "project_id": context.project_id(),
        "external_execution_request_id": external_request_id,
        "scope": ["repository.read", "repository.write", "provider.turn"]
    }))
    .bind(prompt_message_id)
    .bind(serde_json::to_value(&config).map_err(|_| AppError::Internal("serialize run settings"))?)
    .execute(&mut **transaction)
    .await?;
    transition_run(transaction, run_id, RunState::Queued, None).await?;
    outbox::enqueue_message_created(
        transaction,
        context.project_id(),
        context.conversation_id(),
        prompt_message_id,
        sequence,
        context.requesting_actor_id(),
    )
    .await?;
    outbox::enqueue_agent_run_updated(
        transaction,
        context.project_id(),
        context.conversation_id(),
        run_id,
    )
    .await?;
    Ok(run_id)
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

async fn fetch_sponsored_approval(
    state: &AppState,
    approval_id: Uuid,
    project_id: Uuid,
) -> Result<ApprovalView, AppError> {
    let approval = sqlx::query_as::<_, ApprovalRow>(
        r#"
        SELECT approval.id, request.agent_run_id AS agent_run_id,
               approval.kind, approval.summary, approval.options,
               approval.status, approval.response,
               approval.created_at::text AS created_at,
               request.repository_id, approval.requesting_actor_id,
               approval.eligible_role, approval.human_required
        FROM agent_approvals approval
        JOIN external_execution_requests request
          ON request.id = approval.external_execution_request_id
         AND request.project_id = $2
        WHERE approval.id = $1
        "#,
    )
    .bind(approval_id)
    .bind(project_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound("agent_approval"))?;
    Ok(ApprovalView {
        approval,
        capabilities: ApprovalCapabilities { respond: false },
    })
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
